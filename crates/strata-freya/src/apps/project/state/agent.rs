//! The window's **agent bridge** (AA-03, re-pointed by AA-03b): one serial driver that turns
//! an [`AgentAsk`] or an [`AgentNotice`] into a read or write of this window's state, plus the
//! projections it answers with.
//!
//! Mounted by `ProjectLoaded`, so it lives exactly as long as the mount whose handles it
//! borrows — which is what makes a re-root and an engine restart deregister and re-register
//! through the same path a close and an open take. There is no second cleanup route to drift.
//!
//! **Serial on purpose**, the diagnostics driver's reasoning: the engine has two workers and
//! the user's own press comes first. That costs nothing here because the loop never waits for
//! a query — since AA-03b a run is dispatched by the *caller*, on the engine directly
//! (`agent::directory`), and all this loop does is bracket it: check the agent holds the
//! session and record the query, then record its outcome when the notice lands. A settle
//! cannot overtake its own dispatch because the directory awaits the `RunStarting` reply
//! before it touches the engine — the ordering is structural, not a polling preference.
//!
//! **No borrow is ever held across an await.** Every read and write here is a whole statement
//! between two `recv`s: the guard is taken, used and dropped before the loop comes back
//! round. That is the `GenerationalBox` trap, and it is a trap rather than a rule because the
//! failure is a panic on an unrelated repaint.
//!
//! **The catalog is answered from [`ProjectState`]**, never from DataFusion introspection
//! (AGENTS.md §2): introspection hides the defs whose registration failed — precisely the rows an
//! agent most needs to see, because a table that is merely broken looks exactly like a table that
//! was never registered.
//!
//! **An agent's query sessions are not tabs**, and nothing here touches `SessionState`. That
//! is AA-03b's whole correction, and it is what makes the user's tab strip untouchable by
//! anything an agent does — see [`agents`](super::agents) for the reasoning.

use std::path::PathBuf;
use std::sync::Arc;

use freya::prelude::{spawn, use_consume, use_drop, use_hook};
use freya::radio::{use_radio, Radio};
use strata_agent::{
    AgentError, AgentId, CatalogEntry, Described, QuerySessionId, QuerySessionInfo,
    QuerySessionState, RegState,
};
use strata_core::engine::Engine;
use tokio::sync::mpsc;

use crate::agent::ask::{AgentAsk, AgentNotice};
use crate::agent::AgentCtx;
use crate::apps::project::contexts::EngineCtx;

use super::agents::{Agents, AgentsCtx, Closed};
use super::{log_event, LogCtx, LogLevel, ProjChan, ProjectState, Reg};

/// Join the service directory for this mount's lifetime and drive its asks. Call once in
/// `ProjectLoaded`, after the engine, the log, the agents satellite and the project store are
/// in place.
///
/// `name` is taken once rather than read reactively: there is no project-rename entry point
/// today, and a registration is a launch value like the recents promotion beside it. When a
/// rename lands it routes through the Project store, and this is the second reader to point
/// at it.
pub fn use_agent_bridge(agent: AgentCtx, root: PathBuf, name: String) {
    let engine = use_consume::<EngineCtx>();
    let log = use_consume::<LogCtx>();
    let agents = use_consume::<AgentsCtx>();
    // A handle, not a subscription. The driver is a task, and a task has no reactive scope,
    // so its `read`s are peek-equivalent — the same thing the diagnostics driver relies on.
    // The channel named here therefore decides nothing.
    let project = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);

    let registration = use_hook({
        let directory = Arc::clone(&agent.directory);
        move || {
            // The engine goes to the directory as a bare `Arc` — the **data plane**, called
            // from the server's own runtime, so `fetch_page` / `validate` / `functions` and
            // an agent's own run never queue behind a repaint.
            let (id, asks, notices) = directory.register(root, name, engine.arc());
            spawn(drive(asks, notices, engine, project, agents, log));
            id
        }
    });
    use_drop(move || agent.directory.deregister(registration));
}

/// The loop. Ends when both of the directory's senders are dropped, or when the scope that
/// spawned it is torn down — which are the same event seen from the two ends.
async fn drive(
    mut asks: mpsc::Receiver<AgentAsk>,
    mut notices: mpsc::UnboundedReceiver<AgentNotice>,
    engine: EngineCtx,
    project: Radio<ProjectState, ProjChan>,
    mut agents: AgentsCtx,
    log: LogCtx,
) {
    loop {
        // **Not `biased`.** Polling asks first every iteration would starve the notice
        // channel under a sustained stream of tool calls — settles never applied, agents
        // never retracted, their workspaces never cleaned up — and it buys nothing: the
        // ordering it looks like it protects is already structural, because the directory
        // awaits the `RunStarting` *reply* before it touches the engine, so a settle notice
        // cannot exist until its ask has been handled.
        tokio::select! {
            Some(ask) = asks.recv() => answer(ask, &engine, project, &mut agents, log),
            Some(notice) = notices.recv() => apply(notice, &engine, &mut agents, log),
            else => break,
        }
    }
}

