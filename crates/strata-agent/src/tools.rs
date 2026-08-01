//! The **vocabulary** — the ten read-only tools of `docs/AGENT_ACCESS_SPEC.md` §5, over a
//! [`Host`].
//!
//! [`StrataTools`] is the rmcp `ServerHandler`, and it is deliberately transport-free: the
//! Streamable-HTTP server ([`crate::server`]) serves it, the headless host (AA-05) will
//! serve the same value over stdio, and the chat pane (AA-06) will call it in-process. One
//! surface, three frontends.
//!
//! Three rules are enforced here and nowhere else, because here is the only place they can
//! be kept honest:
//!
//! - **The policy gate runs before dispatch.** `Engine::query` does not enforce the
//!   managed-DDL policy — the editor simply never dispatches what validation flagged, and
//!   an agent cannot be trusted with that discipline. So `run` asks
//!   `Engine::policy_verdicts` (AA-01's export of the editor's own predicate) and refuses on
//!   any non-clean answer, including an unjudgeable one: the gate fails closed.
//! - **A stop is not a fault.** `strata_core::engine::stopped_on_purpose` is asked once,
//!   here, and its three strings become [`RunResult::Stopped`] rather than an error.
//! - **`run` never rewrites SQL.** No injected `LIMIT`: the press materializes exactly what
//!   a person's would, and the *response* is bounded by `page_size` plus `read_page`.
//!
//! ## One value per client connection (AA-03b)
//!
//! A [`StrataTools`] *is* one agent: it carries a [`Connection`], which mints the
//! [`AgentId`] every session-scoped call is made under and retracts it on drop. The
//! transport asks for one per client ([`StrataTools::connection`]) and every session-scoped
//! answer is then scoped by construction rather than by a check somebody has to remember —
//! which is the AA-03 hole restated as a type: an agent has no handle on another agent's
//! work, nor on the user's tabs, because it never receives one.
//!
//! `Clone` deliberately *shares* the connection (the transport clones one service across a
//! session's requests); `connection()` is the only thing that starts a new agent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::service::Peer;
use rmcp::{tool, tool_handler, tool_router, RoleServer, ServerHandler};
use strata_core::engine::{stopped_on_purpose, Engine};
use strata_model::SnapshotId;
use uuid::Uuid;

use crate::error::AgentError;
use crate::host::{
    self, Agent, AgentId, AgentIdentity, Host, Project, QuerySessionId, RunMode, Settled,
};
use crate::wire::{
    cells, columns, plan_result, rows_result, Columns, DescribeResult, DescribeTableParams,
    DiagnosticWire, EntryWire, FunctionsResult, PageResult, ProjectParams, ProjectsResult,
    QuerySessionParams, QuerySessionResult, QuerySessionsResult, ReadPageParams, RunParams,
    RunResult, TablesResult, ValidateParams, ValidateResult,
};

/// The most rows one call will hand back, however large a `page_size` is asked for. A cap
/// rather than an error: the response reports the `page_size` actually used, so the clamp is
/// visible in the answer rather than a silent truncation. It is also what a
/// [`Host::default_page_size`] of `0` ("no limit", the app's own reading of that setting)
/// resolves to.
pub const MAX_PAGE_SIZE: usize = 10_000;

/// How many query sessions' results stay readable at once, **per agent**, across projects.
///
/// The cache has no other bound: `close_query_session` and a superseded read drop an entry,
/// but an agent that simply stops calling — or a window that closes under it — is a thing
/// this layer never hears about, so the entry would sit there for the life of the process
/// holding a whole result schema. The oldest is evicted once past this, which costs nothing
/// an agent notices: a `read_page` on an evicted session reports the same "run a query in it
/// first" as one on a session that never ran, and re-running is the recovery either way. Per
/// agent rather than global, so one client's chatter cannot discard another's live result.
const MAX_REMEMBERED_RUNS: usize = 64;

/// What the server remembers about a query session's last settled run, so `read_page` can
/// page it.
///
/// A cache of a fact the settle carried, not a second source of truth: the snapshot it names
/// is retired by the next run in that session, and then the read fails cleanly with "the
/// result was replaced". Deliberately **not** a `SnapshotPin` — pinning would keep a result
/// alive past the run that owns it, which is right for an export window and wrong here,
/// where the honest answer is that the session has moved on.
#[derive(Clone, Debug)]
struct LastRun {
    /// `None` when the query returned no rows — nothing was materialized, so there is
    /// nothing to page, and the honest answer is an empty page rather than a fault.
    snapshot: Option<SnapshotId>,
    /// **Which engine minted that snapshot.** Ids are a per-engine counter that restarts at
    /// 1, and an engine restart remounts the project at the same root — so without this a
    /// remembered `SnapshotId(1)` would silently resolve against the *new* engine's first
    /// snapshot, handing the agent whatever the user has since run under its own stale
    /// columns and total.
    engine: u64,
    /// The wire columns, converted **once** and shared by every response that describes this
    /// result. A schema can carry thousands of nested fields, so re-deriving it per page — or
    /// keeping a second `Vec<ColumnInfo>` beside it to re-derive *from* — is per-field
    /// recursive work paid for nothing.
    columns: Columns,
    total: usize,
    page_size: usize,
    /// Insertion order, for the eviction above. A counter rather than a timestamp: this has
    /// to order two entries, not date them.
    seq: u64,
}

/// One client connection: the agent it is, and the retraction that ends it.
///
/// **RAII, for the same reason `SnapshotPin` and `AgentServer` are** — a connection ending
/// is not an event anything on our side gets told about, so the only honest place to notice
/// is the drop of the value the transport owns for that session's whole life. There is no
/// `disconnect()` to forget to call and no path that can skip it.
///
/// A connection that never opened a query session retracts an [`AgentId`] no host has heard
/// of, which removes nothing — so a probe instance (the transport builds one to read a tool
/// schema) costs a no-op rather than needing a flag to suppress it.
struct Connection<H: Host> {
    host: Arc<H>,
    agent: AgentId,
}

impl<H: Host> Drop for Connection<H> {
    fn drop(&mut self) {
        self.host.agent_gone(self.agent);
    }
}

