//! The window's **agent bridge** (AA-03): one serial driver that turns an [`AgentAsk`] into a
//! Radio read or write and replies, plus the projections it answers with.
//!
//! Mounted by `ProjectLoaded`, so it lives exactly as long as the mount whose handles it
//! borrows — which is what makes a re-root and an engine restart deregister and re-register
//! through the same path a close and an open take. There is no second cleanup route to drift.
//!
//! **Serial on purpose**, the diagnostics driver's reasoning: the engine has two workers and
//! the user's own press comes first. That costs nothing here because the loop never waits for
//! a query — a `run` sets the tab's request and **parks** its reply against the press's nonce,
//! and the keeper that observes the settle (`views::agent_keeper`) completes it. So one ask is
//! handled at a time and a running query blocks none of them.
//!
//! **No Radio borrow is ever held across an await.** Every read and write here is a whole
//! statement between two `recv`s: the guard is taken, used and dropped before the loop comes
//! back round. That is the `GenerationalBox` trap, and it is a trap rather than a rule because
//! the failure is a panic on an unrelated repaint.
//!
//! **The catalog is answered from [`ProjectState`]**, never from DataFusion introspection
//! (AGENTS.md §2): introspection would list the `__snap_*` result snapshots and hide the defs
//! whose registration failed — precisely the rows an agent most needs to see, because a table
//! that is merely broken looks exactly like a table that was never registered.

use std::path::PathBuf;
use std::sync::Arc;

use freya::prelude::{spawn, use_consume, use_drop, use_hook, use_provide_context, State};
use freya::radio::{use_radio, Radio};
use strata_agent::{
    AgentError, CatalogEntry, Described, RegState, RunMode, RunSettle, TabInfo, TabState,
};
use strata_core::engine::Engine;
use strata_model::TabId;
use tokio::sync::{mpsc, oneshot};

use crate::agent::ask::AgentAsk;
use crate::agent::AgentCtx;
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{QueryMode, QuerySpec, RunId};
use crate::apps::project::views::workbench::editor::actions::load_sql;

use super::{log_event, Chan, LogCtx, LogLevel, ProjChan, ProjectState, Reg, SessionState};

/// One agent `run` whose press is out and whose reply is still owed.
///
/// The reply is an `Option` for one reason: a `oneshot::Sender` answers by being **consumed**,
/// and the keeper has to take it out of a shared list rather than own it. `None` is therefore
/// never a state anything observes — the entry is removed in the same write that takes the
/// sender — but it is what makes taking it possible at all.
pub struct AgentRun {
    pub spec: QuerySpec,
    pub reply: Option<oneshot::Sender<Result<RunSettle, AgentError>>>,
}

/// The window's parked run replies, in context: written by the driver, drained by the keepers.
///
/// Context rather than a prop because its two users sit at different depths and neither owns
/// the other (the driver is a hook on `ProjectLoaded`, the keepers are a child of it), and
/// window-lived rather than app-global because a reply is owed by *this* mount: when the
/// subtree goes, the senders go with it and every ask still out answers "the project window
/// closed".
pub type AgentRuns = State<Vec<AgentRun>>;

/// Join the service directory for this mount's lifetime and drive its asks. Call once in
/// `ProjectLoaded`, after the engine, the log and both stores are in place.
///
/// `name` is taken once rather than read reactively: there is no project-rename entry point
/// today, and a registration is a launch value like the recents promotion beside it. When a
/// rename lands it routes through the Project store, and this is the second reader to point
/// at it.
pub fn use_agent_bridge(agent: AgentCtx, root: PathBuf, name: String) {
    let engine = use_consume::<EngineCtx>();
    let log = use_consume::<LogCtx>();
    // Handles, not subscriptions. The driver is a task, and a task has no reactive scope, so
    // its `read`s are peek-equivalent — the same thing the diagnostics driver relies on. The
    // channels named here therefore decide nothing; every write below names its own.
    let session = use_radio::<SessionState, Chan>(Chan::Tabs);
    let project = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
    let runs: AgentRuns = use_provide_context(|| State::create(Vec::new()));

    let registration = use_hook({
        let directory = Arc::clone(&agent.directory);
        let engine = engine.clone();
        move || {
            // The engine goes to the directory as a bare `Arc` — the **data plane**, called
            // from the server's own runtime, so `fetch_page` / `validate` / `functions` never
            // queue behind a repaint.
            let (id, rx) = directory.register(root, name, engine.arc());
            spawn(drive(rx, engine, session, project, log, runs));
            id
        }
    });
    use_drop(move || agent.directory.deregister(registration));
}