/// One ask, answered. Every `send` is `let _ =`: a client that gave up mid-call drops its
/// receiver, and there is nothing to report about an answer nobody is waiting for.
fn answer(
    ask: AgentAsk,
    engine: &Engine,
    project: Radio<ProjectState, ProjChan>,
    agents: &mut AgentsCtx,
    log: LogCtx,
) {
    match ask {
        AgentAsk::Catalog(reply) => {
            let _ = reply.send(catalog(&project.read()));
        }
        AgentAsk::Describe { name, reply } => {
            let _ = reply.send(describe(&project.read(), &name));
        }
        AgentAsk::QuerySessions { agent, reply } => {
            let _ = reply.send(sessions(&agents.read(), agent, engine));
        }
        AgentAsk::OpenQuerySession { agent, reply } => {
            let session = QuerySessionId::new();
            // The satellite hands back whatever the per-agent cap displaced, so a session it
            // has stopped showing does not go on holding an engine workspace.
            let evicted = agents.write().opened(&agent, session);
            // `held`, not `agents`: this is attribution for the event log, and the assistant
            // is left out of the pane's *listing* only, never out of the record.
            let named = agents
                .read()
                .held()
                .find(|a| a.id == agent.id)
                .map(|a| a.name().to_string())
                .unwrap_or_default();
            for old in evicted {
                engine.cleanup_ws(old.into());
            }
            log_event(
                log,
                LogLevel::Info,
                format!("{named} opened a query session"),
            );
            let _ = reply.send(session);
        }
        AgentAsk::CloseQuerySession {
            agent,
            session,
            reply,
        } => {
            let closed = agents.write().closed(agent, session);
            if closed == Closed::NoSuchSession {
                let _ = reply.send(Err(AgentError::no_such_query_session(session)));
                return;
            }
            // **Always, on both arms.** The same teardown a tab close reaches through the
            // root's tab-diff effect: abort whatever is in flight and retire the workspace's
            // snapshot. `close_query_session` promises the agent that "a run still in flight
            // in it is cancelled", and that promise is about a run the engine is really
            // executing — deferring it because the satellite says `Running` would let a
            // runaway scan burn to completion with no way left to stop it, since the handle
            // stops answering here.
            //
            // What it deliberately does not do is raise the T2 confirm — that dialog asks the
            // *user* whether to destroy work, and neither answer suits a tool call. Nor does
            // it need to: this is the agent's own session, so there is nobody else's work in
            // it.
            engine.cleanup_ws(session.into());
            // `Closed::WhenItSettles` still defers the *row*, and a second `cleanup_ws` with
            // it — see `Closed`. That is the AA-03c race: this call can land between a run's
            // `RunStarting` and its `engine.query`, where the abort above finds nothing to
            // abort because the engine has not been handed the work yet, and the dispatch
            // then registers on a `WsId` nothing holds. The settle is what sweeps it.
            let _ = reply.send(Ok(()));
        }
        AgentAsk::RunStarting {
            agent,
            session,
            sql,
            reply,
        } => {
            if !agents.read().holds(agent, session) {
                let _ = reply.send(Err(AgentError::no_such_query_session(session)));
                return;
            }
            // Read before the write, because the caller has to be told which run to name
            // when it settles.
            let seq = agents.read().next_run();
            agents.write().run_started(agent, session, sql);
            let _ = reply.send(Ok(seq));
        }
    }
}

