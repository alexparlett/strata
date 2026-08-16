//! The drawer's **Problems** body — a scope strip over one of two lists (P4-15 item 3).
//!
//! ## Why a strip inside one body, and not a fourth rail entry
//!
//! The rail's bottom group chooses between *surfaces*: Problems, Events, History — three
//! different kinds of record. These two are the same kind, **problems**, at two scopes: what is
//! wrong with the SQL you have open, and what is wrong with the project underneath it. That is a
//! different axis, and collapsing it onto the rail would say the two are as unrelated as
//! Problems and History are. (JetBrains' own Problems panel splits on exactly this axis, with
//! the tool-window selector doing what our rail does.)
//!
//! The selected scope rides [`Layout`](strata_model::Layout) like every other panel decision, so
//! it survives collapsing the drawer, switching to Events and back, and a restart.
//!
//! ## Three kinds of state, two of which live here
//!
//! Worth naming, because it is what earns the split rather than decorating it:
//!
//! | | re-derivable? | who writes it | surface |
//! |---|---|---|---|
//! | A diagnostic | yes — buffer revision + catalog epoch | one driver, `state::diagnostics` | Queries |
//! | A registration failure | yes — `Reg::Failed` on the row | the scan pass | Project |
//! | A write fault | **no** — it describes a write that already happened | its observer, `persisted` | Project |
//! | An event | no, and it is already finished | its observer | *Events, not here* |
//!
//! The first three are **conditions**: true now, retracted when they stop being true. The fourth
//! is a moment. That line is the drawer's, not this strip's — but the third row is why the
//! Project scope exists, because a write fault is a condition that no amount of re-derivation
//! can produce and so had nowhere to be shown for as long as it held.

mod project;
mod queries;

use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::ProblemsTab;

use super::DrawerTheme;
use crate::apps::project::state::{Chan, FaultsCtx, ProjChan, ProjectState, SessionState};
use freya::components::{Activable, FloatingTab};

use crate::components::badge::Badge;
use crate::components::metrics::{SP_1, SP_2, SP_3, SP_4, SP_7};
use crate::components::tones::{tones, Tones};
use crate::components::typography::Control;

pub use project::project_error_count;

/// One problem row, and the group header above it (canvas `--sp-2` / `--sp-3` verticals).
///
/// `ROW_HEIGHT` is a **floor**, not the height: both scopes' messages wrap, so a row is one line
/// tall when it fits and as tall as it needs otherwise.
pub(super) const ROW_HEIGHT: f32 = 26.;
pub(super) const GROUP_HEIGHT: f32 = 32.;
/// A row's vertical inset. Zero while every row was exactly one `ROW_HEIGHT` line — a fixed
/// height centres its own content — and load-bearing now that a wrapped message sets the height
/// itself. Shared, because both scopes' rows wrap and a drawer whose two lists inset differently
/// reads as two lists.
pub(super) const ROW_INSET: f32 = SP_2;
/// A row's left indent — the canvas's `--sp-7`, so rows sit under their group's name.
pub(super) const ROW_INDENT: f32 = SP_7;
/// The panel's horizontal padding (canvas `--sp-4`).
pub(super) const PAD: f32 = SP_4;

/// The Problems body: whichever scope the header's strip has selected.
///
/// Deliberately thin — it holds no counts and writes no drawer tally, because the strip that
/// labels the scopes is in the **header** (see [`ScopeStrip`]) and already has to know both
/// numbers to label its tabs. A second tally here would be a second walk of the same two stores
/// and a number that could disagree with the one two lines above it.
#[derive(PartialEq)]
pub struct Problems {
    pub theme: DrawerTheme,
}

impl Component for Problems {
    fn render(&self) -> impl IntoElement {
        let layout = use_radio::<SessionState, Chan>(Chan::Layout);
        let tones = tones();
        let scope = layout.read().layout.problems_tab;

        let el: Element = match scope {
            ProblemsTab::Queries => queries::Queries {
                theme: self.theme.clone(),
            }
            .into_element(),
            ProblemsTab::Project => project::Project {
                theme: self.theme.clone(),
                tones,
            }
            .into_element(),
        };
        el
    }
}

/// The scope strip, which lives in the **drawer header** beside the "Problems" title — the
/// IntelliJ arrangement, where the panel's name and its scopes share one bar and each scope
/// carries its own count (`Problems  File 6 | Project Errors | …`).
///
/// In the header rather than at the top of the body because that is where a *selector* belongs:
/// the body is what the selection produced, and a strip inside it reads as part of the content it
/// is choosing. It also puts the counts where the drawer's own tally used to be, which is why
/// Problems is the one tab whose header shows no separate number — the tabs are the number.
/// Carries no theme: its only children are [`ScopeTab`]s, and a tab dresses itself from the
/// `floating_tab` component theme.
#[derive(PartialEq)]
pub struct ScopeStrip;

