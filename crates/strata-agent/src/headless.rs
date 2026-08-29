//! The **headless host**: the same vocabulary with the app closed.
//!
//! `strata mcp <project>` serves MCP over **stdio** — the transport for a locally-spawned
//! server, where the client owns the process, so there is no port to bind and no token to
//! present: process ownership *is* the auth. Behind it sits a plain [`Engine`] with the
//! project's registration pass replayed over it
//! ([`Catalog::sync`](strata_engine::Catalog::sync)), and the same [`StrataTools`] the in-app
//! server routes to. One vocabulary, two deployments.
//!
//! What makes this a *second host* rather than a second implementation:
//!
//! - **Registration outcomes are the catalog.** The pass's own answers, folded once at startup into
//!   the shape `ProjectState` holds in the app. Neither asks DataFusion, which would hide the
//!   failed defs.
//! - **A query session is an engine workspace and nothing else.** [`WsId`] nonces, with supersede,
//!   retire and cancel the engine's own.
//! - **One project by construction.** The process is opened *on* a project, so there is nothing to
//!   look up: every tool's `project` argument resolves to it or to nothing.
//!
//! It touches no app config, no `session.json`, no history, and writes nothing: a folder with no
//! project in it is refused rather than scaffolded, because a server the user cannot see should not
//! create the files the app owns. Running beside the live app is safe for the reason it is safe
//! between two app windows — every engine lock-claims its own snapshot directory.
//!
//! **No idle sweep here.** [`StrataTools::retire_idle`] exists for a client with no connection to
//! key on, a Streamable-HTTP condition. Over stdio there is one client whose departure closes the
//! transport, so the service value's drop is the disconnection.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rmcp::transport::stdio;
use rmcp::ServiceExt;
use strata_arrow::plan::as_explain;
use strata_core::config::Settings;
use strata_core::project::{exists_at, load_defs, ProjectDefs};
use strata_engine::register::RegOutcome;
use strata_engine::{Capability, CapabilityPolicyProvider, Engine, RunRows, RunTag, WsId};
use tokio::runtime::Builder as RuntimeBuilder;

use crate::error::AgentError;
use crate::host::{
    Agent, AgentId, CatalogEntry, Described, Host, Project, QuerySessionId, QuerySessionInfo,
    QuerySessionState, RegState, RunMode, RunSettle, Settled,
};
use crate::tools::StrataTools;

/// One query session: an engine workspace with no UI at all.
struct Session {
    agent: AgentId,
    id: QuerySessionId,
    /// Whether anything has ever run in it — `Empty` against `Settled`. Whether a run is *in
    /// flight* is asked of the engine instead, which is the only thing that knows.
    ran: bool,
    /// Runs dispatched into it whose settle has not been seen. More than one is ordinary: a second
    /// `run` supersedes the first, and both awaits are still outstanding.
    dispatched: usize,
    /// Closed while a run was in flight. The handle stops answering immediately (this is a
    /// tombstone, not a session), and the last settle sweeps the row — see
    /// [`close_query_session`](Host::close_query_session).
    closing: bool,
}

/// A [`Host`] over one project folder, its engine, and the pass that registered it.
pub struct HeadlessHost {
    project: Project,
    engine: Arc<Engine>,
    /// The catalog as the registration pass answered it, built once: nothing re-scans here,
    /// so there is no epoch and nothing to invalidate.
    catalog: Vec<CatalogEntry>,
    /// What registration *learned* per table and view — `describe_table`'s answer, and only
    /// real facts: every number in it was read by the pass.
    described: Vec<Described>,
    page_size: usize,
    /// The agent's open query sessions — **deliberately unbounded**, unlike the app's, which
    /// caps them per agent so a client cannot hold a *window's* engine open indefinitely.
    /// Here the engine is the connection's: one client
    /// owns this process, each session holds at most one snapshot (retire-on-dispatch), and
    /// all of it goes when the client does. A cap would buy nothing and would cost what the
    /// app's cap has to work to avoid — cancelling a session the agent is still using.
    sessions: Mutex<Vec<Session>>,
    /// Nonces for the runs this host dispatches. Engine-side lifecycle keys on its own
    /// dispatch id, so all a tag has to be is distinct from its neighbours'.
    runs: AtomicU64,
}

