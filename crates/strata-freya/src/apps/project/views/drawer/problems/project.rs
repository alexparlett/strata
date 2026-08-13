//! The Problems drawer's **Project** scope: conditions about the project rather than about a
//! query's text (P4-15 item 3).
//!
//! Two families, and the pairing is the point — both are things that are *wrong right now* and
//! stay wrong until something fixes them, which is precisely what the Queries scope's rows are
//! and what an event-log row is not.
//!
//! - **Defs the engine refused.** A projection of `Reg::Failed` off the catalog store
//!   ([`ProjectState::registration_faults`]), so it is re-derived like a diagnostic: a re-scan
//!   that fixes the def retracts the row with nothing to invalidate. Until now this condition
//!   was visible only as a triangle on one catalog row, plus an event that scrolled away — so a
//!   project whose sidebar you had not scrolled to could be broken with nothing saying so.
//! - **`.strata` files that are behind**, because a write failed and none has succeeded since
//!   ([`PersistFaults`]). This one cannot be re-derived — you cannot recompute "the last write
//!   failed" from live state — so an observer records it, and [`persisted`] is that observer.
//!
//! ## Why the write faults are here and not in Events
//!
//! They are in Events too, once: the *transition* into failure. What that log cannot carry is the
//! duration. The session autosave retries every 500ms of activity, so a project on a read-only
//! volume would append an identical row per debounce and evict the log's whole 200-entry
//! contents within a couple of minutes of typing — while still leaving nothing on screen to say
//! the condition was ongoing. One row here, held for exactly as long as it is true, is both the
//! fix for the flood and the thing the user actually needs to see.
//!
//! [`persisted`]: crate::apps::project::state::persisted_defs
//! [`PersistFaults`]: crate::apps::project::state::PersistFaults

use freya::clipboard::Clipboard;
use freya::prelude::*;
use freya::radio::use_radio;

use super::super::{DrawerBody, DrawerEmpty, DrawerTheme};
use super::{PAD, ROW_HEIGHT, ROW_INSET};
use crate::apps::project::state::{FaultKind, FaultsCtx, PersistFaults, ProjChan, ProjectState};
use crate::components::badge::Badge;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::SP_3;
use crate::components::tones::Tones;
use crate::components::typography::Body;

/// The copy button's extent, the chat transcript's `ACTIONS_H`: the same 20px glyph button on the
/// same errand, so the two read as one control rather than two takes on one.
const COPY_EXTENT: f32 = 20.;

/// One row of this scope, flattened from the two families so the list renders blind.
#[derive(Clone, PartialEq)]
pub struct ProjectProblem {
    /// What the row is about: the def's name, or the file that is behind.
    pub subject: String,
    /// What is wrong with it — the engine's words, or the write's error.
    pub why: String,
    /// The trailing tag, which is also the one thing that says *which family* the row came
    /// from.
    pub tag: ProblemTag,
}

/// A project-problem row's trailing tag — the two families this scope flattens, kept as a type
/// so the row carries the distinction rather than a rendered word.
///
/// [`FaultKind`] rides inside it rather than being copied out: a refused def's kind is already
/// a closed vocabulary owned by the store, and converting it to a string here would be the one
/// place the drawer could disagree with it.
#[derive(Clone, Copy, PartialEq)]
pub enum ProblemTag {
    /// A `.strata` file a failed write left behind.
    NotSaved,
    /// A def the engine refused.
    Refused(FaultKind),
}

impl ProblemTag {
    fn label(self) -> &'static str {
        match self {
            Self::NotSaved => "not saved",
            Self::Refused(kind) => kind.label(),
        }
    }
}

