//! The **profile cost confirm** (P3-10 · U15), built to the Strata canvas's `profile` comp: a
//! warning chip beside the action over its subject, the copy that says what a scan actually
//! does, and Cancel over an accent **Run scan**.
//!
//! ## It names the work, not a number
//!
//! The canvas quotes cost figures off a `>50 files` gate ("248 files · ~186 MB"). Deliberately
//! not built (DEV_TASKS D4/U15): file count is a backwards proxy for cost — one 10GB Parquet
//! file trips nothing while sixty small ones trip it — and we measure no bytes at all, so any
//! figure here would be a guess wearing a decimal point. What the copy states is the *shape* of
//! the work, which is true at every size: it reads everything once, distinct counts cannot be
//! merged so there is no cheaper form, and the answer is cached until the entry changes.
//!
//! ## Only a first scan asks
//!
//! Every trigger goes through [`ProfileActions::ask`], which raises this dialog when the entry
//! has no scan and starts one straight away when it already does — so the inspector's ↻ (and a
//! second press of a row's menu item) re-scan without a question, exactly as P3-10 specifies.
//! Confirming is [`ProfileActions::start`], the same call the ↻ makes: there is one path from
//! "profile this" to a request on the row, and this dialog is a gate in front of it rather than
//! a second copy of it.

use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::{use_radio_station, RadioStation};
use strata_model::{CatalogKind, ColRef};

use crate::apps::project::state::{
    use_catalog_selection, CatalogSelection, Chan, ProjChan, ProjectState, SessionState,
};
use crate::components::dialog::{Dialog, DialogHeader};
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Control, MonoValue, Prose, Title};

/// What a profile confirm is about — a table or a view, by **name**, which is their shared
/// engine/SQL identity. (A saved query is a stored string: there is nothing to scan.)
#[derive(Clone, PartialEq, Debug)]
pub struct ProfileTarget {
    pub kind: CatalogKind,
    pub name: String,
}

impl ProfileTarget {
    /// The action, used for the title and the row menus' item alike.
    pub fn verb(kind: CatalogKind) -> &'static str {
        match kind {
            CatalogKind::View => "Profile view",
            _ => "Profile table",
        }
    }

    /// What a scan does, in the terms that hold at any size. The two differ in what "reads
    /// everything" means: a table's files, or a view's whole query — joins, aggregates and all.
    fn body(&self) -> &'static str {
        match self.kind {
            CatalogKind::View => {
                "Runs the view's query in full to compute distinct counts, minimums, maximums, \
                 means and medians. Distinct counts cannot be merged, so there is no cheaper \
                 form. The result is cached until the view is redefined or a table it reads \
                 changes."
            }
            _ => {
                "Reads every file to compute distinct counts, minimums, maximums, means and \
                 medians. Distinct counts cannot be merged across files, so there is no cheaper \
                 form. The result is cached until the table changes."
            }
        }
    }
}

/// The handles a profile action writes through, gathered once per surface.
///
/// Stations, not subscribing radios: asking for a scan is something a *handler* does, and a
/// menu item or an inspector button has no business re-rendering because a registration landed
/// in some other row.
#[derive(Clone, Copy)]
pub struct ProfileActions {
    project: RadioStation<ProjectState, ProjChan>,
    session: RadioStation<SessionState, Chan>,
    selection: CatalogSelection,
    /// The confirm slot this dialog watches. Setting it *is* asking the question.
    target: State<Option<ProfileTarget>>,
}

/// Gather the profile action handles from this window's stores + context.
pub fn use_profile_actions() -> ProfileActions {
    ProfileActions {
        project: use_radio_station::<ProjectState, ProjChan>(),
        session: use_radio_station::<SessionState, Chan>(),
        selection: use_catalog_selection(),
        target: use_consume::<State<Option<ProfileTarget>>>(),
    }
}