impl HeadlessHost {
    /// Open `root`: load its defs, build a plain engine, and reconcile it against them
    /// ([`Catalog::sync`](strata_engine::Catalog::sync), over the spec the defs describe).
    ///
    /// The pass runs to completion **before** anything is served, so nothing this host serves
    /// can see a half-registered catalog — and there is no second pass to race it.
    ///
    /// `Err` only for a project that cannot be read. A *def* the engine refused is not an error: it
    /// is a `failed` catalog row, exactly as in the app.
    ///
    /// The pass connects the project's object stores first, so a table over a bucket registers here
    /// exactly as it does in a window. A connection is not itself a catalog entry, so
    /// [`settled`](Self::settled) does not list one — a refused connection surfaces as the `failed`
    /// rows of the tables that needed it.
    ///
    /// The engine is built with a read-only policy ceiling, which is what
    /// [`StrataTools::run`]'s gate is narrowed against: this process has no editor and no user to
    /// ask, so the capability is stated once rather than per tool. It is a ceiling and not the
    /// whole fence — [`run`](Host::run) reads through `Workspace::query`, whose limit is the read
    /// path's own `SQLOptions`.
    pub async fn open(root: PathBuf) -> Result<HeadlessHost, String> {
        if !exists_at(&root) {
            return Err(format!(
                "No Strata project in '{}'. Open the folder in Strata once to create one.",
                root.display()
            ));
        }
        let defs = load_defs(&root)?;
        let engine = Engine::builder()
            .with_data_dir(&root)
            .with_policy(CapabilityPolicyProvider::new(Capability::read_only()))
            .build();
        let mut outcomes = Vec::new();
        engine
            .catalog()
            .sync(engine.catalog().spec(&root, &defs), |o| outcomes.push(o))
            .await;
        Ok(HeadlessHost::settled(root, defs, engine, outcomes))
    }

    /// Fold the defs and what the pass answered for each into the two listings a host serves.
    ///
    /// Driven by the **defs**, not by the outcomes: the catalog is the set of things the user
    /// wrote down, in the order `load_defs` sorted them, and an outcome is the state one of
    /// them reached. A saved query has no outcome at all — it is text the user parked, not an
    /// object the engine holds — so it is a `list_tables` row and never a `describe_table`
    /// answer.
    fn settled(
        root: PathBuf,
        defs: ProjectDefs,
        engine: Arc<Engine>,
        outcomes: Vec<RegOutcome>,
    ) -> HeadlessHost {
        let mut catalog = Vec::new();
        let mut described = Vec::new();
        for def in &defs.tables {
            let result = outcomes.iter().find_map(|o| match o {
                RegOutcome::Table { name, result } if name == &def.name => Some(result),
                _ => None,
            });
            catalog.push(CatalogEntry::Table {
                name: def.name.clone(),
                format: def.format.name().to_string(),
                sources: def.paths.clone(),
                reg: reg_state(result),
            });
            described.push(match result {
                Some(Ok(meta)) => Described::Table {
                    name: def.name.clone(),
                    format: def.format.name().to_string(),
                    sources: def.paths.clone(),
                    partitions: def.partition_cols.clone(),
                    rows: meta.rows,
                    columns: meta.columns.clone(),
                },
                Some(Err(error)) => Described::Failed {
                    name: def.name.clone(),
                    error: error.clone(),
                },
                None => Described::Pending {
                    name: def.name.clone(),
                },
            });
        }
        for def in &defs.views {
            let result = outcomes.iter().find_map(|o| match o {
                RegOutcome::View { name, result } if name == &def.name => Some(result),
                _ => None,
            });
            catalog.push(CatalogEntry::View {
                name: def.name.clone(),
                sql: def.sql.clone(),
                reg: reg_state(result),
            });
            described.push(match result {
                Some(Ok(meta)) => Described::View {
                    name: def.name.clone(),
                    sql: def.sql.clone(),
                    columns: meta.columns.clone(),
                    reads: meta.tables.clone(),
                },
                Some(Err(error)) => Described::Failed {
                    name: def.name.clone(),
                    error: error.clone(),
                },
                None => Described::Pending {
                    name: def.name.clone(),
                },
            });
        }
        catalog.extend(defs.saved_queries.iter().map(|q| CatalogEntry::Query {
            id: q.id,
            name: q.name.clone(),
            sql: q.sql.clone(),
        }));
        HeadlessHost {
            project: Project {
                name: defs.name,
                root,
            },
            engine,
            catalog,
            described,
            page_size: Settings::default().row_limit,
            sessions: Mutex::new(Vec::new()),
            runs: AtomicU64::new(0),
        }
    }