impl Component for ScopeStrip {
    fn render(&self) -> impl IntoElement {
        let mut layout = use_radio::<SessionState, Chan>(Chan::Layout);
        let session = use_radio::<SessionState, Chan>(Chan::Diagnostics);
        let connections = use_radio::<ProjectState, ProjChan>(ProjChan::Connections);
        let tables = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        let views = use_radio::<ProjectState, ProjChan>(ProjChan::Views);
        let faults = use_consume::<FaultsCtx>();
        let tones = tones();

        let scope = layout.read().layout.problems_tab;
        let queries = session.read().error_count();
        let _ = connections.read();
        let _ = views.read();
        let project = project_error_count(&tables.read(), &faults.read());

        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(SP_1)
            .child(
                ScopeTab {
                    label: "Queries",
                    count: queries,
                    selected: scope == ProblemsTab::Queries,
                    tones,
                    on_press: EventHandler::new(move |_: Event<PressEventData>| {
                        layout
                            .write_channel(Chan::Layout)
                            .show_problems_tab(ProblemsTab::Queries);
                    }),
                }
                .into_element(),
            )
            .child(
                ScopeTab {
                    label: "Project",
                    count: project,
                    selected: scope == ProblemsTab::Project,
                    tones,
                    on_press: EventHandler::new(move |_: Event<PressEventData>| {
                        layout
                            .write_channel(Chan::Layout)
                            .show_problems_tab(ProblemsTab::Project);
                    }),
                }
                .into_element(),
            )
    }
}

/// One scope in the strip: its name, and its own error count when it has any.
///
/// Freya's [`FloatingTab`] rather than a rect that looks like one. The three
/// things it brings are the three a hand-rolled version silently goes without: the
/// `AccessibilityRole::Tab` that tells a screen reader this *is* a tab, the focusability that
/// lets the keyboard reach it, and the hover fill + pointer cursor that make it read as
/// pressable before you press it.
///
/// Selection rides [`Activable`], exactly as `components::sidebar_row` does — the context it
/// provides is what `FloatingTab` reads internally, so "which tab is on" stays the caller's and
/// the component keeps no selection state of its own.
///
/// The count stays **composed** as a child rather than becoming a component field: a tab has no
/// opinion about what is written on it, which is the same line the Engine pane's `Table` work
/// drew. The children need a horizontal wrapper because `FloatingTab` centres them in the
/// default (vertical) direction, so label and badge would otherwise stack.
#[derive(PartialEq)]
struct ScopeTab {
    label: &'static str,
    count: usize,
    selected: bool,
    tones: Tones,
    on_press: EventHandler<Event<PressEventData>>,
}

impl Component for ScopeTab {
    fn render(&self) -> impl IntoElement {
        let on_press = self.on_press.clone();
        Activable::new(
            FloatingTab::new()
                .on_press(move |e| on_press.call(e))
                .child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(SP_3)
                        .child(Control::new(self.label))
                        .maybe_child((self.count > 0).then(|| {
                            Badge::value(self.count.to_string(), self.tones.error).outlined()
                        })),
                ),
        )
        .active(self.selected)
    }
}

/// Strip + body tests — the rendered surface, over stores stood up by hand.
///
/// **These exist because of a runtime crash.** Every component here consumes `FaultsCtx` from
/// context, and `use_consume` panics when it is absent; the first version of this shipped with the
/// Configure window missing that provider, and the only thing that caught it was a person pressing
/// the button. A test that *mounts* the tree fails on a missing provider at once, where the
/// projection tests beside it (`project::tests`) never touch context at all.
#[cfg(test)]
mod tests {
    use super::super::{DrawerThemePartial, DrawerThemePreference};
    use super::*;
    use crate::apps::project::state::{Log, PersistFaults, ProjectFile};
    use crate::theme::strata_theme;
    use freya::components::get_theme;
    use freya::prelude::use_init_theme;
    use freya::radio::RadioStation;
    use freya_testing::TestingRunner;
    use std::path::PathBuf;
    use strata_core::project::ProjectDefs;
    use strata_core::theme::load;
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

    fn app() -> impl IntoElement {
        use_init_theme(|| strata_theme(&load("midnight")));
        let theme = get_theme!(&None::<DrawerThemePartial>, DrawerThemePreference, "drawer");
        rect()
            .expanded()
            .child(ScopeStrip)
            .child(Problems { theme })
    }