/// Every project-scope problem, write faults first.
///
/// Write faults lead deliberately: a def the engine refused is a fault in what the user asked
/// for, where a file that will not write is a fault in the app's ability to keep *anything* they
/// ask for — including the fix for the row below it.
pub fn project_problems(project: &ProjectState, faults: &PersistFaults) -> Vec<ProjectProblem> {
    let writes = faults.rows().into_iter().map(|(file, why)| ProjectProblem {
        subject: file.file_name().to_string(),
        why,
        tag: ProblemTag::NotSaved,
    });
    let regs = project
        .registration_faults()
        .into_iter()
        .map(|f| ProjectProblem {
            subject: f.name,
            why: f.why,
            tag: ProblemTag::Refused(f.kind),
        });
    writes.chain(regs).collect()
}

#[derive(PartialEq)]
pub struct Project {
    pub theme: DrawerTheme,
    pub tones: Tones,
}

impl Component for Project {
    fn render(&self) -> impl IntoElement {
        let connections = use_radio::<ProjectState, ProjChan>(ProjChan::Connections);
        let tables = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        let views = use_radio::<ProjectState, ProjChan>(ProjChan::Views);
        let faults = use_consume::<FaultsCtx>();

        let _ = connections.read();
        let _ = views.read();
        let rows = project_problems(&tables.read(), &faults.read());

        let el: Element = match rows.is_empty() {
            true => DrawerEmpty::new(IconName::Check, "No project problems")
                .icon_color(self.tones.ok)
                .into_element(),
            false => DrawerBody::new()
                .children(rows.into_iter().map(|row| {
                    ProjectRow {
                        row,
                        theme: self.theme.clone(),
                        tones: self.tones,
                    }
                    .into_element()
                }))
                .into_element(),
        };
        el
    }
}

/// One project problem: the error glyph, `subject — why`, a button that copies it, and the tag
/// that says which kind it is.
///
/// **The message wraps**, and this is the surface that gets to. Everywhere else a refusal is one
/// line in something narrow, clipping the engine's sentence — and a cut at the comma keeps the
/// symptom while throwing away the diagnosis. One place has to render the whole thing, so this row
/// has no fixed height and no ellipsis, and the narrow surfaces point here rather than paraphrase.
///
/// **Copy, because a message worth reading is a message worth pasting.** `Body` is not selectable
/// text, so without a button the words are legible and still unreachable.
///
/// **Not pressable**, unlike a Queries row, which jumps to the tab that owns it: these have no
/// single place to go, and a press that lands somewhere unhelpful is worse than none. It is also
/// what lets the copy button exist at all, since a `Button` inside a pressable parent fires both.
#[derive(PartialEq)]
struct ProjectRow {
    row: ProjectProblem,
    theme: DrawerTheme,
    tones: Tones,
}

impl Component for ProjectRow {
    fn render(&self) -> impl IntoElement {
        let text = format!("{} — {}", self.row.subject, self.row.why);

        rect()
            .width(Size::fill())
            .min_height(Size::px(ROW_HEIGHT))
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Start)
            .spacing(SP_3)
            .padding((ROW_INSET, PAD))
            .child(Icon::new(IconName::Alert).color(self.tones.error).size(15.))
            .child(
                Body::new(text.clone())
                    .color(self.theme.message_color)
                    .width(Size::flex(1.))
                    .wrap(),
            )
            .child(CopyProblem {
                text,
                color: self.theme.meta_color,
            })
            .child(Badge::value(self.row.tag.label(), self.theme.value_color))
    }
}

/// The row's **copy** press — `subject — why`, exactly the string the row renders.
///
/// Its own component for the transcript's reason: a sibling row settling re-renders the list, and
/// a button rebuilt mid-hover loses the hover it was in. It also keeps the row's builder reading
/// as a row.
///
/// No tooltip, the same call `CopyMessage` makes and for the same cause — a `TooltipContainer` is
/// an `Attached` overlay, and one hanging off a control this small in a dense list is a hover
/// oscillation.
///
/// It is therefore **unnamed to a screen reader**, which is the same gap `CopyMessage` carries and
/// the same reason: `Button` takes no accessible label, so the name belongs on it *in the fork*
/// (`components::tool_button` notes the same). Wrapping it in an `a11y_alt` rect here would put
/// the name on a node that is not the control, which is a worse answer than the one this shares
/// with the transcript's button.
#[derive(PartialEq)]
struct CopyProblem {
    text: String,
    color: Color,
}