/// One notice, applied. Nothing replies and nothing can refuse: by the time either of these
/// arrives, the thing it describes has already happened.
fn apply(notice: AgentNotice, engine: &Engine, agents: &mut AgentsCtx, log: LogCtx) {
    match notice {
        AgentNotice::RunSettled {
            agent,
            session,
            seq,
            outcome,
        } => {
            // A settle that finishes a session closed mid-dispatch hands the session back,
            // and the teardown its close deferred happens here — see `Closed`.
            let retire = agents.write().run_settled(agent, session, seq, outcome);
            if let Some(session) = retire {
                engine.cleanup_ws(session.into());
            }
        }
        AgentNotice::AgentGone(agent) => {
            // **Peeked before writing.** The retraction is broadcast to every window, and
            // `State::write` notifies every subscriber *before* handing back the guard — so
            // writing first would re-render the pane and the rail badge in windows that never
            // heard of this agent, once per disconnect and once per transport schema probe.
            if !agents.peek().knows(agent) {
                return;
            }
            let released = agents.write().gone(agent);
            for session in released {
                engine.cleanup_ws(session.into());
            }
            log_event(log, LogLevel::Info, "An agent disconnected");
        }
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

/// **This agent's** query sessions, oldest first — the order the agent opened them, which is
/// also the order the pane lists them in.
///
/// Whether a session's run is still going is asked of the **engine**, not of the satellite,
/// for the reason `TabCloser` asks it there too: the engine owns the workspace and is the only
/// thing that knows what is executing in it. The satellite's own `Running` is the *record* of
/// what the driver observed and is what the pane paints; this is the state, and a tool asking
/// for it deserves the authority rather than the observation.
fn sessions(agents: &Agents, agent: AgentId, engine: &Engine) -> Vec<QuerySessionInfo> {
    agents
        // `held`, not `agents`: this answers `list_query_sessions` for the agent that asked,
        // and the assistant must see its own sessions. The pane's listing is the only thing
        // that leaves it out.
        .held()
        .find(|a| a.id == agent)
        .into_iter()
        .flat_map(|a| a.sessions.iter())
        // A session the agent has already closed is not one it holds, whatever is still
        // finishing inside it — listing a tombstone would offer back a handle every other
        // tool answers not-found for.
        .filter(|session| !session.closing)
        .map(|session| QuerySessionInfo {
            session: session.id,
            state: match session.runs.is_empty() {
                true => QuerySessionState::Empty,
                false if engine.is_running(session.id.into()) => QuerySessionState::Running,
                false => QuerySessionState::Settled,
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

/// A `State<Agents>` is not constructible outside a renderer, so the satellite's own tests
/// live beside it (`state::agents`) and these cover the two store projections plus the
/// session listing, which is the part that reaches the engine.
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use datafusion::arrow::datatypes::{DataType, Field};
    use strata_agent::{Agent, AgentIdentity};
    use strata_core::engine::{column_info, TableMeta, ViewMeta};
    use strata_core::project::ProjectDefs;
    use strata_model::{ColumnInfo, SavedQuery, SourceFormat, TableDef, TableOrigin, ViewDef};
    use uuid::Uuid;

    use super::*;

    fn column(name: &str) -> ColumnInfo {
        column_info(&Field::new(name, DataType::Int64, true))
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
                        connection: None,
                        sources: vec!["data/orders".into()],
                        partition_cols: vec![("year".into(), "Int32".into())],
                        origin: TableOrigin::External,
                    },
                    TableDef {
                        name: "gone".into(),
                        format: SourceFormat::from_name("csv"),
                        connection: None,
                        sources: vec!["missing.csv".into()],
                        partition_cols: Vec::new(),
                        origin: TableOrigin::External,
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
                ..Default::default()
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

    /// The listing is per agent, and the tri-state is real: a session that has never run is
    /// `Empty`, and one whose run has settled is `Settled`. (`Running` is the engine's own
    /// `is_running`, which this reads rather than re-derives.)
    #[test]
    fn query_sessions_are_listed_per_agent_with_their_state() {
        let engine = Engine::new(BTreeMap::new());
        let mut agents = Agents::default();
        let mine = Agent {
            id: AgentId::new(),
            identity: AgentIdentity {
                name: "claude-code".into(),
                version: "2.1.4".into(),
            },
            in_app: false,
        };
        let theirs = Agent {
            id: AgentId::new(),
            identity: AgentIdentity::default(),
            in_app: false,
        };
        let empty = QuerySessionId::new();
        let used = QuerySessionId::new();
        agents.opened(&mine, empty);
        agents.opened(&mine, used);
        agents.opened(&theirs, QuerySessionId::new());
        agents.run_started(mine.id, used, "SELECT 1".into());

        let listed = sessions(&agents, mine.id, &engine);
        assert_eq!(listed.len(), 2, "the other agent's session is not listed");
        // Oldest session first, matching the pane's own order.
        assert_eq!(listed[0].session, empty);
        assert!(matches!(listed[0].state, QuerySessionState::Empty));
        // Nothing is executing on this engine, so a session that has run has settled.
        assert_eq!(listed[1].session, used);
        assert!(matches!(listed[1].state, QuerySessionState::Settled));

        // And an agent this window has never heard of gets an empty list, not an error.
        assert!(sessions(&agents, AgentId::new(), &engine).is_empty());
    }
}