/// Does asking for a scan of this entry have to go through the confirm?
///
/// **Only a first scan asks** (P3-10). An entry that already carries a request is being
/// re-scanned — the user has seen this question, agreed to it, and is now asking for the same
/// work again — so a second dialog would be noise. Pure over the store, so the rule is testable
/// without a window and there is one copy of it.
pub fn needs_confirm(project: &ProjectState, kind: CatalogKind, name: &str) -> bool {
    project.profile_scan(kind, name).is_none()
}

impl ProfileActions {
    /// **Profile this entry** — the one entry point every trigger uses. A first scan raises the
    /// confirm; a re-scan goes straight through ([`needs_confirm`]).
    pub fn ask(&self, kind: CatalogKind, name: &str) {
        if !needs_confirm(&self.project.peek(), kind, name) {
            self.start(kind, name);
            return;
        }
        let mut target = self.target;
        target.set(Some(ProfileTarget {
            kind,
            name: name.to_string(),
        }));
    }

    /// Ask for the scan: a fresh request on the row, which is what the zone subscribes to (and
    /// what supersedes any scan already running, engine-side).
    ///
    /// The channel is the entry's own section, so a table's scan never wakes the views.
    pub fn start(&self, kind: CatalogKind, name: &str) {
        let mut project = self.project;
        let channel = match kind {
            CatalogKind::View => ProjChan::Views,
            _ => ProjChan::Tables,
        };
        if project
            .write_channel(channel)
            .request_profile(kind, name)
            .is_none()
        {
            // The row went between the press and here — nothing to scan.
            return;
        }
        self.reveal(kind, name);
    }

    /// Drop the request: a cancel, and the honest state afterwards is the zone offering the scan
    /// again. The engine-side abort is the caller's (it holds the engine) — see the inspector's
    /// running state.
    pub fn clear(&self, kind: CatalogKind, name: &str) {
        let mut project = self.project;
        let channel = match kind {
            CatalogKind::View => ProjChan::Views,
            _ => ProjChan::Tables,
        };
        project.write_channel(channel).clear_profile(kind, name);
    }

    /// Put the scan where the user can see it happen.
    ///
    /// A scan started from a **catalog row** has no other feedback than the row's spinner, and
    /// its results land in a panel that may be collapsed or pointed at another table entirely.
    /// So the inspector is opened on this entry, standing on its first column — one scan covers
    /// every column, so any of them shows that it ran. A scan started from the inspector's own
    /// card is already looking at this entry, so nothing moves.
    ///
    /// An entry with no landed schema keeps the selection it had: there is no column to name
    /// yet, and inventing one would point the panel at nothing.
    fn reveal(&self, kind: CatalogKind, name: &str) {
        let mut selection = self.selection;
        let looking = selection
            .peek()
            .as_ref()
            .is_some_and(|c| c.kind == kind && ProjectState::same_name(&c.owner, name));
        if !looking {
            let Some(first) = self.first_column(kind, name) else {
                return;
            };
            selection.set(Some(ColRef {
                kind,
                owner: name.to_string(),
                path: vec![first],
            }));
        }
        // Guarded: a write notifies whether or not it changed anything, and there is no reason
        // to re-render the shell for a panel that is already open.
        let mut session = self.session;
        if !session.peek().layout.inspector_open {
            session.write_channel(Chan::Layout).open_inspector();
        }
    }

    /// The entry's first top-level column, if its schema has landed.
    fn first_column(&self, kind: CatalogKind, name: &str) -> Option<String> {
        let p = self.project.peek();
        let columns = match kind {
            CatalogKind::View => p
                .views
                .iter()
                .find(|v| ProjectState::same_name(&v.def.name, name))
                .and_then(|v| v.reg.ready())
                .map(|info| &info.columns),
            CatalogKind::Query => None,
            CatalogKind::Table => p
                .tables
                .iter()
                .find(|t| ProjectState::same_name(&t.def.name, name))
                .and_then(|t| t.reg.ready())
                .map(|meta| &meta.columns),
        }?;
        columns.first().map(|c| c.name.clone())
    }
}

