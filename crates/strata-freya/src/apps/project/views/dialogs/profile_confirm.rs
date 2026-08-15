//! The **profile cost confirm** (P3-10 · U15), built to the Strata canvas's `profile` comp: a
//! warning chip beside the action over its subject, the copy that says what a scan actually
//! does, and Cancel over an accent **Run scan**.
//!
//! ## It names the work, not a number
//!
//! The canvas quotes cost figures off a `>50 files` gate ("248 files · ~186 MB"). Deliberately
//! not built (`DEV_TASKS` D4/U15): file count is a backwards proxy for cost — one 10GB Parquet
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
//!
//! ## The target says where the request is kept
//!
//! [`ProfileTarget`] has two arms and every action here takes one, because a relation inside a
//! database connection's catalog has no `ProjectState` row to record a request on — a database
//! answers for itself, so there are no defs under it (DB-02). The rule that the store holds the
//! request generalizes rather than being excepted: *whoever owns the surface holds it*, which for
//! a remote relation is the window (`state::catalog`'s `RemoteScans`). Nothing is minted into the
//! store, and the numbers still live only in the freya-query entry the `ScanId` keys.

use freya::prelude::*;
use freya::radio::{use_radio_station, RadioStation};
use strata_model::{CatalogKind, ColOwner, ColRef, RightPane};

use crate::apps::project::query::{ProfileTarget, ScanId};
use crate::apps::project::state::{
    use_catalog_selection, use_remote_scans, CatalogSelection, Chan, ProjChan, ProjectState,
    RemoteScans, SessionState,
};
use crate::components::dialog::{Dialog, DialogHeader};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::SP_3;
use crate::components::tones::tones;
use crate::components::typography::{Control, MonoValue, Prose, Title};
use crate::theme::{use_roles, Role};

/// The confirm's own reading of a [`ProfileTarget`]: what the action is called, and what the work
/// actually is. (A saved query is a stored string: there is nothing to scan.)
///
/// The identity lives with the cache key it *is* (`query::profile`); the words live here, beside
/// the dialog that says them.
trait ProfileWording {
    fn verb(&self) -> &'static str;
    fn body(&self) -> &'static str;
}

/// The action, from a workspace entry's kind alone — what a row menu's item is labelled before
/// there is any target to speak of.
pub fn profile_verb(kind: CatalogKind) -> &'static str {
    match kind {
        CatalogKind::View => "Profile view",
        _ => "Profile table",
    }
}