/// The loop. Ends when the directory's sender is dropped, or when the scope that spawned it
/// is torn down — which are the same event seen from the two ends.
async fn drive(
    mut rx: mpsc::Receiver<AgentAsk>,
    engine: EngineCtx,
    mut session: Radio<SessionState, Chan>,
    project: Radio<ProjectState, ProjChan>,
    log: LogCtx,
    mut runs: AgentRuns,
) {
    // Every `send` is `let _ =`: a client that gave up mid-call drops its receiver, and there
    // is nothing to report about an answer nobody is waiting for.
    while let Some(ask) = rx.recv().await {
        match ask {
            AgentAsk::Catalog(reply) => {
                let _ = reply.send(catalog(&project.read()));
            }
            AgentAsk::Describe { name, reply } => {
                let _ = reply.send(describe(&project.read(), &name));
            }
            AgentAsk::Tabs(reply) => {
                let _ = reply.send(tabs(&session.read(), &engine));
            }
            AgentAsk::OpenTab(reply) => {
                // Appended, **not focused** — see `SessionState::open_background`. The tab
                // arrives in the strip exactly where ⌘T's would; what it does not do is take
                // the editor out from under whoever is typing in it.
                let tab = session.write_channel(Chan::Tabs).open_background();
                let named = session.read().name(tab);
                log_event(log, LogLevel::Info, format!("Agent opened tab '{named}'"));
                let _ = reply.send(tab);
            }
            AgentAsk::CloseTab { tab, reply } => {
                let Some(named) = named(session, tab) else {
                    let _ = reply.send(Err(no_such_tab(tab)));
                    continue;
                };
                // **The close funnel, not the gate in front of it.** `close_one` plus the
                // root's tab-diff effect *is* the funnel every close path shares: the diff
                // calls `EngineCtx::cleanup`, which aborts an in-flight run and retires the
                // tab's snapshot — a running press cancelled the ordinary way, which is what
                // this owes. What it deliberately does not do is raise the T2 confirm. That
                // dialog asks the *user* whether to destroy work, and neither answer is one a
                // tool call can live with: replying Ok while a dialog decides would report a
                // tab closed that is still open, and waiting on the dialog would make an agent
                // block on a modal — the shape spec §6 rules out for profiling, for the same
                // reason. Tabs are shared last-writer-wins (spec §1), so an agent closing one
                // is a write like any other.
                session.write_channel(Chan::Tabs).close_one(tab);
                log_event(log, LogLevel::Info, format!("Agent closed tab '{named}'"));
                let _ = reply.send(Ok(()));
            }
            AgentAsk::Run {
                tab,
                sql,
                mode,
                page_size,
                reply,
            } => {
                let Some(named) = named(session, tab) else {
                    let _ = reply.send(Err(no_such_tab(tab)));
                    continue;
                };
                let spec = QuerySpec {
                    tab,
                    run: RunId::new(),
                    sql,
                    mode: query_mode(mode),
                    page_size,
                };
                // Parked before the press, so the keeper that will observe this settle is
                // mounted by the same update pass that dispatches it. (The SQL arrives past
                // the policy gate and is never rewritten — no injected `LIMIT`; the response
                // is bounded by `page_size` and `read_page`, and the total stays exact.)
                runs.write().push(AgentRun {
                    spec: spec.clone(),
                    reply: Some(reply),
                });
                // **The SQL into the tab's buffer, first** — through the editor's own action,
                // which makes this the History drawer's double-press exactly: load, then
                // press. Without it the tab shows results over an empty editor, and the whole
                // premise of landing agent queries in real tabs is that the user can read what
                // ran and take it over. A plain `set_text`, so it is undoable like any other.
                // (Explain loads the plain statement, not `EXPLAIN …`: the wrapping happens at
                // dispatch, exactly as the editor's own Explain button does it.)
                load_sql(session, tab, &spec.sql);
                session
                    .write_channel(Chan::Request(tab))
                    .set_request(tab, spec);
                log_event(
                    log,
                    LogLevel::Info,
                    format!("Agent ran a query in '{named}'"),
                );
            }
        }
    }
}