/// Mounted at the window root beside the other dialogs, on the same terms: while open, its key
/// barrier precedes every feature listener in document order. Esc cancels, Enter runs the scan.
#[derive(PartialEq)]
pub struct ProfileConfirm {
    pub target: State<Option<ProfileTarget>>,
}

impl Component for ProfileConfirm {
    fn render(&self) -> impl IntoElement {
        let mut slot = self.target;
        let target = slot.read().clone();
        let actions = use_profile_actions();
        let theme = use_theme();

        // Shared by the button and the Enter key, so it holds only `Copy` handles and shadows
        // `slot` inside — a closure that captured the outer `mut` binding would be `FnMut`, and
        // the two handlers can't both take it.
        let confirm = move |()| {
            let mut slot = slot;
            if let Some(target) = slot.peek().clone() {
                actions.start(target.kind, &target.name);
            }
            slot.set(None);
        };

        let Some(target) = target else {
            return rect().into_element();
        };

        let c = theme.read().colors().clone();
        // The action over its subject — the close and drop confirms' shape exactly: the name is
        // mono on its own line, where it reads as the identifier it is.
        let title = rect()
            .width(Size::fill())
            .vertical()
            .child(Title::new(ProfileTarget::verb(target.kind)).color(c.text_primary))
            .child(
                MonoValue::new(target.name.clone())
                    .color(c.primary)
                    .text_overflow(TextOverflow::Ellipsis),
            );

        Dialog::new()
            .on_dismiss(move |_| slot.set(None))
            .on_confirm(confirm)
            // Warning-toned, like the canvas: this is a question about work the user is about to
            // pay for, not a destructive one.
            .header(DialogHeader::new(IconName::Warning, c.warning, title))
            .body(Prose::new(target.body()).color(c.text_secondary).wrap())
            .action(
                Button::new()
                    .flat()
                    .on_press(move |_| slot.set(None))
                    .child(Control::new("Cancel")),
            )
            .action(
                Button::new()
                    // The stock filled dress — accent over inverse text, like the scan card's
                    // own button and the Run control.
                    .filled()
                    .on_press(move |_| confirm(()))
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .child(Icon::new(IconName::Chart).size(13.))
                            .child(Control::new("Run scan")),
                    ),
            )
            .into_element()
    }
}