impl ProfileWording for ProfileTarget {
    fn verb(&self) -> &'static str {
        profile_verb(self.kind())
    }

    /// What a scan does, in the terms that hold at any size — and **where it happens**, which is
    /// the third arm's whole point: a remote scan spends the server's time rather than this
    /// machine's, and it computes a smaller set for it (`profile::Profiled`), so a confirm that
    /// promised medians would be promising what the numbers then do not show.
    fn body(&self) -> &'static str {
        match self {
            ProfileTarget::Workspace {
                kind: CatalogKind::View,
                ..
            } => {
                "Runs the view's query in full to compute distinct counts, minimums, maximums, \
                 means and medians. Distinct counts cannot be merged, so there is no cheaper \
                 form. The result is cached until the view is redefined or a table it reads \
                 changes."
            }
            ProfileTarget::Workspace { .. } => {
                "Reads every file to compute distinct counts, minimums, maximums, means and \
                 medians. Distinct counts cannot be merged across files, so there is no cheaper \
                 form. The result is cached until the table changes."
            }
            ProfileTarget::Remote { .. } => {
                "Runs one statement on the database that reads every row, to compute distinct \
                 counts, minimums, maximums and means. Distinct counts cannot be merged, so \
                 there is no cheaper form, and the server does the work. The result is cached \
                 until the connection is refreshed."
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
    /// Where a **remote** relation's request is kept — see [`RemoteScans`].
    remote: RemoteScans,
    /// The confirm slot this dialog watches. Setting it *is* asking the question.
    target: State<Option<ProfileTarget>>,
}

/// Gather the profile action handles from this window's stores + context.
pub fn use_profile_actions() -> ProfileActions {
    ProfileActions {
        project: use_radio_station::<ProjectState, ProjChan>(),
        session: use_radio_station::<SessionState, Chan>(),
        selection: use_catalog_selection(),
        remote: use_remote_scans(),
        target: use_consume::<State<Option<ProfileTarget>>>(),
    }
}

/// Does asking for a scan of this entry have to go through the confirm?
///
/// **Only a first scan asks** (P3-10). An entry that already carries a request is being
/// re-scanned — the user has seen this question, agreed to it, and is now asking for the same
/// work again — so a second dialog would be noise. Pure over the request, so the rule is one
/// sentence whichever storage the request came out of.
pub fn needs_confirm(scan: Option<ScanId>) -> bool {
    scan.is_none()
}

impl ProfileActions {
    /// The scan asked for on `target`, **from whichever storage backs it** — the workspace entry's
    /// own catalog row, or the window's satellite for a relation that has no row.
    ///
    /// One reader, so every caller of [`ask`](Self::ask), [`start`](Self::start) and the zone that
    /// renders the result are looking at the same slot.
    pub fn scan(&self, target: &ProfileTarget) -> Option<ScanId> {
        match target {
            ProfileTarget::Workspace { kind, name } => {
                self.project.peek().profile_scan(*kind, name)
            }
            ProfileTarget::Remote { relation, .. } => self.remote.peek().get(relation).copied(),
        }
    }

    /// **Profile this entry** — the one entry point every trigger uses. A first scan raises the
    /// confirm; a re-scan goes straight through ([`needs_confirm`]).
    pub fn ask(&self, target: &ProfileTarget) {
        if !needs_confirm(self.scan(target)) {
            self.start(target);
            return;
        }
        let mut slot = self.target;
        slot.set(Some(target.clone()));
    }

    /// Ask for the scan: a fresh request in the target's own storage, which is what the zone
    /// subscribes to (and what supersedes any scan already running, engine-side).
    ///
    /// **The one `None` this can answer is a workspace row that is no longer there** — an entry
    /// the user cannot be looking at any more, which the inspector reports as gone on the same
    /// pass. It is not the bail this generalization exists to remove: before it, a remote target
    /// fell through the workspace lookup and left a confirmed, agreed-to scan starting *nothing*,
    /// with the panel still offering it. A relation now has somewhere to record the request, so
    /// that arm cannot be reached at all.
    ///
    /// A workspace request lands on the entry's own section channel, so a table's scan never wakes
    /// the views.
    pub fn start(&self, target: &ProfileTarget) -> Option<ScanId> {
        let scan = match target {
            ProfileTarget::Workspace { kind, name } => {
                let mut project = self.project;
                project
                    .write_channel(section(*kind))
                    .request_profile(*kind, name)?
            }
            ProfileTarget::Remote { relation, .. } => {
                let scan = ScanId::new();
                let mut remote = self.remote;
                remote.write().insert(relation.clone(), scan);
                scan
            }
        };
        self.reveal(target);
        Some(scan)
    }

    /// Drop the request: a cancel, and the honest state afterwards is the zone offering the scan
    /// again. The engine-side abort is the caller's (it holds the engine) — see the inspector's
    /// running state.
    pub fn clear(&self, target: &ProfileTarget) {
        match target {
            ProfileTarget::Workspace { kind, name } => {
                let mut project = self.project;
                project
                    .write_channel(section(*kind))
                    .clear_profile(*kind, name);
            }
            ProfileTarget::Remote { relation, .. } => {
                let mut remote = self.remote;
                remote.write().remove(relation);
            }
        }
    }

    /// Put the scan where the user can see it happen.
    ///
    /// A scan started from a **catalog row** has no other feedback than the row's spinner, and
    /// its results land in a panel that may be collapsed or pointed at another table entirely.
    /// So the inspector is opened on this entry, standing on its first column — one scan covers
    /// every column, so any of them shows that it ran. A scan started from the inspector's own
    /// card is already looking at this entry, so nothing moves.
    ///
    /// **Where the first column is not known here, the selection is the owner itself** and the
    /// panel stands it on the first column once it has one. A workspace entry whose registration
    /// has landed can be named directly; a remote relation's columns are an introspection this
    /// window may not have made yet, and a workspace entry with no landed schema has no column to
    /// name either — so both take the empty path rather than inventing one or leaving the panel
    /// pointed somewhere else.
    fn reveal(&self, target: &ProfileTarget) {
        let mut selection = self.selection;
        let owner = owner_of(target);
        let looking = selection
            .peek()
            .as_ref()
            .is_some_and(|c| same_owner(&c.owner, &owner));
        if !looking {
            selection.set(Some(ColRef {
                path: self.first_column(target).into_iter().collect(),
                owner,
            }));
        }
        let mut session = self.session;
        if session.peek().layout.right != Some(RightPane::Inspector) {
            session
                .write_channel(Chan::Layout)
                .open_right_pane(RightPane::Inspector);
        }
    }

    /// The entry's first top-level column, where this window already knows it.
    fn first_column(&self, target: &ProfileTarget) -> Option<String> {
        let ProfileTarget::Workspace { kind, name } = target else {
            return None;
        };
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

/// Which catalog section a workspace request lands on.
fn section(kind: CatalogKind) -> ProjChan {
    match kind {
        CatalogKind::View => ProjChan::Views,
        _ => ProjChan::Tables,
    }
}

/// The selection owner a profile target is about.
fn owner_of(target: &ProfileTarget) -> ColOwner {
    match target {
        ProfileTarget::Workspace { kind, name } => ColOwner::Entry {
            kind: *kind,
            name: name.clone(),
        },
        ProfileTarget::Remote { relation, .. } => ColOwner::Remote(relation.clone()),
    }
}

/// Are these the same owner? A workspace name is compared the way the engine resolves it
/// ([`ProjectState::same_name`]) — a remote relation is already the server's own spelling, and is
/// compared as it stands.
fn same_owner(a: &ColOwner, b: &ColOwner) -> bool {
    match (a, b) {
        (
            ColOwner::Entry { kind, name },
            ColOwner::Entry {
                kind: other_kind,
                name: other,
            },
        ) => kind == other_kind && ProjectState::same_name(name, other),
        (ColOwner::Remote(one), ColOwner::Remote(two)) => one == two,
        _ => false,
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
        let roles = use_roles();
        let warning = tones().warning;

        let confirm = move |()| {
            let mut slot = slot;
            if let Some(target) = slot.peek().clone() {
                actions.start(&target);
            }
            slot.set(None);
        };

        let Some(target) = target else {
            return rect().into_element();
        };

        let title = rect()
            .width(Size::fill())
            .vertical()
            .child(Title::new(target.verb()).color(roles.get(Role::Text)))
            .child(
                MonoValue::new(target.label())
                    .color(roles.get(Role::Accent))
                    .text_overflow(TextOverflow::Ellipsis),
            );

        Dialog::new()
            .on_dismiss(move |()| slot.set(None))
            .on_confirm(confirm)
            .header(DialogHeader::new(IconName::Warning, warning, title))
            .body(
                Prose::new(target.body())
                    .color(roles.get(Role::TextMuted))
                    .wrap(),
            )
            .action(
                Button::new()
                    .flat()
                    .on_press(move |_| slot.set(None))
                    .child(Control::new("Cancel")),
            )
            .action(
                Button::new().filled().on_press(move |_| confirm(())).child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(SP_3)
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
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use datafusion::arrow::datatypes::{DataType, Field};
    use freya_testing::TestingRunner;
    use strata_core::engine::column_info;
    use strata_core::engine::{TableMeta, ViewMeta};
    use strata_core::project::ProjectDefs;
    use strata_core::theme::load;
    use strata_model::{ColumnInfo, RemoteRef, TableDef, TableOrigin, ViewDef};

    use super::*;
    use crate::theme::strata_theme;
    use strata_model::SourceFormat;

    fn col(name: &str) -> ColumnInfo {
        column_info(&Field::new(name, DataType::Utf8, true))
    }

    /// One registered table and one registered view — the two things that can be profiled.
    fn project() -> ProjectState {
        let defs = ProjectDefs {
            name: "test".into(),
            tables: vec![TableDef {
                name: "events".into(),
                format: SourceFormat::Parquet,
                connection: None,
                sources: vec!["events.parquet".into()],
                partition_cols: Vec::new(),
                origin: TableOrigin::External,
            }],
            views: vec![ViewDef {
                name: "daily".into(),
                sql: "SELECT * FROM events".into(),
            }],
            saved_queries: Vec::new(),
            ..Default::default()
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
                remote: Vec::new(),
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
        RemoteScans,
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
                let remote = r.provide_root_context(|| State::create(BTreeMap::new()));
                (target, project, session, selection, remote)
            },
            1.,
        )
    }

    fn workspace(kind: CatalogKind, name: &str) -> ProfileTarget {
        ProfileTarget::Workspace {
            kind,
            name: name.to_string(),
        }
    }

    /// `pg.public.orders`, as the tree composes one.
    fn remote_target(kind: CatalogKind, relation: &str) -> ProfileTarget {
        ProfileTarget::Remote {
            kind,
            relation: RemoteRef {
                connection: "pg".into(),
                schema: "public".into(),
                relation: relation.into(),
            },
        }
    }

    fn open(
        runner: &mut TestingRunner,
        slot: &mut State<Option<ProfileTarget>>,
        target: ProfileTarget,
    ) {
        runner.sync_and_update();
        slot.set(Some(target));
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
    /// claims it does make are the ones true at any size (`DEV_TASKS` U15).
    #[test]
    fn the_confirm_describes_the_work_and_quotes_no_figures() {
        let (mut runner, (mut slot, ..)) = runner();
        open(
            &mut runner,
            &mut slot,
            workspace(CatalogKind::Table, "events"),
        );

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
        let scan = |p: &ProjectState| p.profile_scan(CatalogKind::Table, "events");
        assert!(needs_confirm(scan(&p)));

        p.request_profile(CatalogKind::Table, "events");
        assert!(!needs_confirm(scan(&p)));

        p.table_registered(
            "events",
            TableMeta {
                columns: vec![col("amount")],
                rows: Some(11),
            },
        );
        assert!(needs_confirm(scan(&p)));
    }

    /// A **view** says what a scan of a view costs — its whole query, not a file read — and what
    /// invalidates it, which for a view includes the tables underneath it (D10).
    #[test]
    fn a_view_confirm_names_the_query_rather_than_files() {
        let (mut runner, (mut slot, ..)) = runner();
        open(
            &mut runner,
            &mut slot,
            workspace(CatalogKind::View, "daily"),
        );

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
        let (mut runner, (mut slot, project, mut session, selection, _)) = runner();
        session.write_channel(Chan::Layout).close_right_pane();
        open(
            &mut runner,
            &mut slot,
            workspace(CatalogKind::Table, "events"),
        );

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
            session.peek().layout.right == Some(RightPane::Inspector),
            "…in an open inspector"
        );
    }

    /// Cancelling is a true no-op: no request, nothing scanned, nothing revealed.
    #[test]
    fn cancelling_asks_for_nothing() {
        let (mut runner, (mut slot, project, _, selection, _)) = runner();
        open(
            &mut runner,
            &mut slot,
            workspace(CatalogKind::Table, "events"),
        );

        click_action(&mut runner, "Cancel");

        assert_eq!(
            project.peek().profile_scan(CatalogKind::Table, "events"),
            None
        );
        assert!(selection.peek().is_none());
        assert!(slot.peek().is_none());
    }

    /// **The regression this generalization exists to prevent.** A remote relation has no
    /// `ProjectState` row, and before the target had two arms the confirmed scan of one fell
    /// through the workspace lookup and started *nothing* — a cost the user had read, agreed to,
    /// and been given no work for, with the panel still offering the scan. The request now lands
    /// in the window's satellite, and the reveal points the panel at the relation.
    #[test]
    fn confirming_a_remote_scan_records_the_request_it_promised() {
        let (mut runner, (mut slot, project, _, selection, remote)) = runner();
        let target = remote_target(CatalogKind::Table, "orders");
        open(&mut runner, &mut slot, target.clone());

        let texts = texts(&runner);
        assert_eq!(texts[0], "Profile table");
        assert_eq!(texts[1], "pg.public.orders", "named as SQL addresses it");
        let body = texts
            .iter()
            .find(|t| t.contains("distinct counts"))
            .expect("the body copy");
        assert!(
            body.contains("on the database"),
            "the copy says whose time this spends: {body}"
        );
        assert!(
            !body.contains("medians"),
            "…and does not promise a fact the remote set cannot compute: {body}"
        );

        click_action(&mut runner, "Run scan");

        let ProfileTarget::Remote { relation, .. } = &target else {
            unreachable!()
        };
        assert!(
            remote.peek().get(relation).is_some(),
            "the window holds the request the confirm agreed to"
        );
        assert!(slot.peek().is_none(), "the dialog closed itself");
        assert_eq!(
            selection.peek().as_ref().map(|c| c.owner.clone()),
            Some(ColOwner::Remote(relation.clone())),
            "…with the panel pointed at the relation"
        );
        assert!(
            selection.peek().as_ref().is_some_and(|c| c.path.is_empty()),
            "and standing on no column yet: only an introspection can name one"
        );
        assert_eq!(
            project.peek().tables.len(),
            1,
            "nothing was minted into the store"
        );
    }

    /// A **view** on a database says so, in the same words a workspace view does — one vocabulary
    /// for the action, whichever catalog the relation is in.
    #[test]
    fn a_remote_view_is_labelled_a_view() {
        let (mut runner, (mut slot, ..)) = runner();
        open(
            &mut runner,
            &mut slot,
            remote_target(CatalogKind::View, "big_orders"),
        );
        assert_eq!(texts(&runner)[0], "Profile view");
    }

    /// Headless preview for eyeballing against the canvas's `profile` tile. Ignored by default
    /// (it writes a file, asserts nothing):
    /// `cargo test -p strata-freya profile_confirm_preview -- --ignored`.
    #[test]
    #[ignore = "writes target/profile-confirm-preview.png for eyeballing; run explicitly"]
    fn profile_confirm_preview() {
        let (mut runner, (mut slot, ..)) = runner();
        open(
            &mut runner,
            &mut slot,
            workspace(CatalogKind::Table, "events"),
        );
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
        open(
            &mut runner,
            &mut slot,
            workspace(CatalogKind::Table, "events"),
        );

        runner.press_key(Key::Named(NamedKey::Escape));
        runner.sync_and_update();
        assert_eq!(
            project.peek().profile_scan(CatalogKind::Table, "events"),
            None
        );
        assert!(slot.peek().is_none());

        open(
            &mut runner,
            &mut slot,
            workspace(CatalogKind::Table, "events"),
        );
        runner.press_key(Key::Named(NamedKey::Enter));
        runner.sync_and_update();
        assert!(project
            .peek()
            .profile_scan(CatalogKind::Table, "events")
            .is_some());
        assert!(slot.peek().is_none());
    }
}