/// `tab`'s display name, or `None` when nothing open answers to that handle — the one
/// existence check every tab-scoped ask makes, taken as a whole borrow so the read cannot
/// straddle the write that follows it.
fn named(session: Radio<SessionState, Chan>, tab: TabId) -> Option<String> {
    let session = session.read();
    session.tabs.contains_key(&tab).then(|| session.name(tab))
}

/// What a tool says about a tab handle nothing open answers to. One wording, because
/// `list_tabs` is the recovery from every one of them.
fn no_such_tab(tab: TabId) -> AgentError {
    AgentError::NotFound(format!("No open tab '{}'.", tab.0))
}

/// The vocabulary's mode as a press's.
///
/// `Explain` is never `analyze: true`: an `EXPLAIN ANALYZE` **executes** the query, which is
/// the opposite of what a caller asking for a plan wants and costs exactly what they were
/// avoiding. `RunQuery` wraps the statement with `as_explain` on the way, so `mode: "explain"`
/// means "plan this", not "I already typed EXPLAIN".
fn query_mode(mode: RunMode) -> QueryMode {
    match mode {
        RunMode::Run => QueryMode::Run,
        RunMode::Explain => QueryMode::Explain { analyze: false },
    }
}

// --- the projections --------------------------------------------------------
//
// Free functions over the stores rather than steps inside the driver, so the mapping is
// testable against a store built by hand with no renderer and no window: the loop above is
// then only the ordering (read, write, reply), which is the half a test cannot reach anyway.

/// The catalog as the sidebar shows it: tables, then views, then saved queries.
fn catalog(project: &ProjectState) -> Vec<CatalogEntry> {
    let tables = project.tables.iter().map(|row| CatalogEntry::Table {
        name: row.def.name.clone(),
        format: row.def.format.name().to_string(),
        // As stored. A relative entry is the user's own text and stays that way — resolving
        // it is the registration pass's business, and a listing that silently absolutized
        // paths would describe a def nobody wrote.
        sources: row.def.sources.clone(),
        reg: reg_state(&row.reg),
    });
    let views = project.views.iter().map(|row| CatalogEntry::View {
        name: row.def.name.clone(),
        sql: row.def.sql.clone(),
        reg: reg_state(&row.reg),
    });
    let queries = project.saved_queries.iter().map(|q| CatalogEntry::Query {
        id: q.id,
        name: q.name.clone(),
        sql: q.sql.clone(),
    });
    tables.chain(views).chain(queries).collect()
}

/// One table or view in full — only what registration actually read (P3-08).
///
/// A **saved query is not describable**, and falls through to the same not-found as a name
/// nothing owns: it is a string the user parked, not an object the engine holds, so it has no
/// schema to report and no registration state to be in. `list_tables` is where it appears.
fn describe(project: &ProjectState, name: &str) -> Result<Described, AgentError> {
    if let Some(row) = project
        .tables
        .iter()
        .find(|r| ProjectState::same_name(&r.def.name, name))
    {
        return Ok(match &row.reg {
            Reg::Ready(meta) => Described::Table {
                name: row.def.name.clone(),
                format: row.def.format.name().to_string(),
                sources: row.def.sources.clone(),
                partitions: row.def.partition_cols.clone(),
                rows: meta.rows,
                columns: meta.columns.clone(),
            },
            Reg::Failed(error) => Described::Failed {
                name: row.def.name.clone(),
                error: error.clone(),
            },
            Reg::Loading => Described::Pending {
                name: row.def.name.clone(),
            },
        });
    }
    if let Some(row) = project
        .views
        .iter()
        .find(|r| ProjectState::same_name(&r.def.name, name))
    {
        return Ok(match &row.reg {
            Reg::Ready(info) => Described::View {
                name: row.def.name.clone(),
                sql: row.def.sql.clone(),
                columns: info.columns.clone(),
                reads: info.deps.clone(),
            },
            Reg::Failed(error) => Described::Failed {
                name: row.def.name.clone(),
                error: error.clone(),
            },
            Reg::Loading => Described::Pending {
                name: row.def.name.clone(),
            },
        });
    }
    Err(AgentError::NotFound(format!(
        "Table or view '{name}' not found."
    )))
}

