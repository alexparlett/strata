//! The catalog **drop confirm** (P3-05 · D10), built to the Strata canvas's `remove` /
//! `remove-deps` comp: a trash chip beside the action over its subject ("Drop table" over
//! `events`), the what-this-does line, and, when the drop leaves something behind it, an amber
//! consequence callout naming the views — over Cancel + the destructive action. The card, the
//! action strip and the modal keys are the shared [`Dialog`]'s.
//!
//! **This is the whole drop flow, minus its trigger.** Confirming performs the drop: the
//! store's def goes (and is persisted), any tab bound to the row is unbound, and the engine
//! is told. What P3-06 adds is the entry point — a catalog row's context menu setting the
//! [`DropTarget`] slot this dialog watches — not a second copy of the mechanics.
//!
//! The Connections pane's **Forget** (W7) is the fourth target rather than a dialog of its own,
//! for the reason [`DropTarget`] states: destroying a project's work asks on one set of terms.
//!
//! ## What the consequence line claims
//!
//! *Left invalid*, never "will stop working". A dependent view captured its sources by `Arc`
//! when it was created and never re-resolves their names, so it keeps answering after the
//! drop and fails only on the next reload (verified against DataFusion 54 — DEV_TASKS
//! D10/D11). So the warning says exactly what the catalog row will say afterwards: these
//! views are flagged, and they will not survive a reopen.

use freya::components::{get_theme, ScrollView};
use freya::prelude::*;
use freya::radio::{use_radio, use_radio_station, RadioStation};
use strata_core::engine::drop_intent;
use strata_model::{CatalogKind, TableOrigin};
use uuid::Uuid;

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{
    catalog_settled, log_event, persisted_defs, use_catalog, use_report, Catalog, Chan, LogLevel,
    ProjChan, ProjectState, ReportCtx, SessionState,
};
use crate::apps::project::views::{CancelButtonThemePartial, CancelButtonThemePreference};
use crate::components::badge::Badge;
use crate::components::dialog::{Dialog, DialogHeader};
use crate::components::icon::{Icon, IconName};
use crate::components::tones::tones;
use crate::components::typography::{Caption, Control, MonoValue, Prose, Title};
use crate::theme::{use_roles, Role};

/// What a drop confirm is about. The variants mirror each section's identity rule: tables and
/// views are addressed by **name** (their engine/SQL identity, one shared namespace), a saved
/// query by its stable **id** — its name is only the label, carried so the dialog can show it —
/// and a connection by its [`ConnectionDef::url`], which is what the object-store registry keys
/// on and the only form that tells `s3://lake` from `gs://lake`.
///
/// **Forget is here rather than in a dialog of its own** because every path that destroys a
/// project's work asks on the same terms: one card, one pair of actions, one Esc/Enter barrier,
/// one event recorded afterwards. What varies per variant is the wording and what the confirm
/// then calls, which is exactly what these methods are.
///
/// [`ConnectionDef::url`]: strata_model::ConnectionDef::url
#[derive(Clone, PartialEq, Debug)]
pub enum DropTarget {
    /// A table, by name, **with what dropping it destroys** (ED-05): an internal table's data
    /// files go with the def, an external table's do not, and the difference is the one thing
    /// this card exists to tell the user. Carried from the row the gesture started on rather
    /// than looked up here, because by the time the copy is rendered the only ways to answer
    /// are a lookup that cannot fail or a default — and a default reads "the source files on
    /// disk are not deleted" at exactly the moment the action is destructive.
    Table {
        name: String,
        origin: TableOrigin,
    },
    View(String),
    Query {
        id: Uuid,
        name: String,
    },
    /// A connection, by its `ConnectionDef::url()` — `s3://acme-lake` (W7).
    Connection(String),
}

impl DropTarget {
    /// The row's name, as the title shows it.
    pub fn name(&self) -> &str {
        match self {
            DropTarget::View(name) | DropTarget::Connection(name) => name,
            DropTarget::Table { name, .. } | DropTarget::Query { name, .. } => name,
        }
    }

    /// Which catalog section it belongs to — what [`ProjectState::dependent_views`] dispatches
    /// on to pick the right dependency list — or `None` for a target that is not in the SQL
    /// namespace at all. A connection is an object store, so nothing can *read* it by name and
    /// there is no dependency list to ask for.
    fn kind(&self) -> Option<CatalogKind> {
        match self {
            DropTarget::Table { .. } => Some(CatalogKind::Table),
            DropTarget::View(_) => Some(CatalogKind::View),
            DropTarget::Query { .. } => Some(CatalogKind::Query),
            DropTarget::Connection(_) => None,
        }
    }

    /// The action, used for both the title and the button (canvas `removeTitle` / `removeBtn`
    /// are the same string). A saved query is *deleted*, not dropped — it was never registered
    /// with the engine — and a connection is *forgotten*, which is the spec's own word for it
    /// and the right one: nothing in the bucket changes.
    fn verb(&self) -> &'static str {
        match self {
            DropTarget::Table { .. } => "Drop table",
            DropTarget::View(_) => "Drop view",
            DropTarget::Query { .. } => "Delete query",
            DropTarget::Connection(_) => "Forget connection",
        }
    }

    /// What the drop actually does — the canvas's `removeBody`, each line chosen to answer the
    /// question the user is really asking (are my *files* gone? are the tables it read gone?).
    ///
    /// A table's line is the **engine's** ([`drop_intent`]), not a second wording beside it: the
    /// confirm is asking permission for exactly what the drop's report then describes, and the
    /// two sentences have to agree about whether the files go.
    fn body(&self) -> &'static str {
        match self {
            DropTarget::Table { origin, .. } => drop_intent(*origin),
            DropTarget::View(_) => {
                "Removes the view definition from the catalog. The tables it reads are not \
                 affected."
            }
            DropTarget::Query { .. } => {
                "Removes this saved query from the project. Any open tab keeps its SQL."
            }
            DropTarget::Connection(_) => {
                "Removes the object store from this project. Nothing in the bucket is deleted."
            }
        }
    }

    /// How the consequence line refers to it.
    fn noun(&self) -> &'static str {
        match self {
            DropTarget::Table { .. } => "table",
            DropTarget::View(_) => "view",
            DropTarget::Query { .. } => "query",
            DropTarget::Connection(_) => "connection",
        }
    }
}