    /// The window's stores, plus the two handles a missing provider would panic on.
    fn runner(
        faults: PersistFaults,
        failed: Option<&str>,
    ) -> (TestingRunner, RadioStation<SessionState, Chan>) {
        let mut store = {
            let defs = ProjectDefs {
                name: "p".into(),
                tables: vec![table("orders")],
                views: Vec::new(),
                saved_queries: Vec::new(),
                ..Default::default()
            };
            ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-problems-render"))
        };
        if let Some(why) = failed {
            store.table_failed("orders", why.into());
        }
        TestingRunner::new(
            app,
            (600., 400.).into(),
            move |r| {
                let session = r.provide_root_context(|| {
                    RadioStation::<SessionState, Chan>::create(SessionState::default())
                });
                r.provide_root_context(|| RadioStation::<ProjectState, ProjChan>::create(store));
                r.provide_root_context(|| State::create(Log::default()));
                r.provide_root_context(|| State::create(faults));
                session
            },
            1.,
        )
    }

    /// Every text run in the tree, top to bottom — the drawer's own test idiom.
    fn texts(runner: &TestingRunner) -> Vec<String> {
        runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
    }

    /// Settle the tree *and its effects* (see `events`'s note: a render and the effect it dirties
    /// are several polls apart).
    fn settle(runner: &mut TestingRunner) {
        for _ in 0..4 {
            runner.sync_and_update();
        }
    }

    /// The strip mounts and names both scopes. Failing to construct the tree at all is the
    /// missing-provider case this module exists to catch.
    #[test]
    fn the_strip_offers_both_scopes() {
        let (mut runner, _) = runner(PersistFaults::default(), None);
        settle(&mut runner);

        let seen = texts(&runner);
        assert!(
            seen.iter().any(|t| t == "Queries"),
            "no Queries tab: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t == "Project"),
            "no Project tab: {seen:?}"
        );
    }

    /// A clean project shows the Queries scope by default, and its empty state — `Queries` is
    /// `ProblemsTab`'s `Default`, so a session file predating the field lands here too.
    #[test]
    fn a_clean_project_opens_on_queries() {
        let (mut runner, session) = runner(PersistFaults::default(), None);
        settle(&mut runner);

        assert_eq!(session.peek().layout.problems_tab, ProblemsTab::Queries);
        assert!(
            texts(&runner).iter().any(|t| t == "No problems found"),
            "expected the Queries empty state: {:?}",
            texts(&runner)
        );
    }

    /// Selecting Project swaps the body — and the write fault and the refused def both list,
    /// which is the pairing the scope exists for.
    #[test]
    fn the_project_scope_lists_both_families() {
        let mut faults = PersistFaults::default();
        faults.fault(ProjectFile::Defs, "Permission denied".into());
        let (mut runner, session) = runner(faults, Some("No files found"));
        session
            .clone()
            .write_channel(Chan::Layout)
            .show_problems_tab(ProblemsTab::Project);
        settle(&mut runner);

        let seen = texts(&runner).join(" | ");
        assert!(
            seen.contains("project.json") && seen.contains("Permission denied"),
            "no write fault: {seen}"
        );
        assert!(
            seen.contains("orders") && seen.contains("No files found"),
            "no registration fault: {seen}"
        );
        assert!(
            seen.contains("not saved") && seen.contains("table"),
            "no tags: {seen}"
        );
    }

    /// The strip's counts are per scope, so the tab you are *not* on still says it has something.
    #[test]
    fn each_tab_carries_its_own_count() {
        let mut faults = PersistFaults::default();
        faults.fault(ProjectFile::Session, "Read-only file system".into());
        let (mut runner, _) = runner(faults, Some("No files found"));
        settle(&mut runner);

        assert!(
            texts(&runner).iter().any(|t| t == "2"),
            "expected the Project tab to show 2: {:?}",
            texts(&runner)
        );
    }

    /// The Project scope retracts by itself: nothing behind, nothing refused, nothing listed.
    #[test]
    fn the_project_scope_is_empty_when_nothing_is_wrong() {
        let (mut runner, session) = runner(PersistFaults::default(), None);
        session
            .clone()
            .write_channel(Chan::Layout)
            .show_problems_tab(ProblemsTab::Project);
        settle(&mut runner);

        assert!(
            texts(&runner).iter().any(|t| t == "No project problems"),
            "expected the Project empty state: {:?}",
            texts(&runner)
        );
    }
}