/// The open tabs, in strip order.
///
/// Whether a tab's run is still going is asked of the **engine**, not of the store, for the
/// reason `TabCloser` asks it there too: a tab *is* a workspace, only the active tab's results
/// are mounted, and the store holds the request rather than its state. The request is what
/// distinguishes "nothing has ever run here" from "something has" — a run that failed is as
/// settled as one that produced rows, and reporting it as `Empty` would tell an agent nothing
/// had ever happened in a tab showing an error.
fn tabs(session: &SessionState, engine: &Engine) -> Vec<TabInfo> {
    session
        .order
        .iter()
        .filter_map(|id| session.tabs.get(id))
        .map(|tab| TabInfo {
            tab: tab.id,
            title: tab.name.clone(),
            state: match tab.request {
                None => TabState::Empty,
                Some(_) if engine.is_running(tab.id.into()) => TabState::Running,
                Some(_) => TabState::Settled,
            },
        })
        .collect()
}

/// [`Reg`] without its payload — what a *listing* row carries, as against what [`describe`]
/// answers.
fn reg_state<T>(reg: &Reg<T>) -> RegState {
    match reg {
        Reg::Loading => RegState::Pending,
        Reg::Ready(_) => RegState::Ready,
        Reg::Failed(error) => RegState::Failed(error.clone()),
    }
}

#[cfg(test)]
mod tests {
    use strata_core::engine::{TableMeta, ViewMeta};
    use strata_core::project::ProjectDefs;
    use strata_model::{ColumnInfo, Kind, Origin, SavedQuery, SourceFormat, TableDef, ViewDef};
    use uuid::Uuid;

    use super::*;