/// The same action in the past tense, for the event log — the log records what happened, so it
/// cannot reuse [`DropTarget::verb`], which is worded as the thing you are about to do.
fn past(target: &DropTarget) -> &'static str {
    match target {
        DropTarget::Table { .. } => "Dropped table",
        DropTarget::View(_) => "Dropped view",
        DropTarget::Query { .. } => "Deleted query",
        DropTarget::Connection(_) => "Forgot connection",
    }
}

/// The consequence line (D10): how many views this drop leaves invalid, or `None` when it
/// leaves none — the callout is absent entirely rather than stating a zero.
///
/// Count first, names after (they follow as chips): a busy table can back dozens of views, and
/// the number is the part that scales.
fn consequence(count: usize, noun: &str) -> Option<String> {
    match count {
        0 => None,
        1 => Some(format!(
            "1 view reads this {noun} and will be left invalid:"
        )),
        n => Some(format!(
            "{n} views read this {noun} and will be left invalid:"
        )),
    }
}

/// Mounted at the window root beside [`CloseConfirm`](super::CloseConfirm), on the same terms:
/// while open, its key handler precedes every feature listener in document order and consumes
/// every press, so nothing underneath can act on a keystroke aimed at the dialog. Esc cancels,
/// Enter confirms.
#[derive(PartialEq)]
pub struct DropConfirm {
    pub target: State<Option<DropTarget>>,
}

impl Component for DropConfirm {
    fn render(&self) -> impl IntoElement {
        let mut slot = self.target;
        let target = slot.read().clone();
        let engine = use_consume::<EngineCtx>();
        // Two handles on the Project store, deliberately: `views` subscribes to the one channel
        // the consequence line is derived from, while the station is the unsubscribed write side.
        // `RadioStation::read()` would listen on *every* channel, so a catalog re-scan landing
        // under the open dialog would re-render it (and re-run the O(views × deps) scan) once per
        // table, to produce a pixel-identical card.
        let views = use_radio::<ProjectState, ProjChan>(ProjChan::Views);
        let project = use_radio_station::<ProjectState, ProjChan>();
        let session = use_radio_station::<SessionState, Chan>();
        // A drop moves the engine's catalog: every tab reading the dropped row is now wrong,
        // and none of them has been typed in. The bump is what re-derives them.
        let catalog = use_catalog();
        // A drop is a catalog mutation, so it is recorded in the event log (P3-13) — including
        // the one failure mode the catalog itself cannot show: a `DROP VIEW` the engine refused
        // after the def was already gone.
        let report = use_report();
        let tones = tones();
        let roles = use_roles();
        // The destructive action wears the shared `cancel_button` dress — the themes' authored
        // destructive tone (the running body's Cancel, the close confirm's Stop), not a
        // hardcoded red.
        let danger = get_theme!(
            &None::<CancelButtonThemePartial>,
            CancelButtonThemePreference,
            "cancel_button"
        );

        // Shared by the button and the Enter key. It takes the engine rather than capturing it,
        // so the closure holds only `Copy` handles and can live in both handlers; each of them
        // carries its own `Arc` clone.
        let confirm = move |engine: &EngineCtx| {
            let mut slot = slot;
            if let Some(target) = slot.peek().clone() {
                drop_row(engine, project, session, catalog, report, &target);
            }
            slot.set(None);
        };
        let key_engine = engine.clone();

        let Some(target) = target else {
            return rect().into_element();
        };

        // Subscribed to `ProjChan::Views` (see above), so a view registering or being dropped
        // under the open dialog refreshes the count — the dialog blocks input, not the engine,
        // and this is the one screen where acting on a stale count is destructive.
        let dependents = match target.kind() {
            Some(kind) => views.read().dependent_views(kind, target.name()),
            None => Vec::new(),
        };
        let consequence = consequence(dependents.len(), target.noun());

        // The action over its subject — the close confirm's shape exactly, which is what the comp
        // asks for. The name is mono at 12.5 on its own line, where it reads as the identifier it
        // is; inline beside a 14.5 title it just looked shrunken. The accent is what marks it out.
        let title = rect()
            .width(Size::fill())
            .vertical()
            .child(Title::new(target.verb()).color(roles.get(Role::Text)))
            .child(
                MonoValue::new(target.name().to_string())
                    .color(roles.get(Role::Accent))
                    .text_overflow(TextOverflow::Ellipsis),
            );

        let callout = consequence.map(|line| {
            rect()
                .width(Size::fill())
                // No margin of its own: the body column already spaces at 12, which is the
                // comp's `margin-top: var(--sp-4)` above the callout.
                .corner_radius(10.)
                .padding(12.)
                .background(tones.warning.with_a(23))
                .border(Border::new().width(1.).fill(tones.warning.with_a(82)))
                .horizontal()
                .content(Content::Flex)
                .spacing(8.)
                .child(
                    rect()
                        .margin((1., 0., 0., 0.))
                        .child(Icon::new(IconName::Warning).color(tones.warning).size(15.)),
                )
                .child(
                    rect()
                        .width(Size::flex(1.))
                        .vertical()
                        .spacing(8.)
                        .child(Caption::new(line).color(tones.warning).wrap())
                        // The names themselves, as chips. A tall list caps and scrolls rather
                        // than growing the card off the screen (the comp's 96px well).
                        .child(
                            ScrollView::new()
                                .height(Size::auto())
                                .max_height(Size::px(96.))
                                .child(
                                    rect()
                                        .width(Size::fill())
                                        .horizontal()
                                        .content(Content::wrap_spacing(4.))
                                        .spacing(4.)
                                        // `into_iter`: the names are owned and unused after
                                        // this, so the chips take them rather than cloning
                                        // every one a second time per render.
                                        .children(
                                            dependents
                                                .into_iter()
                                                .map(|name| Badge::value(name, tones.warning)),
                                        ),
                                ),
                        ),
                )
        });

        // Header · body · footer, like every other dialog: the title rides the chip, and the copy
        // and callout run the full width beneath rather than indented beside it.
        let body = rect()
            .width(Size::fill())
            .vertical()
            .spacing(12.)
            .child(
                Prose::new(target.body())
                    .color(roles.get(Role::TextMuted))
                    .wrap(),
            )
            .maybe_child(callout);

        Dialog::new()
            .on_dismiss(move |_| slot.set(None))
            .on_confirm(move |_| confirm(&key_engine))
            .header(DialogHeader::new(IconName::Trash, tones.error, title))
            .body(body)
            .action(
                Button::new()
                    .flat()
                    .on_press(move |_| slot.set(None))
                    .child(Control::new("Cancel")),
            )
            .action(
                Button::new()
                    .filled()
                    .theme_colors(
                        ButtonColorsThemePartial::default()
                            .background(danger.background)
                            .hover_background(danger.hover_background)
                            .border_fill(danger.border_fill)
                            .hover_border_fill(danger.border_fill)
                            .color(danger.color)
                            .hover_color(danger.color),
                    )
                    .on_press(move |_| confirm(&engine))
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .child(Icon::new(IconName::Trash).size(13.))
                            .child(Control::new(target.verb())),
                    ),
            )
            .into_element()
    }
}