    /// A dispatch came back: release it, and sweep a session that was closed while it ran.
    ///
    /// The tombstone's other half. A close landing mid-run cannot remove the row, because a
    /// second run may still be in flight in the same workspace and the engine's teardown has
    /// to happen after the last of them — so the row waits here, and only here.
    fn dispatched_back(&self, session: QuerySessionId) {
        let sweep = {
            let mut sessions = self.sessions.lock().unwrap();
            let Some(at) = sessions.iter().position(|s| s.id == session) else {
                return;
            };
            sessions[at].dispatched = sessions[at].dispatched.saturating_sub(1);
            let done = sessions[at].closing && sessions[at].dispatched == 0;
            if done {
                sessions.remove(at);
            }
            done
        };
        if sweep {
            self.engine.ws(session.into()).cleanup();
        }
    }
}

/// [`RegState`] from what the pass answered for a def — `Reg<T>` without the payload, which is
/// what a *listing* row carries and what registration **learned** is [`Host::describe`]'s.
/// `None` is a def the pass never reached, which cannot happen here (see
/// [`HeadlessHost::settled`]) and is exactly what `Pending` means.
fn reg_state<T>(result: Option<&Result<T, String>>) -> RegState {
    match result {
        None => RegState::Pending,
        Some(Ok(_)) => RegState::Ready,
        Some(Err(error)) => RegState::Failed(error.clone()),
    }
}

impl Host for HeadlessHost {
    async fn projects(&self) -> Vec<Project> {
        vec![self.project.clone()]
    }

    fn default_page_size(&self) -> usize {
        self.page_size
    }

    async fn engine(&self, _project: &Path) -> Result<Arc<Engine>, AgentError> {
        Ok(Arc::clone(&self.engine))
    }

    async fn catalog(&self, _project: &Path) -> Result<Vec<CatalogEntry>, AgentError> {
        Ok(self.catalog.clone())
    }

    async fn describe(&self, _project: &Path, name: &str) -> Result<Described, AgentError> {
        self.described
            .iter()
            .find(|d| d.name().eq_ignore_ascii_case(name))
            .cloned()
            .ok_or_else(|| AgentError::NotFound(format!("Table or view '{name}' not found.")))
    }