    fn column(name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            dtype: "Int64".into(),
            kind: Kind::Num,
            nullable: true,
            children: Vec::new(),
            stats: Vec::new(),
        }
    }

    /// A store with one ready table, one refused table, one ready view and a saved query —
    /// built directly, the way AGENTS.md §1 asks (no production signature bent to be
    /// testable).
    fn store() -> ProjectState {
        let mut project = ProjectState::from_defs(
            ProjectDefs {
                name: "sales".into(),
                tables: vec![
                    TableDef {
                        name: "orders".into(),
                        format: SourceFormat::from_name("parquet"),
                        sources: vec!["data/orders".into()],
                        partition_cols: vec![("year".into(), "Int32".into())],
                    },
                    TableDef {
                        name: "gone".into(),
                        format: SourceFormat::from_name("csv"),
                        sources: vec!["missing.csv".into()],
                        partition_cols: Vec::new(),
                    },
                ],
                views: vec![ViewDef {
                    name: "daily".into(),
                    sql: "SELECT * FROM orders".into(),
                }],
                saved_queries: vec![SavedQuery {
                    id: Uuid::nil(),
                    name: "scratch".into(),
                    sql: "SELECT 1".into(),
                    meta: "—".into(),
                }],
            },
            PathBuf::from("/w/sales"),
        );
        project.table_registered(
            "orders",
            TableMeta {
                columns: vec![column("id"), column("total")],
                rows: Some(42),
            },
        );
        project.table_failed("gone", "No source paths".into());
        project.view_registered(
            "daily",
            ViewMeta {
                columns: vec![column("id")],
                tables: vec!["orders".into()],
                aliases: Vec::new(),
            },
        );
        project
    }

    /// The whole catalog, failed rows included — the P3-02 correction, which is the reason
    /// this is a store projection rather than an engine question.
    #[test]
    fn the_catalog_lists_every_def_with_its_registration_state() {
        let listed = catalog(&store());
        match &listed[..] {
            [CatalogEntry::Table {
                name: ready,
                format,
                sources,
                reg: RegState::Ready,
            }, CatalogEntry::Table {
                name: broken,
                reg: RegState::Failed(why),
                ..
            }, CatalogEntry::View {
                name: view,
                reg: RegState::Ready,
                ..
            }, CatalogEntry::Query { name: query, .. }] => {
                assert_eq!(ready, "orders");
                assert_eq!(format, "parquet");
                assert_eq!(sources, &vec!["data/orders".to_string()]);
                assert_eq!(broken, "gone");
                assert_eq!(why, "No source paths");
                assert_eq!(view, "daily");
                assert_eq!(query, "scratch");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn describe_reports_what_registration_read() {
        let project = store();
        let Ok(Described::Table {
            partitions,
            rows,
            columns,
            ..
        }) = describe(&project, "orders")
        else {
            panic!("expected a table");
        };
        assert_eq!(partitions, vec![("year".to_string(), "Int32".to_string())]);
        assert_eq!(rows, Some(42));
        assert_eq!(columns.len(), 2);

        // DataFusion folds unquoted identifiers, so the catalog's own case-insensitive
        // compare is what an agent's spelling is matched with.
        assert!(matches!(
            describe(&project, "ORDERS"),
            Ok(Described::Table { .. })
        ));
    }

    /// A def the engine refused has no schema to report — and saying so is the point, since it
    /// is otherwise indistinguishable from a table nobody registered.
    #[test]
    fn describe_reports_a_refused_def_as_failed_rather_than_missing() {
        let Ok(Described::Failed { error, .. }) = describe(&store(), "gone") else {
            panic!("expected a failed def");
        };
        assert_eq!(error, "No source paths");
    }

    #[test]
    fn describe_of_a_view_names_what_it_reads() {
        let Ok(Described::View { sql, reads, .. }) = describe(&store(), "daily") else {
            panic!("expected a view");
        };
        assert_eq!(sql, "SELECT * FROM orders");
        assert_eq!(reads, vec!["orders".to_string()]);
    }

    /// A saved query is a parked string, not an object the engine holds, so it lists but does
    /// not describe — the same answer a name nothing owns gets.
    #[test]
    fn a_saved_query_is_listed_but_not_describable() {
        for name in ["scratch", "nope"] {
            assert!(
                matches!(describe(&store(), name), Err(AgentError::NotFound(_))),
                "{name}"
            );
        }
    }

    /// Strip order, and the tri-state: a tab that has never run is `Empty`, one whose press
    /// has settled is `Settled`. (`Running` is the engine's own `is_running`, which this reads
    /// rather than re-derives.)
    #[test]
    fn tabs_are_reported_in_strip_order_with_their_run_state() {
        let engine = Engine::new(Default::default());
        let mut session = SessionState::default();
        let first = session.open_named("findings", "SELECT 1".into(), Origin::Scratch);
        let second = session.open_blank();

        let listed = tabs(&session, &engine);
        assert_eq!(
            listed.iter().map(|t| t.tab).collect::<Vec<_>>(),
            vec![first, second]
        );
        assert_eq!(listed[0].title, "findings");
        assert!(listed.iter().all(|t| matches!(t.state, TabState::Empty)));

        session.set_request(
            first,
            QuerySpec {
                tab: first,
                run: RunId::new(),
                sql: "SELECT 1".into(),
                mode: QueryMode::Run,
                page_size: 100,
            },
        );
        // Nothing is executing on this engine, so a tab that carries a request has settled.
        assert!(matches!(
            tabs(&session, &engine)[0].state,
            TabState::Settled
        ));
    }
}