/// Perform the confirmed drop.
///
/// **The store is the catalog**, so the def going is what the sidebar sees — there is nothing to
/// invalidate and nothing refetches (P3-02). Order follows `save_view`'s: mutate the def and
/// persist it first, then tell the engine. A table's dependent views turn invalid on the same
/// write, because their validity is derived from the live table rows (P3-04) and the VIEWS
/// section subscribes to [`ProjChan::Tables`] for exactly this.
///
/// ## A drop whose write fails is rolled back (P4-15 item 4)
///
/// Of every mutation that writes `project.json`, the drop is the only one whose silent failure
/// **resurrects** what the user destroyed: the row leaves the catalog, the file still lists it,
/// and it is back at the next open — a destructive action they deliberately confirmed, undone
/// later with nothing said.
///
/// It is also the only one that *can* be rolled back, which is what decides it. At the moment
/// the write fails nothing else has happened yet: every arm mutates the store and persists
/// inside one guard, and the engine and session calls all come after. So the section is
/// snapshotted, and a failed write puts it back **inside that same guard** — subscribers see one
/// notification carrying the original state, never a row that vanishes and returns.
///
/// The policy is therefore "roll back what can be rolled back", not "mutations are atomic".
/// `save_view` genuinely cannot: `CREATE OR REPLACE VIEW` has already succeeded on the engine, so
/// the view is live and queryable for the rest of the session and undoing it needs a second
/// fallible engine call. That asymmetry is deliberate — the two situations differ in what has
/// already become true — and the Problems drawer's `Project` scope carries the other half either
/// way, naming the file that is behind for as long as it is.
fn drop_row(
    engine: &EngineCtx,
    mut project: RadioStation<ProjectState, ProjChan>,
    mut session: RadioStation<SessionState, Chan>,
    catalog: Catalog,
    report: ReportCtx,
    target: &DropTarget,
) {
    // A removal is recorded at `Info`, not `Ok`: it is a change to the catalog, not a piece of
    // work that succeeded, and the green tick belongs to the latter. Recorded **after** the def
    // is written, and only if that write landed — `persisted` says so, and says so itself when it
    // did not. A drop the project file never heard about comes back on the next open, so logging
    // it first would leave the log contradicting the catalog.
    match target {
        DropTarget::Table { name, .. } => {
            let landed = {
                let mut p = project.write_channel(ProjChan::Tables);
                let taken = p.remove_table(name);
                let landed = persisted_defs(&p, report);
                if let (false, Some((at, row))) = (landed, taken) {
                    p.restore_table(at, row);
                }
                landed
            };
            if !landed {
                return;
            }
            let engine = engine.clone();
            let name = name.clone();
            // **`Engine::drop_table`, not `deregister`** (ED-05): a table Strata wrote owns a
            // directory under `.strata/tables/`, and forgetting the provider without deleting it
            // orphans that data forever — no def points at it and the `.strata` housekeeping only
            // sweeps `.tmp-` directories. The typed `DROP TABLE` goes through the same call, so
            // the two gestures cannot leave different states behind.
            //
            // `spawn_forever` rather than `spawn`, for the reason the view arm below gives: the
            // engine call has to outlive the dialog that ordered it.
            spawn_forever(async move {
                // `if_exists`: the row came out of the store, and a def whose registration failed
                // has no provider to deregister — which is a drop that has nothing left to do,
                // not a drop that failed.
                if let Err(e) = engine.drop_table(name.clone(), true).await {
                    // The def is already gone, which is the catalog's truth; what may be left is
                    // a stale registration or, on an internal table, its data directory. The log
                    // is the only surface that can say so — the row it would describe is gone.
                    tracing::error!("drop table '{name}': {e}");
                    log_event(
                        report.log,
                        LogLevel::Warning,
                        format!("The engine could not finish dropping table '{name}': {e}"),
                    );
                }
                // Every tab that reads it is now wrong, and none of them has been typed in.
                catalog_settled(catalog);
            });
        }
        DropTarget::View(name) => {
            let landed = {
                let mut p = project.write_channel(ProjChan::Views);
                let taken = p.remove_view(name);
                let landed = persisted_defs(&p, report);
                if let (false, Some((at, row))) = (landed, taken) {
                    p.restore_view(at, row);
                }
                landed
            };
            if !landed {
                return;
            }
            if session.peek().is_bound_to_view(name) {
                session.write_channel(Chan::Tabs).unbind_view(name);
            }
            let engine = engine.clone();
            let name = name.clone();
            // **`spawn_forever`, not `spawn`.** `spawn` binds the task to `current_scope_id()`,
            // which during an event is the scope of the element that owns the handler — a
            // `Button` inside the dialog. Confirming closes the dialog in the same tick, that
            // subtree unmounts, and scope teardown drops its tasks *before the future is ever
            // polled*: the def would go, the file would be written, and DataFusion would never
            // hear about it. Verified with a probe — the task ran 0 times. The engine call has
            // to outlive the dialog that ordered it, so it belongs to the root.
            spawn_forever(async move {
                if let Err(e) = engine.drop_view(name.clone()).await {
                    // The def is already gone, which is the catalog's truth; a failed DROP VIEW
                    // leaves a stale registration the next re-scan clears. The log is the only
                    // surface that can say so — the row it would have described is gone.
                    tracing::error!("drop view '{name}': {e}");
                    // Only the part the row above doesn't already say — it named the drop.
                    log_event(
                        report.log,
                        LogLevel::Warning,
                        format!("The engine kept view '{name}' until the next re-scan: {e}"),
                    );
                }
                catalog_settled(catalog);
            });
        }
        DropTarget::Connection(url) => {
            let landed = {
                let mut p = project.write_channel(ProjChan::Connections);
                let taken = p.remove_connection(url);
                let landed = persisted_defs(&p, report);
                if let (false, Some((at, row))) = (landed, taken) {
                    p.restore_connection(at, row);
                }
                landed
            };
            if !landed {
                return;
            }
            // Synchronous and local, like a table's `deregister`: DataFusion drops the entry
            // from its object-store registry. Without it the bucket stays queryable for the
            // life of the window — `register_pass` is additive by contract, so nothing else
            // would ever take the store back out.
            engine.disconnect(url);
        }
        DropTarget::Query { id, .. } => {
            // Never registered with the engine — a saved query is a stored string.
            let landed = {
                let mut p = project.write_channel(ProjChan::Queries);
                let taken = p.remove_saved_query(*id);
                let landed = persisted_defs(&p, report);
                if let (false, Some((at, query))) = (landed, taken) {
                    p.restore_saved_query(at, query);
                }
                landed
            };
            if !landed {
                return;
            }
            if session.peek().is_bound_to_query(*id) {
                session.write_channel(Chan::Tabs).unbind_saved_query(*id);
            }
        }
    }
    // Unconditional: every arm above returns early when its write did not land, so reaching here
    // means the def is gone *and* `project.json` says so.
    log_event(
        report.log,
        LogLevel::Info,
        format!("{} '{}'", past(target), target.name()),
    );
}