    async fn query_sessions(
        &self,
        _project: &Path,
        agent: AgentId,
    ) -> Result<Vec<QuerySessionInfo>, AgentError> {
        Ok(self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.agent == agent && !s.closing)
            .map(|s| QuerySessionInfo {
                session: s.id,
                state: match s.ran {
                    false => QuerySessionState::Empty,
                    true if self.engine.ws(s.id.into()).is_running() => QuerySessionState::Running,
                    true => QuerySessionState::Settled,
                },
            })
            .collect())
    }

    /// The agent's [`AgentIdentity`](crate::AgentIdentity) is deliberately dropped here: it
    /// exists to name the caller of a tool where a surface has to, and headless there is no
    /// such surface and exactly one client. What matters — that a session belongs to the
    /// agent that opened it — is the id, which every method below matches on.
    async fn open_query_session(
        &self,
        _project: &Path,
        agent: &Agent,
    ) -> Result<QuerySessionId, AgentError> {
        let session = QuerySessionId::new();
        self.sessions.lock().unwrap().push(Session {
            agent: agent.id,
            id: session,
            ran: false,
            dispatched: 0,
            closing: false,
        });
        Ok(session)
    }

    async fn close_query_session(
        &self,
        _project: &Path,
        agent: AgentId,
        session: QuerySessionId,
    ) -> Result<(), AgentError> {
        {
            let mut sessions = self.sessions.lock().unwrap();
            let Some(at) = sessions
                .iter()
                .position(|s| s.agent == agent && s.id == session && !s.closing)
            else {
                return Err(AgentError::no_such_query_session(session));
            };
            match sessions[at].dispatched {
                0 => {
                    sessions.remove(at);
                }
                _ => sessions[at].closing = true,
            }
        }
        self.engine.ws(session.into()).cleanup();
        Ok(())
    }

    async fn run(
        &self,
        _project: &Path,
        agent: AgentId,
        session: QuerySessionId,
        sql: String,
        mode: RunMode,
        page_size: usize,
    ) -> Result<RunSettle, AgentError> {
        {
            let mut sessions = self.sessions.lock().unwrap();
            let Some(open) = sessions
                .iter_mut()
                .find(|s| s.agent == agent && s.id == session && !s.closing)
            else {
                return Err(AgentError::no_such_query_session(session));
            };
            open.ran = true;
            open.dispatched += 1;
        }

        let ws = WsId::from(session);
        let tag = RunTag(self.runs.fetch_add(1, Ordering::Relaxed) as u128);
        let settled = match mode {
            RunMode::Run => self
                .engine
                .ws(ws)
                .query(tag, sql, page_size)
                .await
                .map(|RunRows { output, .. }| Settled::Rows(output)),
            RunMode::Explain => self
                .engine
                .ws(ws)
                .explain(tag, as_explain(&sql, false))
                .await
                .map(Settled::Plan),
        };
        self.dispatched_back(session);
        Ok(settled)
    }

    /// Sync and non-blocking as the trait requires: a short mutex and the engine's own
    /// teardown, with nothing awaited — which is what a `Drop` on the transport's runtime can
    /// afford.
    fn agent_gone(&self, agent: AgentId) {
        let released: Vec<QuerySessionId> = {
            let Ok(mut sessions) = self.sessions.lock() else {
                return;
            };
            let released = sessions
                .iter()
                .filter(|s| s.agent == agent)
                .map(|s| s.id)
                .collect();
            sessions.retain(|s| s.agent != agent);
            released
        };
        for session in released {
            self.engine.ws(session.into()).cleanup();
        }
    }
}