/// The tool vocabulary over one [`Host`], **as one agent**.
pub struct StrataTools<H: Host> {
    host: Arc<H>,
    /// Keyed by `(agent, project root, query session)`. The agent is in the key rather than
    /// checked against it: a cache shared by every connection would otherwise let a handle
    /// that leaked between two agents read the other's rows, and a key is a check that
    /// cannot be forgotten.
    runs: Arc<Mutex<HashMap<(AgentId, PathBuf, QuerySessionId), LastRun>>>,
    /// Stamps each remembered run so the oldest can be found. Shared with every clone of the
    /// service, like the map it orders.
    seq: Arc<AtomicU64>,
    connection: Arc<Connection<H>>,
}

// A manual `Clone`: the derive would demand `H: Clone`, and the whole point of the `Arc` is
// that the host is shared, never copied. A clone is the **same** agent — see the module
// note; `connection()` is what starts a new one.
impl<H: Host> Clone for StrataTools<H> {
    fn clone(&self) -> Self {
        StrataTools {
            host: Arc::clone(&self.host),
            runs: Arc::clone(&self.runs),
            seq: Arc::clone(&self.seq),
            connection: Arc::clone(&self.connection),
        }
    }
}

impl<H: Host> StrataTools<H> {
    /// The vocabulary over `host`, as one agent — what an in-process caller (the chat pane,
    /// AA-06) holds directly, and what a transport clones connections from.
    pub fn new(host: Arc<H>) -> Self {
        StrataTools {
            connection: Arc::new(Connection {
                host: Arc::clone(&host),
                agent: AgentId::new(),
            }),
            host,
            runs: Arc::new(Mutex::new(HashMap::new())),
            seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The same vocabulary for a **new** client: a fresh [`AgentId`], the shared host and
    /// the shared run cache. This is what a transport's per-session service factory calls.
    pub fn connection(&self) -> Self {
        StrataTools {
            host: Arc::clone(&self.host),
            runs: Arc::clone(&self.runs),
            seq: Arc::clone(&self.seq),
            connection: Arc::new(Connection {
                host: Arc::clone(&self.host),
                agent: AgentId::new(),
            }),
        }
    }

    /// **Open a query session** — the semantic call, with no MCP peer in it.
    ///
    /// The `#[tool]` wrapper below does one thing this does not: read the client's
    /// `clientInfo` off the peer. Everything else about opening a session is here, so the
    /// in-process caller AA-06 will be — a chat pane with a name of its own and no MCP
    /// connection anywhere — introduces itself by calling this, rather than needing a
    /// transport it has no use for.
    pub async fn open_session(
        &self,
        identity: AgentIdentity,
        project: Option<&str>,
    ) -> Result<QuerySessionId, AgentError> {
        let project = self.project(project).await?;
        let agent = Agent {
            id: self.connection.agent,
            identity,
        };
        self.host.open_query_session(&project.root, &agent).await
    }

    /// What a client said it was at `initialize`, or a blank identity if it has not said.
    ///
    /// A blank is rendered honestly by the surface that shows it rather than refused here: a
    /// client is not obliged to introduce itself well, and losing its whole session over a
    /// missing name would be the app punishing the user for the client's manners.
    fn identity(peer: &Peer<RoleServer>) -> AgentIdentity {
        peer.peer_info()
            .map(|info| AgentIdentity {
                name: info.client_info.name.clone(),
                version: info.client_info.version.clone(),
            })
            .unwrap_or_default()
    }

    /// The one resolution path every project-scoped tool takes.
    async fn project(&self, want: Option<&str>) -> Result<Project, AgentError> {
        host::resolve(self.host.projects().await, want)
    }

    /// A project plus its engine — the pair every data-plane tool needs.
    async fn engine(&self, want: Option<&str>) -> Result<(Project, Arc<Engine>), AgentError> {
        let project = self.project(want).await?;
        let engine = self.host.engine(&project.root).await?;
        Ok((project, engine))
    }

    fn key(&self, root: &Path, session: QuerySessionId) -> (AgentId, PathBuf, QuerySessionId) {
        (self.connection.agent, root.to_path_buf(), session)
    }

    fn remember(
        &self,
        root: &Path,
        session: QuerySessionId,
        snapshot: Option<SnapshotId>,
        engine: u64,
        columns: Columns,
        total: usize,
        page_size: usize,
    ) {
        let agent = self.connection.agent;
        let mut runs = self.runs.lock().unwrap();
        runs.insert(
            self.key(root, session),
            LastRun {
                snapshot,
                engine,
                columns,
                total,
                page_size,
                seq: self.seq.fetch_add(1, Ordering::Relaxed),
            },
        );
        // **Bounded per agent, not globally.** The map is shared by every connection, so a
        // global cap would let one chatty client evict a peer's still-readable result — a
        // cross-agent effect in a vocabulary whose whole point is that an agent cannot reach
        // another's work, and one that reports itself as "run a query in it first" for a
        // query that *was* run.
        while runs.keys().filter(|(a, _, _)| *a == agent).count() > MAX_REMEMBERED_RUNS {
            let Some(oldest) = runs
                .iter()
                .filter(|((a, _, _), _)| *a == agent)
                .min_by_key(|(_, run)| run.seq)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            runs.remove(&oldest);
        }
    }

    fn forget(&self, root: &Path, session: QuerySessionId) {
        self.runs.lock().unwrap().remove(&self.key(root, session));
    }

    fn recall(&self, root: &Path, session: QuerySessionId) -> Option<LastRun> {
        self.runs
            .lock()
            .unwrap()
            .get(&self.key(root, session))
            .cloned()
    }
}

/// A handle is a [`QuerySessionId`]'s `Uuid` as text. Anything else never named a session, so
/// it gets the same answer an expired handle does — `list_query_sessions` is the recovery
/// either way.
fn session_handle(text: &str) -> Result<QuerySessionId, AgentError> {
    Uuid::parse_str(text)
        .map(QuerySessionId)
        // The wording is `AgentError::no_such_query_session`'s, but the handle never parsed,
        // so there is no id to hand it — the text the caller sent is what has to be echoed.
        .map_err(|_| AgentError::NotFound(format!("No open query session '{text}'.")))
}

#[tool_router]
impl<H: Host> StrataTools<H> {
    /// List the open Strata projects: name and root folder. Every other tool takes an
    /// optional 'project' naming one of these, needed only when more than one is open.
    #[tool(annotations(read_only_hint = true))]
    async fn list_projects(&self) -> Json<ProjectsResult> {
        Json(ProjectsResult {
            projects: self
                .host
                .projects()
                .await
                .into_iter()
                .map(Into::into)
                .collect(),
        })
    }

    /// List a project's catalog: registered tables, saved views and saved queries, each with
    /// its source and whether the engine accepted it. This is the catalog as the app shows
    /// it, so a def the engine refused is listed with its error rather than silently missing.
    #[tool(annotations(read_only_hint = true))]
    async fn list_tables(
        &self,
        Parameters(params): Parameters<ProjectParams>,
    ) -> Result<Json<TablesResult>, AgentError> {
        let project = self.project(params.project.as_deref()).await?;
        let entries = self.host.catalog(&project.root).await?;
        Ok(Json(TablesResult {
            entries: entries.into_iter().map(EntryWire::from).collect(),
        }))
    }

    /// Describe one table or view: its columns and types, nested fields, Hive partition
    /// columns, source paths and format, plus the row count and column statistics the source
    /// reports for free. Only facts that were read — nothing is scanned or estimated.
    #[tool(annotations(read_only_hint = true))]
    async fn describe_table(
        &self,
        Parameters(params): Parameters<DescribeTableParams>,
    ) -> Result<Json<DescribeResult>, AgentError> {
        let project = self.project(params.project.as_deref()).await?;
        let described = self.host.describe(&project.root, &params.name).await?;
        Ok(Json(DescribeResult::from(described)))
    }

    /// List the SQL functions this project's engine has registered: names, overload
    /// signatures and documentation. What is registered is what exists.
    #[tool(annotations(read_only_hint = true))]
    async fn list_functions(
        &self,
        Parameters(params): Parameters<ProjectParams>,
    ) -> Result<Json<FunctionsResult>, AgentError> {
        let (_, engine) = self.engine(params.project.as_deref()).await?;
        Ok(Json(FunctionsResult::from(engine.functions())))
    }

    /// Check SQL without running it: lints, the read-only policy, and a dry plan against the
    /// real catalog. The cheap way to find a typo or a missing column before spending a run.
    #[tool(annotations(read_only_hint = true))]
    async fn validate(
        &self,
        Parameters(params): Parameters<ValidateParams>,
    ) -> Result<Json<ValidateResult>, AgentError> {
        let (_, engine) = self.engine(params.project.as_deref()).await?;
        let diagnostics = engine.validate(params.sql).await;
        Ok(Json(ValidateResult {
            diagnostics: diagnostics.iter().map(DiagnosticWire::from).collect(),
        }))
    }

    /// Open a query session and return its handle: a place your queries run in sequence,
    /// each replacing the last. It is yours, not one of the user's editor tabs — nothing you
    /// do here disturbs what they are working on. The user watches your sessions in the
    /// Agents pane and can promote any query you ran into their own editor.
    #[tool]
    async fn open_query_session(
        &self,
        peer: Peer<RoleServer>,
        Parameters(params): Parameters<ProjectParams>,
    ) -> Result<Json<QuerySessionResult>, AgentError> {
        let session = self
            .open_session(Self::identity(&peer), params.project.as_deref())
            .await?;
        Ok(Json(QuerySessionResult {
            query_session: session.0.to_string(),
        }))
    }

    /// List your own query sessions in this project: handle, and whether a run is in flight,
    /// settled, or has never happened.
    #[tool(annotations(read_only_hint = true))]
    async fn list_query_sessions(
        &self,
        Parameters(params): Parameters<ProjectParams>,
    ) -> Result<Json<QuerySessionsResult>, AgentError> {
        let project = self.project(params.project.as_deref()).await?;
        let sessions = self
            .host
            .query_sessions(&project.root, self.connection.agent)
            .await?;
        Ok(Json(QuerySessionsResult {
            query_sessions: sessions.into_iter().map(Into::into).collect(),
        }))
    }

    /// Run read-only SQL in one of your query sessions and wait for it to settle. It runs on
    /// the project's real engine, so it costs and behaves exactly like a query the user
    /// presses Run on, and it replaces whatever that session last produced. Returns page 1
    /// plus the exact total; use read_page for the rest. Set mode to 'explain' for the query
    /// plan without executing.
    #[tool]
    async fn run(
        &self,
        Parameters(params): Parameters<RunParams>,
    ) -> Result<Json<RunResult>, AgentError> {
        let session = session_handle(&params.query_session)?;
        let (project, engine) = self.engine(params.project.as_deref()).await?;

        // Nothing to run is refused before anything else, because the gate below cannot catch
        // it: a blank statement parses to *zero* statements, so it draws zero refusals and
        // reads as clean. Dispatching it would leave the user a failed run they did not make.
        // The editor's own funnel says the same thing (`actions::press_query`: a blank buffer
        // never runs); this is that rule where the agent path can reach it.
        if params.sql.trim().is_empty() {
            return Err(AgentError::Query("The query is empty.".into()));
        }

        // The gate, before dispatch. `Err` is "could not judge" — unparseable input is never
        // a policy pass, so it is refused here with the engine's own parse wording rather
        // than sent on to fail downstream.
        match engine.policy_verdicts(params.sql.clone()).await {
            Err(e) => return Err(AgentError::Query(e)),
            Ok(refusals) if !refusals.is_empty() => return Err(AgentError::Policy(refusals)),
            Ok(_) => {}
        }

        let mode = RunMode::from(params.mode.unwrap_or_default());
        // A `0` from the host is the app's "no limit", not a request for empty pages — see
        // `Host::default_page_size`. A `0` the *caller* asked for is nothing at all, and the
        // clamp's floor answers it with one row.
        let page_size = match params.page_size {
            Some(asked) => asked.clamp(1, MAX_PAGE_SIZE),
            None => match self.host.default_page_size() {
                0 => MAX_PAGE_SIZE,
                limit => limit.min(MAX_PAGE_SIZE),
            },
        };

        let settled = self
            .host
            .run(
                &project.root,
                self.connection.agent,
                session,
                params.sql,
                mode,
                page_size,
            )
            .await?;
        // **After** the dispatch, never before: a run refused at the ownership gate (or lost
        // to a window that went) never retired anything, so forgetting first would throw away
        // the page of a result that is still there to read. An explain materializes nothing
        // and leaves the previous result alone either way.
        if mode == RunMode::Run {
            self.forget(&project.root, session);
        }
        let handle = params.query_session;
        match settled {
            Ok(Settled::Rows(output)) => {
                // Converted once and shared: the response and every later page describe the
                // same schema, so nothing re-walks it.
                let cols = columns(&output.columns);
                self.remember(
                    &project.root,
                    session,
                    output.snapshot,
                    engine.id(),
                    Columns::clone(&cols),
                    output.total,
                    output.page_size,
                );
                Ok(Json(rows_result(handle, cols, output)))
            }
            Ok(Settled::Plan(plan)) => Ok(Json(plan_result(handle, plan))),
            // The one place stopped-vs-failed is judged.
            Err(e) if stopped_on_purpose(&e) => Ok(Json(RunResult::Stopped {
                query_session: handle,
                reason: e,
            })),
            Err(e) => Err(AgentError::Query(e)),
        }
    }

    /// Read another page of a query session's last settled result. Pages are 1-based and use
    /// the page size that run used. The result is an immutable snapshot, so paging never
    /// re-runs the query — but a newer run in that session replaces it, and then this reports
    /// that.
    #[tool(annotations(read_only_hint = true))]
    async fn read_page(
        &self,
        Parameters(params): Parameters<ReadPageParams>,
    ) -> Result<Json<PageResult>, AgentError> {
        let session = session_handle(&params.query_session)?;
        let (project, engine) = self.engine(params.project.as_deref()).await?;
        let Some(last) = self.recall(&project.root, session) else {
            return Err(AgentError::NotFound(format!(
                "No result to read in query session '{}'. Run a query in it first.",
                params.query_session
            )));
        };
        // A snapshot id only means anything alongside the engine that minted it: the counter
        // restarts at 1 on an engine restart, and the project remounts at the same root, so an
        // id remembered across one would otherwise resolve against a *different* result — in
        // practice whatever the user has run since. Checked before the empty-page arm too, so
        // nothing is answered out of an entry the current engine never made.
        if last.engine != engine.id() {
            self.forget(&project.root, session);
            return Err(AgentError::ResultMoved);
        }
        let page = params.page.max(1);

        let Some(snapshot) = last.snapshot else {
            // A run that produced no rows materialized nothing. Reporting an empty page is
            // the truth; a "not found" would read as a lost result.
            return Ok(Json(PageResult {
                query_session: params.query_session,
                columns: last.columns,
                rows: Vec::new(),
                total: 0,
                page,
                page_size: last.page_size,
            }));
        };

        let sort = params.sort.map(|s| (s.column, s.ascending));
        match engine
            .fetch_page(snapshot, page, last.page_size, sort)
            .await
        {
            Ok((rows, _)) => Ok(Json(PageResult {
                query_session: params.query_session,
                columns: last.columns,
                rows: cells(&rows),
                total: last.total,
                page,
                page_size: last.page_size,
            })),
            // Ask the engine, never its prose: a snapshot that is gone is a replaced result,
            // anything else is a real read failure. Asked *after* the read, so the answer
            // cannot race the dispatch that retired it.
            Err(e) => {
                if engine.snapshot_live(snapshot) {
                    Err(AgentError::Query(e))
                } else {
                    self.forget(&project.root, session);
                    Err(AgentError::ResultMoved)
                }
            }
        }
    }

    /// Close one of your query sessions. A run still in flight in it is cancelled. Closing is
    /// tidy rather than required — every session you hold goes when you disconnect.
    #[tool(annotations(destructive_hint = false))]
    async fn close_query_session(
        &self,
        Parameters(params): Parameters<QuerySessionParams>,
    ) -> Result<Json<QuerySessionResult>, AgentError> {
        let session = session_handle(&params.query_session)?;
        let project = self.project(params.project.as_deref()).await?;
        self.host
            .close_query_session(&project.root, self.connection.agent, session)
            .await?;
        self.forget(&project.root, session);
        Ok(Json(QuerySessionResult {
            query_session: params.query_session,
        }))
    }
}

#[tool_handler(
    name = "strata",
    instructions = "Strata is a local parquet/CSV/JSON query workspace over Apache DataFusion. \
Read-only: SELECT, EXPLAIN, SHOW and DESCRIBE run; everything else is refused. \
Start with list_tables and describe_table to learn the catalog, validate to check SQL \
cheaply, then open_query_session and run. Your work lives in query sessions of your own, \
which the user watches in the Agents pane and can promote into their editor — so it never \
disturbs the tabs they are working in. Open a session per line of investigation; each run \
in a session replaces the last one's result."
)]
impl<H: Host> ServerHandler for StrataTools<H> {}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::{env, process};

    use strata_core::engine::sql::Blocked;
    use strata_core::engine::{RunTag, TableSpec, WsId, CANCELLED};
    use strata_model::SourceFormat;

    use crate::host::{CatalogEntry, Described, QuerySessionState, RegState};
    use crate::mock::{MockHost, MockProject};
    use crate::wire::{Mode, QuerySessionStateWire, Sort, StateWire};

    use super::*;

    /// A scratch folder of our own, per test.
    fn scratch(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata_agent_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A project whose engine really holds a `people` table of five rows, plus the catalog
    /// rows an app would have folded from the same registration.
    async fn one_project(tag: &str) -> (PathBuf, StrataTools<MockHost>) {
        let root = scratch(tag);
        fs::write(
            root.join("people.csv"),
            "id,name\n1,ana\n2,ben\n3,cara\n4,dev\n5,eli\n",
        )
        .unwrap();
        let project = MockProject::new("sales", &root);
        let meta = project
            .engine
            .register(TableSpec {
                name: "people".into(),
                paths: vec![root.join("people.csv").display().to_string()],
                format: SourceFormat::from_name("csv"),
                partitions: Vec::new(),
            })
            .await
            .unwrap();
        let project = project
            .with_catalog(vec![
                CatalogEntry::Table {
                    name: "people".into(),
                    format: "csv".into(),
                    sources: vec!["people.csv".into()],
                    reg: RegState::Ready,
                },
                CatalogEntry::Table {
                    name: "gone".into(),
                    format: "parquet".into(),
                    sources: vec!["missing.parquet".into()],
                    reg: RegState::Failed("No source paths".into()),
                },
            ])
            .with_described(Described::Table {
                name: "people".into(),
                format: "csv".into(),
                sources: vec!["people.csv".into()],
                partitions: Vec::new(),
                rows: meta.rows,
                columns: meta.columns,
            });
        let tools = StrataTools::new(MockHost::new(vec![project]));
        (root, tools)
    }

    fn no_project() -> ProjectParams {
        ProjectParams { project: None }
    }

    /// What a client that introduced itself looks like. The tools' own `#[tool]` wrapper
    /// reads this off the MCP peer; every test drives the semantic call underneath it, which
    /// is the same thing the in-process caller will do.
    fn claude() -> AgentIdentity {
        AgentIdentity {
            name: "claude-code".into(),
            version: "2.1.4".into(),
        }
    }

    async fn open(tools: &StrataTools<MockHost>) -> String {
        tools
            .open_session(claude(), None)
            .await
            .unwrap()
            .0
            .to_string()
    }

    fn run_params(session: &str, sql: &str) -> RunParams {
        RunParams {
            query_session: session.into(),
            sql: sql.into(),
            mode: None,
            page_size: None,
            project: None,
        }
    }

    // --- projects ---------------------------------------------------------

    #[tokio::test]
    async fn list_projects_names_the_open_windows() {
        let tools = StrataTools::new(MockHost::new(vec![
            MockProject::new("sales", "/w/sales"),
            MockProject::new("ops", "/w/ops"),
        ]));
        let listed = tools.list_projects().await.0.projects;
        let names: Vec<&str> = listed.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["sales", "ops"]);
        assert_eq!(listed[1].root, "/w/ops");
    }

    /// With two windows open, a project-scoped tool must not guess — and the error is the
    /// recovery, so it has to name them.
    #[tokio::test]
    async fn a_project_scoped_tool_is_ambiguous_with_two_open() {
        let tools = StrataTools::new(MockHost::new(vec![
            MockProject::new("sales", "/w/sales"),
            MockProject::new("ops", "/w/ops"),
        ]));
        let Err(e) = tools.list_tables(Parameters(no_project())).await else {
            panic!("expected an ambiguous-project error");
        };
        let text = e.to_string();
        assert!(text.contains("sales (/w/sales)"), "{text}");
        assert!(text.contains("ops (/w/ops)"), "{text}");

        // Naming one resolves it.
        let named = tools
            .list_tables(Parameters(ProjectParams {
                project: Some("ops".into()),
            }))
            .await
            .unwrap();
        assert!(named.0.entries.is_empty());
    }

    // --- catalog ----------------------------------------------------------

    /// The catalog as the store shows it: a def the engine refused is a row with its error,
    /// not a missing row.
    #[tokio::test]
    async fn list_tables_reports_a_failed_def_with_its_error() {
        let (_root, tools) = one_project("list_tables").await;
        let entries = tools
            .list_tables(Parameters(no_project()))
            .await
            .unwrap()
            .0
            .entries;
        match &entries[..] {
            [EntryWire::Table {
                name: ready,
                state: StateWire::Ready,
                error: None,
                ..
            }, EntryWire::Table {
                name: failed,
                state: StateWire::Failed,
                error: Some(why),
                ..
            }] => {
                assert_eq!(ready, "people");
                assert_eq!(failed, "gone");
                assert_eq!(why, "No source paths");
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn describe_table_reports_what_registration_read() {
        let (_root, tools) = one_project("describe").await;
        let described = tools
            .describe_table(Parameters(DescribeTableParams {
                name: "people".into(),
                project: None,
            }))
            .await
            .unwrap()
            .0;
        assert_eq!(described.name, "people");
        assert_eq!(described.format.as_deref(), Some("csv"));
        let columns: Vec<&str> = described.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(columns, vec!["id", "name"]);
    }

    #[tokio::test]
    async fn describe_table_on_an_unknown_name_is_not_found() {
        let (_root, tools) = one_project("describe_unknown").await;
        let Err(AgentError::NotFound(message)) = tools
            .describe_table(Parameters(DescribeTableParams {
                name: "nope".into(),
                project: None,
            }))
            .await
        else {
            panic!("expected a not-found error");
        };
        assert!(message.contains("'nope'"), "{message}");
    }

    /// The function list is the live registry, so it carries DataFusion's built-ins and the
    /// JSON accessors `build_context` registers — no second list to keep in step.
    #[tokio::test]
    async fn list_functions_is_the_live_registry() {
        let (_root, tools) = one_project("functions").await;
        let functions = tools
            .list_functions(Parameters(no_project()))
            .await
            .unwrap()
            .0;
        assert!(functions.scalar.iter().any(|f| f.name == "json_get"));
        assert!(functions.aggregate.iter().any(|f| f.name == "count"));
        assert!(!functions.window.is_empty());
    }

    #[tokio::test]
    async fn validate_finds_a_missing_table_without_running_anything() {
        let (_root, tools) = one_project("validate").await;
        let clean = tools
            .validate(Parameters(ValidateParams {
                sql: "SELECT id FROM people".into(),
                project: None,
            }))
            .await
            .unwrap()
            .0;
        assert!(clean.diagnostics.is_empty(), "{clean:?}");

        let broken = tools
            .validate(Parameters(ValidateParams {
                sql: "SELECT id FROM nope".into(),
                project: None,
            }))
            .await
            .unwrap()
            .0;
        assert!(
            broken
                .diagnostics
                .iter()
                .any(|d| d.message.contains("nope")),
            "{broken:?}"
        );
    }

    // --- query sessions ---------------------------------------------------

    #[tokio::test]
    async fn query_sessions_open_list_and_close() {
        let (_root, tools) = one_project("sessions").await;
        let session = open(&tools).await;

        let listed = tools
            .list_query_sessions(Parameters(no_project()))
            .await
            .unwrap()
            .0
            .query_sessions;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].query_session, session);
        assert!(matches!(listed[0].state, QuerySessionStateWire::Empty));

        tools
            .close_query_session(Parameters(QuerySessionParams {
                query_session: session.clone(),
                project: None,
            }))
            .await
            .unwrap();
        assert!(tools
            .list_query_sessions(Parameters(no_project()))
            .await
            .unwrap()
            .0
            .query_sessions
            .is_empty());

        // The handle is now stale, and the answer is the plain statement
        // `list_query_sessions` recovers from — the same one a handle that never existed gets.
        let Err(AgentError::NotFound(_)) = tools
            .close_query_session(Parameters(QuerySessionParams {
                query_session: session,
                project: None,
            }))
            .await
        else {
            panic!("expected a not-found error");
        };
    }

    /// **An agent sees its own sessions and nothing else** — the rule AA-03b exists for,
    /// where AA-03's `list_tabs` handed over every open tab including the user's.
    ///
    /// Two connections over one host and one run cache, which is the arrangement that could
    /// leak: the second lists nothing, and a handle it should never have had answers exactly
    /// as a made-up one does — no "that belongs to someone else", which would confirm the
    /// session exists.
    #[tokio::test]
    async fn an_agent_sees_only_its_own_query_sessions() {
        let (_root, first) = one_project("scoping").await;
        let second = first.connection();
        let borrowed = open(&first).await;
        open(&second).await;

        let mine = first
            .list_query_sessions(Parameters(no_project()))
            .await
            .unwrap()
            .0
            .query_sessions;
        let theirs = second
            .list_query_sessions(Parameters(no_project()))
            .await
            .unwrap()
            .0
            .query_sessions;
        assert_eq!(mine.len(), 1);
        assert_eq!(theirs.len(), 1);
        assert_ne!(mine[0].query_session, theirs[0].query_session);

        for reached in [
            second
                .run(Parameters(run_params(&borrowed, "SELECT 1")))
                .await
                .err(),
            second
                .close_query_session(Parameters(QuerySessionParams {
                    query_session: borrowed.clone(),
                    project: None,
                }))
                .await
                .err(),
        ] {
            assert!(
                matches!(reached, Some(AgentError::NotFound(_))),
                "another agent's session is simply not there: {reached:?}"
            );
        }
    }

    /// A connection ending takes its query sessions with it — the RAII half of the same
    /// rule, and the only signal there is that a client has gone.
    #[tokio::test]
    async fn dropping_a_connection_retracts_its_query_sessions() {
        let (root, tools) = one_project("disconnect").await;
        let gone = tools.connection();
        open(&gone).await;
        open(&tools).await;
        assert_eq!(
            tools
                .host
                .query_sessions(&root, gone.connection.agent)
                .await
                .unwrap()
                .len(),
            1
        );

        drop(gone);

        assert!(
            tools
                .host
                .query_sessions(&root, tools.connection.agent)
                .await
                .unwrap()
                .len()
                == 1,
            "the surviving agent keeps its own"
        );
        assert_eq!(
            tools.host.projects().await.len(),
            1,
            "and the project is untouched"
        );
    }

    #[tokio::test]
    async fn a_malformed_handle_is_not_found_rather_than_a_parse_error() {
        let (_root, tools) = one_project("bad_handle").await;
        let Err(AgentError::NotFound(message)) = tools
            .run(Parameters(run_params("not-a-uuid", "SELECT 1")))
            .await
        else {
            panic!("expected a not-found error");
        };
        assert!(message.contains("not-a-uuid"), "{message}");
    }

    // --- run --------------------------------------------------------------

    #[tokio::test]
    async fn run_returns_page_one_and_the_exact_total() {
        let (_root, tools) = one_project("run").await;
        let session = open(&tools).await;
        let mut params = run_params(&session, "SELECT id, name FROM people ORDER BY id");
        params.page_size = Some(2);

        let result = tools.run(Parameters(params)).await.unwrap().0;
        let RunResult::Ok {
            columns,
            rows,
            total,
            page,
            page_size,
            ..
        } = result
        else {
            panic!("{result:?}");
        };
        assert_eq!(
            columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["id", "name"]
        );
        // Bounded by the page, exact in the total: no `LIMIT` was injected.
        assert_eq!(rows.len(), 2);
        assert_eq!(total, 5);
        assert_eq!((page, page_size), (1, 2));
        assert_eq!(
            rows[0],
            vec![Some("1".to_string()), Some("ana".to_string())]
        );
    }

    /// The run lands on the **query session's own engine workspace**, which is what keeps an
    /// agent's work off the user's tabs while staying a real execution: the snapshot is
    /// retired by the next run in that session and by nothing else.
    #[tokio::test]
    async fn a_run_executes_against_the_query_sessions_workspace() {
        let (root, tools) = one_project("workspace").await;
        let session = open(&tools).await;
        let RunResult::Ok { .. } = tools
            .run(Parameters(run_params(&session, "SELECT id FROM people")))
            .await
            .unwrap()
            .0
        else {
            panic!("expected rows");
        };

        let engine = tools.host.engine(&root).await.unwrap();
        let ws = WsId::from(QuerySessionId(Uuid::parse_str(&session).unwrap()));

        // **The claim under test**: the run landed on the *session's* workspace, not somewhere
        // else. Proved by retiring that workspace's snapshot the way only its owner can — a
        // newer dispatch on the same `WsId` — and watching the agent's page go with it. A run
        // dispatched anywhere else would leave this readable.
        engine
            .query(ws, RunTag(4242), "SELECT name FROM people".into(), 10)
            .await
            .unwrap();
        assert!(
            matches!(
                tools
                    .read_page(Parameters(ReadPageParams {
                        query_session: session,
                        page: 1,
                        sort: None,
                        project: None,
                    }))
                    .await,
                Err(AgentError::ResultMoved)
            ),
            "the session's own workspace owned that snapshot"
        );
    }

    /// The app reads `row_limit: 0` as "no limit", so a host returning that setting verbatim
    /// must not get one-row pages out of it — which a bare `clamp(1, MAX)` would give.
    #[tokio::test]
    async fn a_host_default_of_zero_means_no_limit_not_one_row() {
        let (_root, tools) = one_project("no_limit").await;
        tools.host.set_default_page_size(0);
        let session = open(&tools).await;

        let RunResult::Ok {
            rows, page_size, ..
        } = tools
            .run(Parameters(run_params(&session, "SELECT id FROM people")))
            .await
            .unwrap()
            .0
        else {
            panic!("expected rows");
        };
        assert_eq!(page_size, MAX_PAGE_SIZE);
        assert_eq!(rows.len(), 5, "every row, not one");
    }

    /// An agent that stops calling is a thing this layer never hears about, so without a
    /// bound the cache keeps a whole result schema per abandoned session for the life of the
    /// process.
    #[tokio::test]
    async fn the_run_cache_evicts_its_oldest_entry() {
        let (_root, tools) = one_project("evict").await;
        let mut sessions = Vec::new();
        for _ in 0..MAX_REMEMBERED_RUNS + 1 {
            let session = open(&tools).await;
            tools
                .run(Parameters(run_params(&session, "SELECT id FROM people")))
                .await
                .unwrap();
            sessions.push(session);
        }
        assert_eq!(tools.runs.lock().unwrap().len(), MAX_REMEMBERED_RUNS);

        // The first session's result is gone, and reads exactly like a session that never
        // ran; the newest is still there.
        let evicted = tools
            .read_page(Parameters(ReadPageParams {
                query_session: sessions[0].clone(),
                page: 1,
                sort: None,
                project: None,
            }))
            .await;
        assert!(
            matches!(evicted, Err(AgentError::NotFound(_))),
            "the oldest is evicted"
        );
        assert!(tools
            .read_page(Parameters(ReadPageParams {
                query_session: sessions.pop().unwrap(),
                page: 1,
                sort: None,
                project: None,
            }))
            .await
            .is_ok());
    }

    /// **Eviction is per agent.** The cache is one map shared by every connection, so a global
    /// bound would let a chatty client discard a peer's still-readable result — a cross-agent
    /// effect in a vocabulary whose whole point is that an agent cannot reach another's work,
    /// and one that reports itself as "run a query in it first" for a query that *was* run.
    #[tokio::test]
    async fn one_agent_cannot_evict_anothers_result() {
        let (_root, mine) = one_project("evict_scoping").await;
        let theirs = mine.connection();

        let kept = open(&mine).await;
        mine.run(Parameters(run_params(&kept, "SELECT id FROM people")))
            .await
            .unwrap();

        // The other agent fills its own quota and then some.
        for _ in 0..MAX_REMEMBERED_RUNS + 4 {
            let session = open(&theirs).await;
            theirs
                .run(Parameters(run_params(&session, "SELECT id FROM people")))
                .await
                .unwrap();
        }

        assert!(
            mine.read_page(Parameters(ReadPageParams {
                query_session: kept,
                page: 1,
                sort: None,
                project: None,
            }))
            .await
            .is_ok(),
            "another agent's chatter must not discard my page"
        );
    }

    /// **A snapshot id only means anything beside the engine that minted it.** Ids are a
    /// per-engine counter that restarts at 1, and an engine restart remounts the project at
    /// the same root — so a remembered id would otherwise resolve against a *different*
    /// result, in practice whatever the user has run since.
    #[tokio::test]
    async fn a_remembered_page_does_not_survive_the_engine_that_made_it() {
        let root = scratch("engine_swap");
        fs::write(root.join("people.csv"), "id,name\n1,ana\n2,ben\n").unwrap();
        async fn register(engine: &Engine, root: &Path) {
            engine
                .register(TableSpec {
                    name: "people".into(),
                    paths: vec![root.join("people.csv").display().to_string()],
                    format: SourceFormat::from_name("csv"),
                    partitions: Vec::new(),
                })
                .await
                .unwrap();
        }
        let project = MockProject::new("sales", &root);
        register(&project.engine, &root).await;
        let tools = StrataTools::new(MockHost::new(vec![project]));

        let session = open(&tools).await;
        tools
            .run(Parameters(run_params(&session, "SELECT id FROM people")))
            .await
            .unwrap();

        // The restart: the project keeps its root and gets a fresh engine, whose snapshot
        // counter starts over at 1 — the very id the agent is holding.
        let replacement = MockProject::new("sales", &root);
        register(&replacement.engine, &root).await;
        tools.host.replace_engine(&root, replacement.engine.clone());
        replacement
            .engine
            .query(WsId(9), RunTag(1), "SELECT name FROM people".into(), 10)
            .await
            .unwrap();

        assert!(
            matches!(
                tools
                    .read_page(Parameters(ReadPageParams {
                        query_session: session,
                        page: 1,
                        sort: None,
                        project: None,
                    }))
                    .await,
                Err(AgentError::ResultMoved)
            ),
            "an id from a retired engine must never resolve against the new one"
        );
    }

    #[tokio::test]
    async fn an_oversized_page_is_clamped_and_says_so() {
        let (_root, tools) = one_project("clamp").await;
        let session = open(&tools).await;
        let mut params = run_params(&session, "SELECT id FROM people");
        params.page_size = Some(MAX_PAGE_SIZE * 10);

        let RunResult::Ok { page_size, .. } = tools.run(Parameters(params)).await.unwrap().0 else {
            panic!("expected rows");
        };
        assert_eq!(page_size, MAX_PAGE_SIZE);
    }

    /// The gate is the editor's own predicate, and the agent reads the editor's own words.
    #[tokio::test]
    async fn run_refuses_blocked_ddl_with_the_editors_message() {
        let (_root, tools) = one_project("policy").await;
        let session = open(&tools).await;
        let Err(e) = tools
            .run(Parameters(run_params(
                &session,
                "CREATE TABLE copy AS SELECT * FROM people",
            )))
            .await
        else {
            panic!("expected a policy refusal");
        };
        assert_eq!(e.to_string(), Blocked::CreateTable.editor_message());
    }

    /// Fail closed: input that cannot be judged is never a policy pass.
    #[tokio::test]
    async fn run_refuses_input_it_cannot_parse() {
        let (_root, tools) = one_project("unparseable").await;
        let session = open(&tools).await;
        assert!(matches!(
            tools
                .run(Parameters(run_params(&session, "SELECT FROM WHERE )")))
                .await,
            Err(AgentError::Query(_))
        ));
    }

    #[tokio::test]
    async fn run_in_explain_mode_returns_the_plan_and_materializes_nothing() {
        let (_root, tools) = one_project("explain").await;
        let session = open(&tools).await;
        let mut params = run_params(&session, "SELECT id FROM people");
        params.mode = Some(Mode::Explain);

        let RunResult::Plan {
            analyze,
            logical,
            physical,
            ..
        } = tools.run(Parameters(params)).await.unwrap().0
        else {
            panic!("expected a plan");
        };
        assert!(!analyze);
        assert!(logical.contains("people"), "{logical}");
        assert!(!physical.is_empty());

        // Nothing was materialized, so there is nothing to page.
        let Err(AgentError::NotFound(_)) = tools
            .read_page(Parameters(ReadPageParams {
                query_session: session,
                page: 1,
                sort: None,
                project: None,
            }))
            .await
        else {
            panic!("an explain leaves no readable result");
        };
    }

    /// A cancel or a supersede is news the user already has. It settles as an outcome with a
    /// reason, never as a fault.
    #[tokio::test]
    async fn a_stopped_run_is_an_outcome_not_an_error() {
        let root = scratch("stopped");
        let tools = StrataTools::new(MockHost::new(vec![
            MockProject::new("sales", &root).settling(CANCELLED)
        ]));
        let session = open(&tools).await;
        let result = tools
            .run(Parameters(run_params(&session, "SELECT 1")))
            .await
            .expect("a stop is not an error")
            .0;
        let RunResult::Stopped { reason, .. } = result else {
            panic!("{result:?}");
        };
        assert_eq!(reason, CANCELLED);
    }

    #[tokio::test]
    async fn run_against_an_unknown_query_session_is_not_found() {
        let (_root, tools) = one_project("run_unknown").await;
        let stray = QuerySessionId::new().0.to_string();
        assert!(matches!(
            tools.run(Parameters(run_params(&stray, "SELECT 1"))).await,
            Err(AgentError::NotFound(_))
        ));
    }

    // --- read_page --------------------------------------------------------

    #[tokio::test]
    async fn read_page_walks_the_settled_snapshot() {
        let (_root, tools) = one_project("read_page").await;
        let session = open(&tools).await;
        let mut params = run_params(&session, "SELECT id FROM people ORDER BY id");
        params.page_size = Some(2);
        tools.run(Parameters(params)).await.unwrap();

        let page = tools
            .read_page(Parameters(ReadPageParams {
                query_session: session.clone(),
                page: 3,
                sort: None,
                project: None,
            }))
            .await
            .unwrap()
            .0;
        // Page size follows the run that settled it, so paging is consistent.
        assert_eq!(page.page_size, 2);
        assert_eq!(page.total, 5);
        assert_eq!(page.rows, vec![vec![Some("5".to_string())]]);

        let sorted = tools
            .read_page(Parameters(ReadPageParams {
                query_session: session,
                page: 1,
                sort: Some(Sort {
                    column: "id".into(),
                    ascending: false,
                }),
                project: None,
            }))
            .await
            .unwrap()
            .0;
        assert_eq!(sorted.rows[0], vec![Some("5".to_string())]);
    }

    /// A newer run in that session retires the snapshot. The read must say so — and say it
    /// from the engine's answer to "is this still there?", never from its prose.
    #[tokio::test]
    async fn read_page_reports_a_result_a_newer_run_replaced() {
        let (root, tools) = one_project("moved").await;
        let session = open(&tools).await;
        tools
            .run(Parameters(run_params(&session, "SELECT id FROM people")))
            .await
            .unwrap();

        // Straight at the engine, on the session's own workspace — what the next run reaches.
        let engine = tools.host.engine(&root).await.unwrap();
        engine
            .query(
                WsId(Uuid::parse_str(&session).unwrap().as_u128()),
                RunTag(999),
                "SELECT name FROM people".into(),
                10,
            )
            .await
            .unwrap();

        assert!(matches!(
            tools
                .read_page(Parameters(ReadPageParams {
                    query_session: session.clone(),
                    page: 1,
                    sort: None,
                    project: None,
                }))
                .await,
            Err(AgentError::ResultMoved)
        ));
    }

    /// A query with no rows materializes nothing, so an empty page is the honest answer —
    /// "not found" would read as a lost result.
    #[tokio::test]
    async fn read_page_of_an_empty_result_is_an_empty_page() {
        let (_root, tools) = one_project("empty").await;
        let session = open(&tools).await;
        tools
            .run(Parameters(run_params(
                &session,
                "SELECT id FROM people WHERE id > 99",
            )))
            .await
            .unwrap();

        let page = tools
            .read_page(Parameters(ReadPageParams {
                query_session: session,
                page: 1,
                sort: None,
                project: None,
            }))
            .await
            .unwrap()
            .0;
        assert_eq!(page.total, 0);
        assert!(page.rows.is_empty());
    }

    /// The mock's `QuerySessionState` reaches the wire verbatim — pinned because the enum is
    /// the one shape a well-meaning "unknown" arm could be added to.
    #[test]
    fn a_session_state_crosses_the_wire_unchanged() {
        let wire = crate::wire::QuerySessionWire::from(crate::host::QuerySessionInfo {
            session: QuerySessionId::new(),
            state: QuerySessionState::Running,
        });
        assert!(matches!(wire.state, QuerySessionStateWire::Running));
    }
}