/// Drop-confirm tests — the dialog driven the way the user drives it, over a catalog whose
/// dependency shape is the point: `orders` backs two views, one of them through a *nested* view,
/// while `users` backs none. So the two halves of the consequence line (its count and its names)
/// and its absence are all observable off one store.
///
/// The dialog is asserted through its rendered text rather than its internals, because the whole
/// deliverable here is what it *says* before a destructive action.
#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::thread::sleep;
    use std::time::Duration;

    use crate::apps::project::state::{CatalogState, Log, PersistFaults};

    use freya_testing::TestingRunner;
    use futures::executor::block_on;
    use strata_core::engine::{RunTag, TableMeta, ViewMeta, WsId};
    use strata_core::project::{self as project_io, ProjectDefs};
    use strata_core::theme::load;
    use strata_model::{
        ConnectionDef, GcsStore, Origin, Provider, S3Store, SavedQuery, SourceFormat, TableDef,
        TableOrigin, ViewDef,
    };

    use super::*;
    use crate::theme::strata_theme;

    fn table(name: &str) -> TableDef {
        TableDef {
            name: name.into(),
            format: SourceFormat::Parquet,
            sources: vec![format!("{name}.parquet")],
            partition_cols: Vec::new(),
            origin: TableOrigin::External,
        }
    }

    /// A table def Strata wrote — `.strata/tables/<name>/`, exactly as a CTAS leaves it.
    fn internal(name: &str) -> TableDef {
        TableDef {
            name: name.into(),
            format: SourceFormat::Arrow,
            sources: vec![format!(".strata/tables/{name}/")],
            partition_cols: Vec::new(),
            origin: TableOrigin::Internal,
        }
    }

    /// The drop gesture a catalog row starts, for the ordinary (external) table in this fixture.
    fn dropping(name: &str) -> DropTarget {
        DropTarget::Table {
            name: name.into(),
            origin: TableOrigin::External,
        }
    }

    fn view(name: &str, sql: &str) -> ViewDef {
        ViewDef {
            name: name.into(),
            sql: sql.into(),
        }
    }

    const QUERY_ID: Uuid = Uuid::from_u128(7);

    /// `orders` ← `orders_daily` ← `orders_weekly` (the nested reader), plus an unrelated
    /// `users` table, one saved query, and two connections **over one bucket** — the pair that
    /// only `url()` tells apart (W7).
    fn project(root: &Path) -> ProjectState {
        let defs = ProjectDefs {
            name: "test".into(),
            connections: vec![
                ConnectionDef {
                    address: "lake".into(),
                    provider: Provider::S3(S3Store::default()),
                    client_config: Default::default(),
                },
                ConnectionDef {
                    address: "lake".into(),
                    provider: Provider::Gcs(GcsStore::default()),
                    client_config: Default::default(),
                },
            ],
            tables: vec![table("orders"), table("users")],
            views: vec![
                view("orders_daily", "SELECT * FROM orders"),
                view("orders_weekly", "SELECT * FROM orders_daily"),
            ],
            saved_queries: vec![SavedQuery {
                id: QUERY_ID,
                name: "orders by region".into(),
                sql: "SELECT 1".into(),
                meta: "—".into(),
            }],
            ..Default::default()
        };
        let mut p = ProjectState::from_defs(defs, root.to_path_buf());
        for name in ["orders", "users"] {
            p.table_registered(
                name,
                TableMeta {
                    columns: Vec::new(),
                    rows: Some(1),
                },
            );
        }
        p.view_registered(
            "orders_daily",
            ViewMeta {
                columns: Vec::new(),
                tables: vec!["orders".into()],
                aliases: Vec::new(),
            },
        );
        // The planner inlines the inner view: base tables in `tables`, the view in `aliases`.
        p.view_registered(
            "orders_weekly",
            ViewMeta {
                columns: Vec::new(),
                tables: vec!["orders".into()],
                aliases: vec!["orders_daily".into()],
            },
        );
        p
    }

    fn app() -> impl IntoElement {
        use_init_theme(|| strata_theme(&load("midnight")));
        let target = use_consume::<State<Option<DropTarget>>>();
        rect().expanded().child(DropConfirm { target })
    }

    type Handles = (
        State<Option<DropTarget>>,
        RadioStation<SessionState, Chan>,
        RadioStation<ProjectState, ProjChan>,
        crate::apps::project::state::LogCtx,
    );

    /// A scratch project folder for one test.
    ///
    /// `env::temp_dir()` + **pid**, matching `strata_core::project`'s own test convention and for
    /// the same reason it gives: the OS temp dir is machine-shared, so a hardcoded `/tmp/…` path
    /// collides between parallel test binaries (this repo builds in several worktrees at once)
    /// and can land on another user's directory. Confirming a drop really does write
    /// `.strata/project.json`, and `persist` only logs a failure — so a collision would surface
    /// as a test that passes while writing nothing.
    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("strata-drop-{tag}-{}", std::process::id()))
    }

    /// A runner over its own scratch project — per test, because confirming really does write.
    fn runner(tag: &'static str) -> (TestingRunner, Handles) {
        let root = temp_root(tag);
        TestingRunner::new(
            app,
            (900., 700.).into(),
            move |r| {
                r.provide_root_context(EngineCtx::default);
                r.provide_root_context(|| State::create(CatalogState::Settled(0)));
                let target = r.provide_root_context(|| State::create(None::<DropTarget>));
                let session = r.provide_root_context(|| {
                    RadioStation::<SessionState, Chan>::create(SessionState::default())
                });
                let project = r.provide_root_context(|| {
                    RadioStation::<ProjectState, ProjChan>::create(project(&root))
                });
                // The window's event log: a drop is a mutation, so it records one (P3-13).
                let log = r.provide_root_context(|| State::create(Log::default()));
                // And the write-fault satellite the same funnel holds the condition in (P4-15) —
                // a failed drop write raises a Problems row as well as the event asserted below.
                r.provide_root_context(|| State::create(PersistFaults::default()));
                (target, session, project, log)
            },
            1.,
        )
    }

    /// Open the dialog on `target` and settle the tree.
    fn open(runner: &mut TestingRunner, slot: &mut State<Option<DropTarget>>, target: DropTarget) {
        runner.sync_and_update();
        slot.set(Some(target));
        runner.sync_and_update();
        runner.sync_and_update();
    }

    /// Every `label()` run in the tree — the dialog's body copy, chips and button labels.
    fn texts(runner: &TestingRunner) -> Vec<String> {
        runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
    }

    fn shows(runner: &TestingRunner, text: &str) -> bool {
        texts(runner).iter().any(|t| t == text)
    }

    /// The header's two lines — the action over its subject — joined with a space, so a test can
    /// state the whole title in one assertion.
    fn title(runner: &TestingRunner) -> String {
        texts(runner)
            .into_iter()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Press the action-strip button labelled `text`.
    ///
    /// The **lowest** matching run, not the first: the header's title carries the same words as
    /// the button it confirms ("Drop table" over `orders`, then `Drop table` in the strip), so
    /// taking the first match presses the title and the drop silently never happens.
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

    /// The headline: the confirm states how many views the drop leaves invalid **and** names
    /// them — including the one that only reads the table through another view, which is exactly
    /// the reader a warning built by scanning SQL text would miss.
    #[test]
    fn the_confirm_counts_and_names_every_view_the_drop_leaves_invalid() {
        let (mut runner, (mut slot, ..)) = runner("names");
        open(&mut runner, &mut slot, dropping("orders"));

        assert_eq!(title(&runner), "Drop table orders");
        assert!(
            shows(&runner, "2 views read this table and will be left invalid:"),
            "the consequence line leads with the count: {:?}",
            texts(&runner)
        );
        assert!(shows(&runner, "orders_daily"));
        assert!(
            shows(&runner, "orders_weekly"),
            "the nested reader is just as invalid"
        );
    }

    /// A drop that breaks nothing says nothing — no callout, no "0 views". The absence is the
    /// message.
    #[test]
    fn a_drop_with_no_dependents_shows_no_consequence_line() {
        let (mut runner, (mut slot, ..)) = runner("nodeps");
        open(&mut runner, &mut slot, dropping("users"));

        assert_eq!(title(&runner), "Drop table users");
        assert!(
            !texts(&runner).iter().any(|t| t.contains("left invalid")),
            "nothing reads `users`: {:?}",
            texts(&runner)
        );
        // The body copy still runs — the dialog isn't empty, it just makes no extra claim. And
        // for an **external** table the claim it makes is that the files stay, which is the half
        // that stopped being true for every table the moment Strata could own some (ED-05).
        assert!(texts(&runner)
            .iter()
            .any(|t| t.contains("files on disk are not deleted")));
    }

    /// **An internal table's confirm names the data**, because dropping one deletes it. The
    /// external sentence above would be reassuring the user at exactly the moment the action is
    /// destructive — and both sentences are the engine's, so the card cannot promise something
    /// the drop's own report then contradicts.
    #[test]
    fn dropping_an_internal_table_says_its_data_goes_with_it() {
        let (mut runner, (mut slot, ..)) = runner("internal");
        open(
            &mut runner,
            &mut slot,
            DropTarget::Table {
                name: "daily".into(),
                origin: TableOrigin::Internal,
            },
        );

        assert_eq!(title(&runner), "Drop table daily");
        let texts = texts(&runner);
        assert!(
            texts.iter().any(|t| t.contains("data files")),
            "the copy names the data: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("are not deleted")),
            "and never claims the files survive: {texts:?}"
        );
    }

    /// Dropping a **view** asks the other dependency list: the view that reads it, not the
    /// readers of the table underneath it. Getting these two crossed would name `orders_weekly`
    /// for every drop in the project.
    #[test]
    fn dropping_a_view_names_its_view_readers_and_uses_the_view_wording() {
        let (mut runner, (mut slot, ..)) = runner("view");
        open(
            &mut runner,
            &mut slot,
            DropTarget::View("orders_daily".into()),
        );

        assert_eq!(title(&runner), "Drop view orders_daily");
        assert!(
            shows(&runner, "1 view reads this view and will be left invalid:"),
            "singular, and about the view: {:?}",
            texts(&runner)
        );
        assert!(shows(&runner, "orders_weekly"));
        // Once, in the header — the row being dropped is not listed as its own dependent. A
        // presence check can't say this any more, since the title names it by design.
        assert_eq!(
            texts(&runner)
                .iter()
                .filter(|t| *t == "orders_daily")
                .count(),
            1,
            "`orders_daily` appears as the subject, not as a chip"
        );
    }

    /// A saved query is a stored string, not a SQL object — nothing can read it, so the delete
    /// never warns, and it says *delete* rather than *drop*.
    #[test]
    fn deleting_a_saved_query_never_warns() {
        let (mut runner, (mut slot, ..)) = runner("query");
        open(
            &mut runner,
            &mut slot,
            DropTarget::Query {
                id: QUERY_ID,
                name: "orders by region".into(),
            },
        );

        assert_eq!(title(&runner), "Delete query orders by region");
        assert!(!texts(&runner).iter().any(|t| t.contains("left invalid")));
    }

    /// Confirming performs the drop: the def leaves the catalog and the dialog closes. The views
    /// over it stay — they are left *invalid*, not removed, which is the distinction the whole
    /// consequence line rests on.
    #[test]
    fn confirming_removes_the_def_and_leaves_its_dependents_in_place() {
        let (mut runner, (mut slot, _, project, ..)) = runner("confirm");
        open(&mut runner, &mut slot, dropping("orders"));

        click_action(&mut runner, "Drop table");

        let p = project.peek();
        assert!(
            !p.tables.iter().any(|t| t.def.name == "orders"),
            "the table is out of the catalog"
        );
        assert_eq!(p.views.len(), 2, "its readers are flagged, not deleted");
        assert_eq!(
            p.view_problem(&p.views[0]).as_deref(),
            Some("Reads orders, which is no longer in the catalog."),
            "and the row now says what the dialog warned"
        );
        assert!(slot.peek().is_none(), "the dialog closed itself");
    }

    /// A confirmed drop is **recorded** (P3-13). Worth pinning here rather than on the log store:
    /// the log is only useful if the mutation paths actually write to it, and the row it describes
    /// is gone from the catalog by the time anyone reads the message. Past tense, and the saved
    /// query is *deleted* — the log borrows the dialog's own distinction.
    #[test]
    fn confirming_records_the_drop_in_the_event_log() {
        let (mut runner, (mut slot, _, _, log)) = runner("logged");
        open(&mut runner, &mut slot, dropping("orders"));

        click_action(&mut runner, "Drop table");

        let recorded: Vec<(LogLevel, String)> = log
            .peek()
            .events()
            .map(|e| (e.level, e.message.clone()))
            .collect();
        assert_eq!(
            recorded,
            [(LogLevel::Info, "Dropped table 'orders'".to_string())]
        );
    }

    /// A drop the **project file never heard about** is not logged as a drop — it is logged as the
    /// write failure it was. The def is gone from this session's store either way (that is the
    /// catalog's truth and there is no rollback), but the row comes back on the next open, so a
    /// `Dropped table 'orders'` beside it would be the log contradicting the catalog. Before the
    /// fix, `save_defs` failing was a `tracing::error!` and nothing else, and the drop event was
    /// logged unconditionally *ahead* of the write.
    ///
    /// A read-only `.strata/` is the portable way to make the write fail (as `util`'s own
    /// `write_atomic` tests do); Unix-only, because that is where the mode bits mean this.
    #[cfg(unix)]
    #[test]
    fn a_drop_whose_project_write_fails_is_logged_as_the_failure() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_root("nowrite");
        let strata = project_io::strata_dir(&root);
        std::fs::create_dir_all(&strata).unwrap();
        let (mut runner, (mut slot, _, _, log)) = runner("nowrite");
        open(&mut runner, &mut slot, dropping("orders"));

        std::fs::set_permissions(&strata, std::fs::Permissions::from_mode(0o500)).unwrap();
        click_action(&mut runner, "Drop table");
        std::fs::set_permissions(&strata, std::fs::Permissions::from_mode(0o700)).unwrap();

        let recorded: Vec<(LogLevel, String)> = log
            .peek()
            .events()
            .map(|e| (e.level, e.message.clone()))
            .collect();
        assert_eq!(recorded.len(), 1, "one event, not two: {recorded:?}");
        let (level, message) = &recorded[0];
        assert_eq!(*level, LogLevel::Error);
        assert!(
            message
                .as_str()
                .starts_with("Could not write the project file: "),
            "unexpected message: {message}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A drop whose write fails is rolled back** (P4-15 item 4): the row is still in the
    /// catalog afterwards, so the store and `project.json` agree and the next open shows the same
    /// thing this session does.
    ///
    /// Without it the drop is the one mutation whose silent failure *resurrects* what the user
    /// destroyed — gone from the sidebar all session, back at the next open, with the only
    /// evidence a log row. Rollback is available here and nowhere else because at the moment the
    /// write fails nothing else has happened yet: the engine call comes after.
    #[cfg(unix)]
    #[test]
    fn a_drop_whose_project_write_fails_is_rolled_back() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_root("rollback");
        let strata = project_io::strata_dir(&root);
        std::fs::create_dir_all(&strata).unwrap();
        let (mut runner, (mut slot, _, project, _)) = runner("rollback");
        open(&mut runner, &mut slot, dropping("orders"));

        std::fs::set_permissions(&strata, std::fs::Permissions::from_mode(0o500)).unwrap();
        click_action(&mut runner, "Drop table");
        std::fs::set_permissions(&strata, std::fs::Permissions::from_mode(0o700)).unwrap();

        let p = project.peek();
        assert!(
            p.tables.iter().any(|t| t.def.name == "orders"),
            "the row should still be in the catalog: {:?}",
            p.tables.iter().map(|t| &t.def.name).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The rollback puts the row back **where it was**, not on the end — the catalog is ordered,
    /// and a row that reappeared somewhere else would read as a different change.
    #[cfg(unix)]
    #[test]
    fn a_rolled_back_drop_keeps_the_catalogs_order() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_root("rollback-order");
        let strata = project_io::strata_dir(&root);
        std::fs::create_dir_all(&strata).unwrap();
        let (mut runner, (mut slot, _, project, _)) = runner("rollback-order");
        let before: Vec<String> = project
            .peek()
            .tables
            .iter()
            .map(|t| t.def.name.clone())
            .collect();
        open(&mut runner, &mut slot, dropping("orders"));

        std::fs::set_permissions(&strata, std::fs::Permissions::from_mode(0o500)).unwrap();
        click_action(&mut runner, "Drop table");
        std::fs::set_permissions(&strata, std::fs::Permissions::from_mode(0o700)).unwrap();

        let after: Vec<String> = project
            .peek()
            .tables
            .iter()
            .map(|t| t.def.name.clone())
            .collect();
        assert_eq!(after, before, "the catalog should be exactly as it was");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A saved query rolls back the same way — no engine involved at all, so this is the arm that
    /// isolates the store half of the rule.
    #[cfg(unix)]
    #[test]
    fn a_dropped_query_whose_write_fails_stays_in_the_catalog() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_root("rollback-query");
        let strata = project_io::strata_dir(&root);
        std::fs::create_dir_all(&strata).unwrap();
        let (mut runner, (mut slot, _, project, _)) = runner("rollback-query");
        let id = project.peek().saved_queries.first().map(|q| q.id);
        let Some(id) = id else {
            let _ = std::fs::remove_dir_all(&root);
            return;
        };
        open(
            &mut runner,
            &mut slot,
            DropTarget::Query {
                id,
                name: "q".into(),
            },
        );

        std::fs::set_permissions(&strata, std::fs::Permissions::from_mode(0o500)).unwrap();
        click_action(&mut runner, "Delete query");
        std::fs::set_permissions(&strata, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(
            project.peek().saved_queries.iter().any(|q| q.id == id),
            "the query should still be in the catalog"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Cancelling is a true no-op — the catalog is untouched, nothing is logged, and the dialog
    /// closes. Worth pinning because the destructive path runs through the same closure the Enter
    /// key uses.
    #[test]
    fn cancelling_touches_nothing() {
        let (mut runner, (mut slot, _, project, log)) = runner("cancel");
        open(&mut runner, &mut slot, dropping("orders"));

        click_action(&mut runner, "Cancel");
        assert_eq!(
            log.peek().len(),
            0,
            "a drop that never happened is not an event"
        );

        assert!(project.peek().tables.iter().any(|t| t.def.name == "orders"));
        assert!(slot.peek().is_none());
    }

    /// Dropping a view unbinds the tab that was saving to it. Left bound, the next ⌘S would
    /// re-create the view the user just dropped — the buffer survives, the binding must not.
    #[test]
    fn dropping_a_view_unbinds_the_tab_bound_to_it() {
        let (mut runner, (mut slot, mut session, ..)) = runner("unbind");
        let tab = session.write_channel(Chan::Tabs).open_named(
            "orders_daily",
            "SELECT * FROM orders".into(),
            Origin::View("orders_daily".into()),
        );
        open(
            &mut runner,
            &mut slot,
            DropTarget::View("orders_daily".into()),
        );

        click_action(&mut runner, "Drop view");

        let s = session.peek();
        let t = s.tabs.get(&tab).expect("the tab is still open");
        assert!(
            matches!(t.origin, Origin::Scratch),
            "the binding is cut, not the tab"
        );
        assert_eq!(t.text(), "SELECT * FROM orders", "the buffer is untouched");
    }

    /// The action strip is the comps' **58px**: a 34px button row with `--sp-4` above and below.
    /// Freya's `button_layout` hugs its label (≈28px) unless told otherwise, which made both
    /// confirms read as squashed — so the number is asserted rather than eyeballed, on the strip
    /// *and* on its buttons, since only the pair together prove where the height came from.
    #[test]
    fn the_action_strip_is_the_comps_fifty_eight_pixels() {
        let (mut runner, (mut slot, ..)) = runner("footer");
        open(&mut runner, &mut slot, dropping("orders"));

        // Both actions, by their laid-out boxes: the two buttons are the only 34px-tall boxes
        // carrying a button role.
        let buttons: Vec<f32> = runner.find_many(|node, element| {
            (element.accessibility().builder.role() == AccessibilityRole::Button)
                .then(|| node.layout().area.height())
        });
        assert_eq!(
            buttons,
            vec![34., 34.],
            "Cancel and the drop action are both 34px tall"
        );

        // The strip: the widest box that is exactly 58 tall.
        let strip = runner
            .find_many(|node, _| {
                let a = node.layout().area;
                ((a.height() - 58.).abs() < 0.5).then(|| a.width())
            })
            .into_iter()
            .fold(0., f32::max);
        assert!(
            strip > 0.,
            "no 58px-tall strip: 12 + 34 + 12 did not survive the layout"
        );
    }

    /// Headless preview for eyeballing against the canvas `remove-deps` comp. Ignored by default
    /// (it writes a file, asserts nothing):
    /// `cargo test -p strata-freya drop_confirm_preview -- --ignored`.
    #[test]
    #[ignore = "writes target/drop-confirm-preview.png for eyeballing; run explicitly"]
    fn drop_confirm_preview() {
        let (mut runner, (mut slot, ..)) = runner("preview");
        open(&mut runner, &mut slot, dropping("orders"));
        runner.render_to_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/drop-confirm-preview.png"
        ));
    }

    /// Deleting a saved query removes it by **id** and unbinds the tab that was saving to it.
    /// The whole `DropTarget::Query` confirm branch was otherwise unexercised — the dialog was
    /// opened and read, never confirmed — while `remove_saved_query`'s `#[allow(dead_code)]` was
    /// dropped on the strength of that very call.
    #[test]
    fn deleting_a_saved_query_removes_it_and_unbinds_its_tab() {
        let (mut runner, (mut slot, mut session, project, ..)) = runner("querydel");
        let tab = session.write_channel(Chan::Tabs).open_named(
            "orders by region",
            "SELECT 1".into(),
            Origin::SavedQuery(QUERY_ID),
        );
        open(
            &mut runner,
            &mut slot,
            DropTarget::Query {
                id: QUERY_ID,
                name: "orders by region".into(),
            },
        );

        click_action(&mut runner, "Delete query");

        assert!(
            project.peek().saved_queries.is_empty(),
            "the query is out of the project"
        );
        let s = session.peek();
        let t = s.tabs.get(&tab).expect("the tab is still open");
        assert!(matches!(t.origin, Origin::Scratch), "the binding is cut");
        assert_eq!(t.text(), "SELECT 1", "the buffer is untouched");
    }

    /// Enter confirms. It runs through `Dialog::on_confirm` — a *different* closure from the
    /// button's — so without this the two could be swapped and the suite would stay green.
    #[test]
    fn enter_confirms_the_drop() {
        let (mut runner, (mut slot, _, project, ..)) = runner("enter");
        open(&mut runner, &mut slot, dropping("orders"));

        runner.press_key(Key::Named(NamedKey::Enter));
        runner.sync_and_update();

        assert!(
            !project.peek().tables.iter().any(|t| t.def.name == "orders"),
            "Enter performed the drop"
        );
        assert!(slot.peek().is_none(), "and closed the dialog");
    }

    /// **Forget** is the same confirm on a fourth target: it names the connection by its
    /// `url()`, warns about nothing (an object store is not in the SQL namespace, so nothing can
    /// read it by name), and confirming takes exactly that connection out of the project.
    ///
    /// The bucket the two connections share is the whole point of asserting the survivor: a
    /// removal keyed on it would take both, or the wrong one.
    #[test]
    fn forgetting_a_connection_removes_the_one_its_url_names() {
        let (mut runner, (mut slot, _, project, ..)) = runner("forget");
        open(
            &mut runner,
            &mut slot,
            DropTarget::Connection("gs://lake".into()),
        );

        assert_eq!(title(&runner), "Forget connection gs://lake");
        assert!(
            !texts(&runner).iter().any(|t| t.contains("left invalid")),
            "nothing reads an object store by name: {:?}",
            texts(&runner)
        );
        assert!(texts(&runner)
            .iter()
            .any(|t| t.contains("Nothing in the bucket is deleted")));

        click_action(&mut runner, "Forget connection");

        assert_eq!(
            project
                .peek()
                .connections
                .iter()
                .map(|c| c.def.url())
                .collect::<Vec<_>>(),
            ["s3://lake"],
            "the other connection over the same bucket stays"
        );
        assert!(slot.peek().is_none(), "the dialog closed itself");
    }

    /// A forget is **recorded**, in the past tense and in the pane's own word — the row it
    /// describes is gone from the store by the time anyone reads the message.
    #[test]
    fn forgetting_a_connection_records_the_event() {
        let (mut runner, (mut slot, _, _, log)) = runner("forget-log");
        open(
            &mut runner,
            &mut slot,
            DropTarget::Connection("s3://lake".into()),
        );

        click_action(&mut runner, "Forget connection");

        let recorded: Vec<(LogLevel, String)> = log
            .peek()
            .events()
            .map(|e| (e.level, e.message.clone()))
            .collect();
        assert_eq!(
            recorded,
            [(LogLevel::Info, "Forgot connection 's3://lake'".to_string())]
        );
    }

    /// **Confirming an internal table's drop deletes its data** — the half of ED-05's parity
    /// only this surface can show, the other half (that the typed statement and
    /// `Engine::drop_table` leave the same state) being pinned in `strata-core`.
    ///
    /// Before ED-05 this arm called `Engine::deregister`, which forgets the provider and nothing
    /// else. On an internal table that orphans `.strata/tables/<slug>/` forever: no def points at
    /// it and the `.strata` housekeeping only sweeps `.tmp-` directories.
    ///
    /// Driven over a **real** engine and a real project folder, because the claim is about a
    /// directory on disk. The engine call is spawned, so the assertion waits for it rather than
    /// assuming one tick is enough.
    #[test]
    fn confirming_an_internal_drop_deletes_its_data() {
        let root = temp_root("internal-data");
        let _ = std::fs::remove_dir_all(&root);
        project_io::save_defs(&root, &ProjectDefs::default()).expect("scaffolded");
        let engine = EngineCtx::default();
        engine.set_data_dir(&root);
        // A real internal table, made the way the editor makes one.
        block_on(engine.run(
            WsId(9),
            RunTag(9),
            "CREATE TABLE daily AS SELECT 1 AS n".into(),
            10,
        ))
        .expect("created");
        let dir = project_io::tables_dir(&root).join("daily");
        assert!(dir.exists(), "the CTAS wrote its data");

        let (mut runner, (mut slot, ..)) = {
            let engine = engine.clone();
            let root = root.clone();
            TestingRunner::new(
                app,
                (900., 700.).into(),
                move |r| {
                    r.provide_root_context(|| engine.clone());
                    r.provide_root_context(|| State::create(CatalogState::Settled(0)));
                    let target = r.provide_root_context(|| State::create(None::<DropTarget>));
                    let session = r.provide_root_context(|| {
                        RadioStation::<SessionState, Chan>::create(SessionState::default())
                    });
                    let project = r.provide_root_context(|| {
                        let mut p = ProjectState::from_defs(
                            ProjectDefs {
                                tables: vec![internal("daily")],
                                ..Default::default()
                            },
                            root.clone(),
                        );
                        p.table_registered(
                            "daily",
                            TableMeta {
                                columns: Vec::new(),
                                rows: Some(1),
                            },
                        );
                        RadioStation::<ProjectState, ProjChan>::create(p)
                    });
                    let log = r.provide_root_context(|| State::create(Log::default()));
                    r.provide_root_context(|| State::create(PersistFaults::default()));
                    (target, session, project, log)
                },
                1.,
            )
        };
        open(
            &mut runner,
            &mut slot,
            DropTarget::Table {
                name: "daily".into(),
                origin: TableOrigin::Internal,
            },
        );

        click_action(&mut runner, "Drop table");

        // The drop is dispatched onto the engine's own runtime and awaited by a root task, so
        // the answer lands a wake later — bounded rather than assumed.
        for _ in 0..200 {
            if !dir.exists() {
                break;
            }
            sleep(Duration::from_millis(10));
            runner.sync_and_update();
        }
        assert!(!dir.exists(), "{} was left behind", dir.display());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Esc cancels — the dialog's own key barrier, the same one that stops a keystroke aimed at
    /// it reaching the workbench underneath.
    #[test]
    fn escape_cancels_the_drop() {
        let (mut runner, (mut slot, _, project, ..)) = runner("escape");
        open(&mut runner, &mut slot, dropping("orders"));

        runner.press_key(Key::Named(NamedKey::Escape));
        runner.sync_and_update();

        assert!(project.peek().tables.iter().any(|t| t.def.name == "orders"));
        assert!(slot.peek().is_none());
    }
}