/// Open `root` and serve it over stdio until the client goes away — the whole of
/// `strata mcp <project>` below the argument parsing.
///
/// **Blocking, and it owns its runtime.** The caller is `main` with no executor of its own
/// (the engine's is private and the app's UI thread is not one), so the runtime is built
/// here, used for rmcp and the transport, and dropped when the client disconnects. Two
/// workers, matching the in-app server: nothing here is CPU-bound — the engine does the work
/// on its own threads.
///
/// **stdout belongs to the transport.** Anything written to it that is not MCP framing is a
/// protocol error at the client, so a caller must have its logging pointed at stderr before
/// this is reached.
pub fn serve_stdio(root: PathBuf) -> Result<(), String> {
    let rt = RuntimeBuilder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("strata-mcp")
        .build()
        .map_err(|e| format!("agent stdio server runtime: {e}"))?;
    let served = rt.block_on(async {
        let host = HeadlessHost::open(root).await?;
        tracing::info!(
            "agent access serving '{}' over stdio",
            host.project.root.display()
        );
        let service = StrataTools::new(Arc::new(host))
            .serve(stdio())
            .await
            .map_err(|e| format!("agent stdio server: {e}"))?;
        let reason = service
            .waiting()
            .await
            .map_err(|e| format!("agent stdio server: {e}"))?;
        tracing::info!("agent access over stdio ended: {reason:?}");
        Ok(())
    });
    rt.shutdown_background();
    served
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::{env, process};

    use strata_core::project::save_defs;
    use strata_model::{SourceFormat, TableDef, TableOrigin, ViewDef};

    use strata_engine::register::{CatalogSpec, RegKind};

    use crate::host::AgentIdentity;

    use super::*;
    use strata_engine::SourceDefs;

    /// A scratch project folder of our own, per test — `tag` is load-bearing for the reason
    /// `strata-engine`'s own helper says: these run concurrently in one process and DataFusion
    /// re-LISTs a table's sources at scan time.
    fn scratch(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata_headless_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn table(name: &str, source: &str) -> TableDef {
        TableDef {
            name: name.into(),
            format: SourceFormat::from_name("csv"),
            source: None,
            paths: vec![source.into()],
            partition_cols: Vec::new(),
            origin: TableOrigin::External,
        }
    }

    /// A project holding one good table, one whose source is missing, and a view over the
    /// good one — the three states a catalog listing has to tell apart.
    async fn project(tag: &str) -> (PathBuf, HeadlessHost) {
        let root = scratch(tag);
        fs::write(root.join("people.csv"), "id,name\n1,ana\n2,ben\n3,cara\n").unwrap();
        save_defs(
            &root,
            &ProjectDefs {
                name: "sales".into(),
                sources: Vec::new(),
                tables: vec![table("people", "people.csv"), table("gone", "missing.csv")],
                views: vec![ViewDef {
                    name: "adults".into(),
                    sql: "SELECT id FROM people".into(),
                }],
                saved_queries: Vec::new(),
            },
        )
        .unwrap();
        let host = HeadlessHost::open(root.clone()).await.unwrap();
        (root, host)
    }

    fn agent() -> Agent {
        Agent {
            id: AgentId::new(),
            identity: AgentIdentity {
                name: "claude-code".into(),
                version: "2.1.4".into(),
            },
            in_app: false,
        }
    }

    /// **Registration outcomes are the catalog**: a def the engine refused is a row with its
    /// error, not a missing row — the same answer the app's store projection gives.
    #[tokio::test]
    async fn the_catalog_is_what_the_registration_pass_answered() {
        let (root, host) = project("catalog").await;

        let entries = host.catalog(&root).await.unwrap();

        match &entries[..] {
            [CatalogEntry::Table {
                name: failed,
                reg: RegState::Failed(why),
                ..
            }, CatalogEntry::Table {
                name: ready,
                reg: RegState::Ready,
                sources,
                ..
            }, CatalogEntry::View {
                name: view,
                reg: RegState::Ready,
                ..
            }] => {
                assert_eq!(failed, "gone");
                assert!(why.contains("missing.csv"), "{why}");
                assert_eq!(ready, "people");
                assert_eq!(sources, &vec!["people.csv".to_string()]);
                assert_eq!(view, "adults");
            }
            other => panic!("{other:?}"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// A defs file that shrank leaves no ghost: what the spec stops naming stops resolving.
    ///
    /// Driven through the host rather than only the engine because that is where a client would
    /// notice — a table nothing defines must not still answer a query. Note what is *not*
    /// removed: `gone` never registered, so there is nothing to take out and no outcome for it.
    #[tokio::test]
    async fn a_replay_over_a_shrunk_catalog_leaves_no_ghost() {
        let (root, host) = project("shrunk").await;
        let RunRows { output: before, .. } = host
            .engine
            .ws(WsId(1))
            .query(RunTag(1), "SELECT id FROM adults".into(), 10)
            .await
            .expect("the view the first pass created");
        assert_eq!(before.total, 3);

        let mut outcomes = Vec::new();
        host.engine
            .catalog()
            .sync(CatalogSpec::default(), |o| outcomes.push(o))
            .await;

        let removed: Vec<(&str, RegKind)> = outcomes
            .iter()
            .filter_map(|o| match o {
                RegOutcome::Removed { name, kind } => Some((name.as_str(), *kind)),
                _ => None,
            })
            .collect();
        assert_eq!(
            removed,
            vec![("adults", RegKind::View), ("people", RegKind::Table)],
            "the view goes before the table it reads, and the def that never registered is not \
             a removal: {outcomes:?}"
        );

        let refused = host
            .engine
            .ws(WsId(1))
            .query(RunTag(2), "SELECT id FROM people".into(), 10)
            .await
            .expect_err("a table no def names is not queryable");
        let refused = refused.to_string();
        assert!(refused.contains("people"), "{refused}");

        let _ = fs::remove_dir_all(&root);
    }

    /// The other half of the same contract: what the spec does name is registered, so one call
    /// is the whole replay.
    #[tokio::test]
    async fn a_replay_registers_what_the_spec_still_names() {
        let (root, host) = project("replay").await;
        let defs = load_defs(&root).expect("defs");
        let known = SourceDefs::of(&defs.sources);
        let desired = CatalogSpec {
            sources: defs.sources.clone(),
            tables: defs
                .tables
                .iter()
                .filter(|def| def.name == "people")
                .map(|def| host.engine.catalog().table_spec(&root, def, &known))
                .collect(),
            views: Vec::new(),
        };

        let mut outcomes = Vec::new();
        let report = host
            .engine
            .catalog()
            .sync(desired, |o| outcomes.push(o))
            .await;

        assert!(
            outcomes.iter().any(|o| matches!(
                o,
                RegOutcome::Table { name, result: Ok(_) } if name == "people"
            )),
            "{outcomes:?}"
        );
        assert_eq!(report.generation, host.engine.catalog().generation());
        let RunRows { output, .. } = host
            .engine
            .ws(WsId(1))
            .query(RunTag(1), "SELECT id FROM people".into(), 10)
            .await
            .expect("the table the replay re-registered");
        assert_eq!(output.total, 3);

        let _ = fs::remove_dir_all(&root);
    }

    /// `describe_table` reports what registration read, per kind — and a name nothing owns is
    /// a plain not-found, `list_tables` being the recovery.
    #[tokio::test]
    async fn describe_reports_what_registration_read() {
        let (root, host) = project("describe").await;

        let Described::Table { rows, columns, .. } = host.describe(&root, "people").await.unwrap()
        else {
            panic!("the ready table describes as a table");
        };
        assert_eq!(
            columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["id", "name"]
        );
        assert_eq!(rows, None);

        let Described::View { reads, .. } = host.describe(&root, "adults").await.unwrap() else {
            panic!("the view describes as a view");
        };
        assert_eq!(reads, vec!["people".to_string()]);

        assert!(matches!(
            host.describe(&root, "gone").await.unwrap(),
            Described::Failed { .. }
        ));
        assert!(matches!(
            host.describe(&root, "nope").await,
            Err(AgentError::NotFound(_))
        ));
        let _ = fs::remove_dir_all(&root);
    }

    /// A run is a real execution against the query session's own workspace, and the session
    /// reports the states in order: nothing run, then settled.
    #[tokio::test]
    async fn a_run_executes_in_its_query_sessions_workspace() {
        let (root, host) = project("run").await;
        let who = agent();
        let session = host.open_query_session(&root, &who).await.unwrap();

        assert!(matches!(
            host.query_sessions(&root, who.id).await.unwrap()[..],
            [QuerySessionInfo {
                state: QuerySessionState::Empty,
                ..
            }]
        ));

        let settled = host
            .run(
                &root,
                who.id,
                session,
                "SELECT id FROM people ORDER BY id".into(),
                RunMode::Run,
                2,
            )
            .await
            .unwrap();
        let Ok(Settled::Rows(output)) = settled else {
            panic!("{settled:?}");
        };
        assert_eq!(output.rows.len(), 2);
        assert_eq!(output.total, 3);
        assert!(matches!(
            host.query_sessions(&root, who.id).await.unwrap()[..],
            [QuerySessionInfo {
                state: QuerySessionState::Settled,
                ..
            }]
        ));
        let _ = fs::remove_dir_all(&root);
    }

    /// **An agent addresses its own sessions and nothing else**, and another agent's handle
    /// gets the answer a made-up one does — never "that is not yours", which would confirm
    /// the session exists.
    #[tokio::test]
    async fn an_agent_reaches_only_its_own_query_sessions() {
        let (root, host) = project("scoping").await;
        let mine = agent();
        let theirs = agent();
        let session = host.open_query_session(&root, &mine).await.unwrap();
        host.open_query_session(&root, &theirs).await.unwrap();

        assert_eq!(
            host.query_sessions(&root, theirs.id).await.unwrap().len(),
            1
        );
        assert!(matches!(
            host.run(
                &root,
                theirs.id,
                session,
                "SELECT 1".into(),
                RunMode::Run,
                10
            )
            .await,
            Err(AgentError::NotFound(_))
        ));
        assert!(matches!(
            host.close_query_session(&root, theirs.id, session).await,
            Err(AgentError::NotFound(_))
        ));
        let _ = fs::remove_dir_all(&root);
    }

    /// Closing tears the workspace down and the handle stops answering; a connection ending
    /// takes every session with it, which is the only teardown stdio actually needs.
    #[tokio::test]
    async fn closing_and_disconnecting_both_release_a_session() {
        let (root, host) = project("teardown").await;
        let who = agent();
        let closed = host.open_query_session(&root, &who).await.unwrap();
        let dropped = host.open_query_session(&root, &who).await.unwrap();

        host.close_query_session(&root, who.id, closed)
            .await
            .unwrap();
        assert!(matches!(
            host.close_query_session(&root, who.id, closed).await,
            Err(AgentError::NotFound(_))
        ));
        assert_eq!(
            host.query_sessions(&root, who.id).await.unwrap().len(),
            1,
            "the other one is untouched"
        );

        host.agent_gone(who.id);
        assert!(host.query_sessions(&root, who.id).await.unwrap().is_empty());
        assert!(matches!(
            host.run(&root, who.id, dropped, "SELECT 1".into(), RunMode::Run, 10)
                .await,
            Err(AgentError::NotFound(_))
        ));
        let _ = fs::remove_dir_all(&root);
    }

    /// **A close racing a dispatch is a tombstone.** The handle stops answering at once and
    /// the engine is aborted at once, but the *row* waits for the last settle: a close can
    /// land between `run`'s ownership check and `engine.query`, where the teardown finds
    /// nothing to tear down and the dispatch then registers a snapshot on a workspace no
    /// later close could name again. Driven here rather than raced, because the state under
    /// test is exactly the one `run` leaves behind when it releases the lock to await.
    #[tokio::test]
    async fn a_close_during_a_dispatch_waits_for_the_settle_to_release_the_row() {
        let (root, host) = project("tombstone").await;
        let who = agent();
        let session = host.open_query_session(&root, &who).await.unwrap();
        host.sessions.lock().unwrap()[0].dispatched = 1;

        host.close_query_session(&root, who.id, session)
            .await
            .unwrap();

        assert!(
            host.query_sessions(&root, who.id).await.unwrap().is_empty(),
            "the handle stops answering at once"
        );
        assert!(
            matches!(
                host.run(&root, who.id, session, "SELECT 1".into(), RunMode::Run, 10)
                    .await,
                Err(AgentError::NotFound(_))
            ),
            "and so does every other tool"
        );
        assert_eq!(
            host.sessions.lock().unwrap().len(),
            1,
            "but the row outlives the close, because the settle still has to sweep it"
        );

        host.dispatched_back(session);

        assert!(
            host.sessions.lock().unwrap().is_empty(),
            "which the last settle does"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// `explain` means "plan this statement", never "the caller already wrote EXPLAIN" and
    /// never `analyze` — which would execute the very query the caller was avoiding.
    #[tokio::test]
    async fn explain_mode_plans_the_statement_without_executing_it() {
        let (root, host) = project("explain").await;
        let who = agent();
        let session = host.open_query_session(&root, &who).await.unwrap();

        let settled = host
            .run(
                &root,
                who.id,
                session,
                "SELECT id FROM people".into(),
                RunMode::Explain,
                10,
            )
            .await
            .unwrap();

        let Ok(Settled::Plan(plan)) = settled else {
            panic!("{settled:?}");
        };
        assert!(!plan.analyze, "a plan must not run what it is planning");
        assert!(plan.is_some(), "and it has trees to show: {plan:?}");
        let _ = fs::remove_dir_all(&root);
    }

    /// A folder with no project is **refused**, not scaffolded: this host writes nothing to
    /// the project, and a server the user cannot see creating the files the app owns is the
    /// one surprise that would be hard to undo.
    #[tokio::test]
    async fn a_folder_with_no_project_is_refused() {
        let root = scratch("empty");

        let Err(error) = HeadlessHost::open(root.clone()).await else {
            panic!("a folder with no project in it is not a project");
        };

        assert!(error.contains("No Strata project"), "{error}");
        assert!(
            !root.join(".strata").exists(),
            "and nothing was written into it"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// The page size is the app's shipped default, reached without opening app config — the
    /// zero that means "no limit" is the tool layer's to resolve, so nothing is normalized
    /// here.
    #[tokio::test]
    async fn the_default_page_size_is_the_shipped_setting() {
        let (root, host) = project("page_size").await;
        assert_eq!(host.default_page_size(), Settings::default().row_limit);
        let _ = fs::remove_dir_all(&root);
    }
}