impl Component for CopyProblem {
    fn render(&self) -> impl IntoElement {
        let text = self.text.clone();
        let color = self.color;

        Button::new()
            .flat()
            .width(Size::px(COPY_EXTENT))
            .height(Size::px(COPY_EXTENT))
            .on_press(move |_: Event<PressEventData>| {
                if let Err(err) = Clipboard::set(text.clone()) {
                    tracing::warn!("problem copy failed: {err:?}");
                }
            })
            .child(Icon::new(IconName::Copy).size(12.).color(color))
    }
}

/// How many project-scope problems there are — the strip's tab count, the drawer header's share
/// of the tally, and the rail badge's, all from one function so the three cannot disagree.
///
/// Every row here is an error: a def the engine refused has nothing behind it, and a file that
/// will not write is not a warning about a possible future.
pub fn project_error_count(project: &ProjectState, faults: &PersistFaults) -> usize {
    project.registration_fault_count() + faults.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::project::state::ProjectFile;
    use std::path::PathBuf;
    use strata_core::project::ProjectDefs;
    use strata_model::{SourceFormat, TableDef, TableOrigin};

    fn table(name: &str) -> TableDef {
        TableDef {
            name: name.into(),
            format: SourceFormat::Parquet,
            connection: None,
            sources: vec![format!("{name}.parquet")],
            partition_cols: vec![],
            origin: TableOrigin::External,
        }
    }

    fn store() -> ProjectState {
        let defs = ProjectDefs {
            name: "p".into(),
            tables: vec![table("orders"), table("users")],
            views: Vec::new(),
            saved_queries: Vec::new(),
            ..Default::default()
        };
        ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-project-problems"))
    }

    /// A def the engine refused becomes a row; one still `Loading`, or registered, does not.
    /// `Loading` is the case worth pinning — a project mid-scan must not flash every row it has
    /// not answered for yet as a problem.
    #[test]
    fn only_refused_defs_are_problems() {
        let mut s = store();
        s.table_failed("orders", "No files found".into());

        let rows = s.registration_faults();
        assert_eq!(rows.len(), 1, "one refused def: {rows:?}");
        assert_eq!(rows[0].name, "orders");
        assert_eq!(rows[0].why, "No files found");
    }

    /// The two families land in one list, **write faults first** — a file that will not write is
    /// a fault in the app's ability to keep anything the user asks for, including the fix for the
    /// registration row below it.
    #[test]
    fn write_faults_lead_the_list() {
        let mut s = store();
        s.table_failed("orders", "No files found".into());
        let mut faults = PersistFaults::default();
        faults.fault(ProjectFile::Defs, "Permission denied".into());

        let rows = project_problems(&s, &faults);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].subject, "project.json");
        assert!(matches!(rows[0].tag, ProblemTag::NotSaved));
        assert_eq!(rows[1].subject, "orders");
        assert!(matches!(rows[1].tag, ProblemTag::Refused(FaultKind::Table)));
    }

    /// The count the strip, the drawer header and the rail badge all read is the length of that
    /// same list — asserted here so a future row kind can't be listed and not counted.
    #[test]
    fn the_count_is_the_list() {
        let mut s = store();
        s.table_failed("orders", "No files found".into());
        let mut faults = PersistFaults::default();
        faults.fault(ProjectFile::Session, "Read-only file system".into());

        assert_eq!(
            project_error_count(&s, &faults),
            project_problems(&s, &faults).len()
        );
    }

    /// Nothing wrong, nothing listed — the empty state the drawer renders its tick for.
    #[test]
    fn a_clean_project_has_no_problems() {
        let s = store();
        let faults = PersistFaults::default();
        assert!(project_problems(&s, &faults).is_empty());
        assert_eq!(project_error_count(&s, &faults), 0);
    }
}