/// Profile-confirm tests — the dialog driven the way the user drives it. Nothing here touches
/// the engine: the deliverable is the gate (what it says, and that confirming leaves a request
/// on the row), while the scan behind that request is the engine's own tested contract.
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use freya_testing::TestingRunner;
    use strata_core::engine::{TableMeta, ViewMeta};
    use strata_core::project::ProjectDefs;
    use strata_core::theme::load;
    use strata_model::{ColumnInfo, Kind, TableDef, ViewDef};

    use super::*;
    use crate::theme::strata_theme;
    use strata_model::SourceFormat;

    fn col(name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            dtype: "Utf8".into(),
            kind: Kind::Str,
            nullable: true,
            children: Vec::new(),
            stats: Vec::new(),
        }
    }

    /// One registered table and one registered view — the two things that can be profiled.
    fn project() -> ProjectState {
        let defs = ProjectDefs {
            name: "test".into(),
            tables: vec![TableDef {
                name: "events".into(),
                format: SourceFormat::Parquet,
                sources: vec!["events.parquet".into()],
                partition_cols: Vec::new(),
            }],
            views: vec![ViewDef {
                name: "daily".into(),
                sql: "SELECT * FROM events".into(),
            }],
            saved_queries: Vec::new(),
        };
        let mut p =
            ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-profile-confirm-test"));
        p.table_registered(
            "events",
            TableMeta {
                columns: vec![col("amount"), col("region")],
                rows: Some(10),
            },
        );
        p.view_registered(
            "daily",
            ViewMeta {
                columns: vec![col("day")],
                tables: vec!["events".into()],
                aliases: Vec::new(),
            },
        );
        p
    }

    fn app() -> impl IntoElement {
        use_init_theme(|| strata_theme(&load("midnight")));
        let target = use_consume::<State<Option<ProfileTarget>>>();
        rect().expanded().child(ProfileConfirm { target })
    }

    type Handles = (
        State<Option<ProfileTarget>>,
        RadioStation<ProjectState, ProjChan>,
        RadioStation<SessionState, Chan>,
        CatalogSelection,
    );

    fn runner() -> (TestingRunner, Handles) {
        TestingRunner::new(
            app,
            (900., 700.).into(),
            |r| {
                let target = r.provide_root_context(|| State::create(None::<ProfileTarget>));
                let project = r.provide_root_context(|| {
                    RadioStation::<ProjectState, ProjChan>::create(project())
                });
                let session = r.provide_root_context(|| {
                    RadioStation::<SessionState, Chan>::create(SessionState::default())
                });
                let selection = r.provide_root_context(|| State::create(None::<ColRef>));
                (target, project, session, selection)
            },
            1.,
        )
    }

    fn open(
        runner: &mut TestingRunner,
        slot: &mut State<Option<ProfileTarget>>,
        kind: CatalogKind,
        name: &str,
    ) {
        runner.sync_and_update();
        slot.set(Some(ProfileTarget {
            kind,
            name: name.to_string(),
        }));
        runner.sync_and_update();
        runner.sync_and_update();
    }

    fn texts(runner: &TestingRunner) -> Vec<String> {
        runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
    }

    /// Press the action-strip button labelled `text` — the **lowest** match, since the header's
    /// title can carry the same words as the button that confirms it.
    fn click_action(runner: &mut TestingRunner, text: &str) {
        let area = runner
            .find_many(|node, element| {
                Label::try_downcast(element)
                    .filter(|l| l.text == text)
                    .map(|_| node.layout().area)
            })
            .into_iter()
            .max_by(|a, b| a.min_y().total_cmp(&b.min_y()))
            .unwrap_or_else(|| panic!("no text run {text:?} in the tree"));
        let point = (
            (area.min_x() + area.width() / 2.) as f64,
            (area.min_y() + area.height() / 2.) as f64,
        );
        runner.move_cursor(point);
        runner.click_cursor(point);
        runner.sync_and_update();
        runner.sync_and_update();
    }

    /// The headline: the confirm describes the **work**, and quotes no arithmetic. The three
    /// claims it does make are the ones true at any size (DEV_TASKS U15).
    #[test]
    fn the_confirm_describes_the_work_and_quotes_no_figures() {
        let (mut runner, (mut slot, ..)) = runner();
        open(&mut runner, &mut slot, CatalogKind::Table, "events");

        let texts = texts(&runner);
        assert_eq!(texts[0], "Profile table");
        assert_eq!(texts[1], "events");
        let body = texts
            .iter()
            .find(|t| t.contains("distinct counts"))
            .expect("the body copy");
        assert!(body.contains("Reads every file"));
        assert!(body.contains("cannot be merged across files"));
        assert!(body.contains("cached until the table changes"));
        assert!(
            !body.chars().any(|c| c.is_ascii_digit()),
            "no file counts, no row counts, no sizes: {body}"
        );
        assert!(texts.iter().any(|t| t == "Run scan"));
    }

    /// **Only a first scan asks.** A row that already carries a request is being re-scanned, and
    /// the ↻ that asks for it has no business raising the same question twice.
    #[test]
    fn a_re_scan_needs_no_confirm() {
        let mut p = project();
        assert!(needs_confirm(&p, CatalogKind::Table, "events"));

        p.request_profile(CatalogKind::Table, "events");
        assert!(!needs_confirm(&p, CatalogKind::Table, "events"));

        // Invalidation puts the question back: the numbers are gone, so this is a first scan
        // again.
        p.table_registered(
            "events",
            TableMeta {
                columns: vec![col("amount")],
                rows: Some(11),
            },
        );
        assert!(needs_confirm(&p, CatalogKind::Table, "events"));
    }

    /// A **view** says what a scan of a view costs — its whole query, not a file read — and what
    /// invalidates it, which for a view includes the tables underneath it (D10).
    #[test]
    fn a_view_confirm_names_the_query_rather_than_files() {
        let (mut runner, (mut slot, ..)) = runner();
        open(&mut runner, &mut slot, CatalogKind::View, "daily");

        let texts = texts(&runner);
        assert_eq!(texts[0], "Profile view");
        let body = texts
            .iter()
            .find(|t| t.contains("distinct counts"))
            .expect("the body copy");
        assert!(body.contains("Runs the view's query in full"));
        assert!(body.contains("a table it reads changes"));
    }

    /// Confirming leaves a scan request on the row — the thing the inspector subscribes to — and
    /// closes the dialog. It also reveals the entry, since a scan asked for from a catalog row
    /// would otherwise run out of sight.
    #[test]
    fn running_the_scan_records_the_request_and_reveals_the_entry() {
        let (mut runner, (mut slot, project, mut session, selection)) = runner();
        // The layout starts with the inspector open, so close it — a scan asked for from a
        // catalog row has to be able to *reveal* the panel, not merely find it up.
        session.write_channel(Chan::Layout).close_inspector();
        open(&mut runner, &mut slot, CatalogKind::Table, "events");

        click_action(&mut runner, "Run scan");

        assert!(
            project
                .peek()
                .profile_scan(CatalogKind::Table, "events")
                .is_some(),
            "the row now carries the request the zone subscribes to"
        );
        assert!(slot.peek().is_none(), "the dialog closed itself");
        assert_eq!(
            selection.peek().as_ref().map(|c| c.path.clone()),
            Some(vec!["amount".to_string()]),
            "…standing on the entry's first column"
        );
        assert!(
            session.peek().layout.inspector_open,
            "…in an open inspector"
        );
    }

    /// Cancelling is a true no-op: no request, nothing scanned, nothing revealed.
    #[test]
    fn cancelling_asks_for_nothing() {
        let (mut runner, (mut slot, project, _, selection)) = runner();
        open(&mut runner, &mut slot, CatalogKind::Table, "events");

        click_action(&mut runner, "Cancel");

        assert_eq!(
            project.peek().profile_scan(CatalogKind::Table, "events"),
            None
        );
        assert!(selection.peek().is_none());
        assert!(slot.peek().is_none());
    }

    /// Headless preview for eyeballing against the canvas's `profile` tile. Ignored by default
    /// (it writes a file, asserts nothing):
    /// `cargo test -p strata-freya profile_confirm_preview -- --ignored`.
    #[test]
    #[ignore = "writes target/profile-confirm-preview.png for eyeballing; run explicitly"]
    fn profile_confirm_preview() {
        let (mut runner, (mut slot, ..)) = runner();
        open(&mut runner, &mut slot, CatalogKind::Table, "events");
        runner.render_to_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/profile-confirm-preview.png"
        ));
    }

    /// Esc cancels and Enter runs — the dialog's own barrier, and a *different* closure from the
    /// button's, so without this the two could be swapped and the suite would stay green.
    #[test]
    fn escape_cancels_and_enter_runs_the_scan() {
        let (mut runner, (mut slot, project, ..)) = runner();
        open(&mut runner, &mut slot, CatalogKind::Table, "events");

        runner.press_key(Key::Named(NamedKey::Escape));
        runner.sync_and_update();
        assert_eq!(
            project.peek().profile_scan(CatalogKind::Table, "events"),
            None
        );
        assert!(slot.peek().is_none());

        open(&mut runner, &mut slot, CatalogKind::Table, "events");
        runner.press_key(Key::Named(NamedKey::Enter));
        runner.sync_and_update();
        assert!(project
            .peek()
            .profile_scan(CatalogKind::Table, "events")
            .is_some());
        assert!(slot.peek().is_none());
    }
}
