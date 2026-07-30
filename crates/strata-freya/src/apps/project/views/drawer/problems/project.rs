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

use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::CatalogKind;

use super::super::{DrawerBody, DrawerEmpty, DrawerTheme};
use super::{Tones, PAD, ROW_HEIGHT};
use crate::apps::project::state::{FaultsCtx, PersistFaults, ProjChan, ProjectState};
use crate::components::badge::Badge;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Body;

/// One row of this scope, flattened from the two families so the list renders blind.
#[derive(Clone, PartialEq)]
pub struct ProjectProblem {
    /// What the row is about: the def's name, or the file that is behind.
    pub subject: String,
    /// What is wrong with it — the engine's words, or the write's error.
    pub why: String,
    /// The trailing tag: `table` / `view`, or `not saved`.
    pub tag: String,
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
        tag: "not saved".into(),
    });
    let regs = project
        .registration_faults()
        .into_iter()
        .map(|f| ProjectProblem {
            subject: f.name,
            why: f.why,
            tag: match f.kind {
                CatalogKind::View => "view".into(),
                _ => "table".into(),
            },
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
        // `ProjChan::Tables` and `Views` are the two channels a registration answer lands on;
        // the catalog rows already subscribe to exactly these, so a def flipping to `Failed`
        // wakes this list at the same moment it wakes its row.
        let tables = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        let views = use_radio::<ProjectState, ProjChan>(ProjChan::Views);
        let faults = use_consume::<FaultsCtx>();

        let _ = views.read();
        let rows = project_problems(&tables.read(), &faults.read());

        let el: Element = match rows.is_empty() {
            true => DrawerEmpty::new(IconName::Check, "No project problems")
                .icon_color(self.tones.ok)
                .color(self.theme.empty_color)
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

/// One project problem: the error glyph, `subject — why`, and the tag that says which kind it is.
///
/// **Not pressable**, unlike a Queries row. That row jumps to the tab that owns it; these have no
/// single place to go — a registration failure's fix is the Configure window *or* a re-scan
/// depending on why it failed, and a write fault's fix is outside the app entirely. Offering a
/// press that lands somewhere unhelpful is worse than not offering one (P4-15 leaves the retry
/// action to its own pass, since the session writer's retry has to go through the autosave hook
/// rather than around it, or it writes away the window geometry).
#[derive(PartialEq)]
struct ProjectRow {
    row: ProjectProblem,
    theme: DrawerTheme,
    tones: Tones,
}

impl Component for ProjectRow {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::fill())
            .height(Size::px(ROW_HEIGHT))
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(8.)
            .padding((0., PAD))
            .child(Icon::new(IconName::Alert).color(self.tones.error).size(15.))
            .child(
                Body::new(format!("{} — {}", self.row.subject, self.row.why))
                    .color(self.theme.message_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            // A **badge**, not dim trailing text. The tag classifies the row — it is what tells a
            // def the engine refused apart from a file that will not save — so it is a fact, not
            // the incidental annotation `meta_color` dresses elsewhere in this drawer (an Events
            // timestamp, a Queries `line L:C`). As plain `Path` on `meta_color` it resolved to
            // the sheet's `disabled` at **2.15:1** against the drawer, under the 3:1 floor at any
            // size and illegible at mono 400 11.
            //
            // `Badge::value` is the role History's line-count pill already uses, and the tint it
            // derives from its foreground is what makes a small marker read — the same reason
            // that pill is legible on a tone this one could not carry as bare text.
            .child(Badge::value(self.row.tag.clone(), self.theme.value_color))
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
    use strata_model::{SourceFormat, TableDef};

    fn table(name: &str) -> TableDef {
        TableDef {
            name: name.into(),
            format: SourceFormat::Parquet,
            sources: vec![format!("{name}.parquet")],
            partition_cols: vec![],
        }
    }

    fn store() -> ProjectState {
        let defs = ProjectDefs {
            name: "p".into(),
            tables: vec![table("orders"), table("users")],
            views: Vec::new(),
            saved_queries: Vec::new(),
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
        assert_eq!(rows[0].tag, "not saved");
        assert_eq!(rows[1].subject, "orders");
        assert_eq!(rows[1].tag, "table");
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
