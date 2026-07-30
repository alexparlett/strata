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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use strata_core::engine::{stopped_on_purpose, Engine};
use strata_model::{SnapshotId, TabId};
use uuid::Uuid;

use crate::error::AgentError;
use crate::host::{self, Host, Project, RunMode, Settled};
use crate::wire::{
    cells, columns, plan_result, rows_result, Columns, DescribeResult, DescribeTableParams,
    DiagnosticWire, EntryWire, FunctionsResult, PageResult, ProjectParams, ProjectsResult,
    ReadPageParams, RunParams, RunResult, TabParams, TabResult, TablesResult, TabsResult,
    ValidateParams, ValidateResult,
};

/// The most rows one call will hand back, however large a `page_size` is asked for. A cap
/// rather than an error: the response reports the `page_size` actually used, so the clamp is
/// visible in the answer rather than a silent truncation. It is also what a
/// [`Host::default_page_size`] of `0` ("no limit", the app's own reading of that setting)
/// resolves to.
pub const MAX_PAGE_SIZE: usize = 10_000;

/// How many tabs' results stay readable at once, across every project.
///
/// The cache has no other bound: `close_tab` and a superseded read drop an entry, but a tab
/// the **user** closes in the app is a thing the server never hears about, so its entry would
/// sit there for the life of the process holding a whole result schema. The oldest is evicted
/// once past this, which costs nothing an agent notices — a `read_page` on an evicted tab
/// reports the same "run a query in it first" as one on a tab that never ran, and re-running
/// is the recovery either way.
const MAX_REMEMBERED_RUNS: usize = 64;

/// What the server remembers about a tab's last settled run, so `read_page` can page it.
///
/// A cache of a fact the settle reply carried, not a second source of truth: the snapshot it
/// names may be retired by the next press in that tab (the user's or the agent's), and then
/// the read fails cleanly with "the result was replaced". Deliberately **not** a
/// `SnapshotPin` — pinning would keep a result alive past the run that owns it, which is
/// right for an export window and wrong here, where the honest answer is that the tab has
/// moved on.
#[derive(Clone, Debug)]
struct LastRun {
    /// `None` when the query returned no rows — nothing was materialized, so there is
    /// nothing to page, and the honest answer is an empty page rather than a fault.
    snapshot: Option<SnapshotId>,
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

/// The tool vocabulary over one [`Host`].
pub struct StrataTools<H: Host> {
    host: Arc<H>,
    /// Keyed by `(project root, tab)` — a tab handle is unique per project, and a project's
    /// root is its identity.
    runs: Arc<Mutex<HashMap<(PathBuf, TabId), LastRun>>>,
    /// Stamps each remembered run so the oldest can be found. Shared with every clone of the
    /// service, like the map it orders.
    seq: Arc<AtomicU64>,
}

// A manual `Clone`: the derive would demand `H: Clone`, and the whole point of the `Arc` is
// that the host is shared, never copied. The service is cloned per MCP session.
impl<H: Host> Clone for StrataTools<H> {
    fn clone(&self) -> Self {
        StrataTools {
            host: Arc::clone(&self.host),
            runs: Arc::clone(&self.runs),
            seq: Arc::clone(&self.seq),
        }
    }
}

impl<H: Host> StrataTools<H> {
    pub fn new(host: Arc<H>) -> Self {
        StrataTools {
            host,
            runs: Arc::new(Mutex::new(HashMap::new())),
            seq: Arc::new(AtomicU64::new(0)),
        }
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

    fn remember(
        &self,
        root: &Path,
        tab: TabId,
        snapshot: Option<SnapshotId>,
        columns: Columns,
        total: usize,
        page_size: usize,
    ) {
        let mut runs = self.runs.lock().unwrap();
        runs.insert(
            (root.to_path_buf(), tab),
            LastRun {
                snapshot,
                columns,
                total,
                page_size,
                seq: self.seq.fetch_add(1, Ordering::Relaxed),
            },
        );
        while runs.len() > MAX_REMEMBERED_RUNS {
            let Some(oldest) = runs
                .iter()
                .min_by_key(|(_, run)| run.seq)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            runs.remove(&oldest);
        }
    }

    fn forget(&self, root: &Path, tab: TabId) {
        self.runs.lock().unwrap().remove(&(root.to_path_buf(), tab));
    }

    fn recall(&self, root: &Path, tab: TabId) -> Option<LastRun> {
        self.runs
            .lock()
            .unwrap()
            .get(&(root.to_path_buf(), tab))
            .cloned()
    }
}

/// A tab handle is a `TabId`'s `Uuid` as text. Anything else never named a tab, so it gets
/// the same answer an expired handle does — `list_tabs` is the recovery either way.
fn tab_handle(text: &str) -> Result<TabId, AgentError> {
    Uuid::parse_str(text)
        .map(TabId)
        .map_err(|_| AgentError::NotFound(format!("No open tab '{text}'.")))
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

    /// Open a new query tab in the project window and return its handle. Tabs are shared
    /// with the user: they can read, edit, re-run or close one at any time.
    #[tool]
    async fn open_tab(
        &self,
        Parameters(params): Parameters<ProjectParams>,
    ) -> Result<Json<TabResult>, AgentError> {
        let project = self.project(params.project.as_deref()).await?;
        let tab = self.host.open_tab(&project.root).await?;
        Ok(Json(TabResult {
            tab: tab.0.to_string(),
        }))
    }

    /// List the project's open tabs: handle, title, and whether a run is in flight, settled,
    /// or has never happened.
    #[tool(annotations(read_only_hint = true))]
    async fn list_tabs(
        &self,
        Parameters(params): Parameters<ProjectParams>,
    ) -> Result<Json<TabsResult>, AgentError> {
        let project = self.project(params.project.as_deref()).await?;
        let tabs = self.host.tabs(&project.root).await?;
        Ok(Json(TabsResult {
            tabs: tabs.into_iter().map(Into::into).collect(),
        }))
    }

    /// Run read-only SQL in a tab and wait for it to settle. This is an ordinary press: it
    /// replaces whatever that tab was showing, appears in history and the event log, and can
    /// be cancelled or superseded by the user. Returns page 1 plus the exact total; use
    /// read_page for the rest. Set mode to 'explain' for the query plan without executing.
    #[tool]
    async fn run(
        &self,
        Parameters(params): Parameters<RunParams>,
    ) -> Result<Json<RunResult>, AgentError> {
        let tab = tab_handle(&params.tab)?;
        let (project, engine) = self.engine(params.project.as_deref()).await?;

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

        // Dispatching a run retires the tab's previous snapshot, so what we remembered about
        // it is already dead. An explain materializes nothing and leaves it alone.
        if mode == RunMode::Run {
            self.forget(&project.root, tab);
        }

        let settled = self
            .host
            .run(&project.root, tab, params.sql, mode, page_size)
            .await?;
        let handle = params.tab;
        match settled {
            Ok(Settled::Rows(output)) => {
                // Converted once and shared: the response and every later page describe the
                // same schema, so nothing re-walks it.
                let cols = columns(&output.columns);
                self.remember(
                    &project.root,
                    tab,
                    output.snapshot,
                    Columns::clone(&cols),
                    output.total,
                    output.page_size,
                );
                Ok(Json(rows_result(handle, cols, output)))
            }
            Ok(Settled::Plan(plan)) => Ok(Json(plan_result(handle, plan))),
            // The one place stopped-vs-failed is judged.
            Err(e) if stopped_on_purpose(&e) => Ok(Json(RunResult::Stopped {
                tab: handle,
                reason: e,
            })),
            Err(e) => Err(AgentError::Query(e)),
        }
    }

    /// Read another page of a tab's last settled result. Pages are 1-based and use the page
    /// size that run used. The result is an immutable snapshot, so paging never re-runs the
    /// query — but a newer run in that tab replaces it, and then this reports that.
    #[tool(annotations(read_only_hint = true))]
    async fn read_page(
        &self,
        Parameters(params): Parameters<ReadPageParams>,
    ) -> Result<Json<PageResult>, AgentError> {
        let tab = tab_handle(&params.tab)?;
        let (project, engine) = self.engine(params.project.as_deref()).await?;
        let Some(last) = self.recall(&project.root, tab) else {
            return Err(AgentError::NotFound(format!(
                "No result to read in tab '{}'. Run a query in it first.",
                params.tab
            )));
        };
        let page = params.page.max(1);

        let Some(snapshot) = last.snapshot else {
            // A run that produced no rows materialized nothing. Reporting an empty page is
            // the truth; a "not found" would read as a lost result.
            return Ok(Json(PageResult {
                tab: params.tab,
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
                tab: params.tab,
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
                    self.forget(&project.root, tab);
                    Err(AgentError::ResultMoved)
                }
            }
        }
    }

    /// Close a tab. A run still in flight in it is cancelled, the same way closing the tab in
    /// the app would.
    #[tool(annotations(destructive_hint = false))]
    async fn close_tab(
        &self,
        Parameters(params): Parameters<TabParams>,
    ) -> Result<Json<TabResult>, AgentError> {
        let tab = tab_handle(&params.tab)?;
        let project = self.project(params.project.as_deref()).await?;
        self.host.close_tab(&project.root, tab).await?;
        self.forget(&project.root, tab);
        Ok(Json(TabResult { tab: params.tab }))
    }
}

#[tool_handler(
    name = "strata",
    instructions = "Strata is a local parquet/CSV/JSON query workspace over Apache DataFusion. \
Read-only: SELECT, EXPLAIN, SHOW and DESCRIBE run; everything else is refused. \
Start with list_tables and describe_table to learn the catalog, validate to check SQL \
cheaply, then open_tab and run. Every run lands as a real query tab in the user's window, \
so park findings in their own tabs and iterate in a scratch one."
)]
impl<H: Host> ServerHandler for StrataTools<H> {}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::{env, process};

    use strata_core::engine::sql::Blocked;
    use strata_core::engine::{RunTag, TableSpec, WsId, CANCELLED};
    use strata_model::SourceFormat;

    use crate::host::{CatalogEntry, Described, RegState};
    use crate::mock::{MockHost, MockProject};
    use crate::wire::{Mode, Sort, StateWire, TabStateWire};

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

    async fn open(tools: &StrataTools<MockHost>) -> String {
        tools
            .open_tab(Parameters(no_project()))
            .await
            .unwrap()
            .0
            .tab
    }

    fn run_params(tab: &str, sql: &str) -> RunParams {
        RunParams {
            tab: tab.into(),
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

    // --- tabs -------------------------------------------------------------

    #[tokio::test]
    async fn tabs_open_list_and_close() {
        let (_root, tools) = one_project("tabs").await;
        let tab = open(&tools).await;

        let listed = tools
            .list_tabs(Parameters(no_project()))
            .await
            .unwrap()
            .0
            .tabs;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].tab, tab);
        assert!(matches!(listed[0].state, TabStateWire::Empty));

        tools
            .close_tab(Parameters(TabParams {
                tab: tab.clone(),
                project: None,
            }))
            .await
            .unwrap();
        assert!(tools
            .list_tabs(Parameters(no_project()))
            .await
            .unwrap()
            .0
            .tabs
            .is_empty());

        // The handle is now stale, and the answer is the plain statement `list_tabs` recovers
        // from — the same one a handle that never existed gets.
        let Err(AgentError::NotFound(_)) = tools
            .close_tab(Parameters(TabParams { tab, project: None }))
            .await
        else {
            panic!("expected a not-found error");
        };
    }

    #[tokio::test]
    async fn a_malformed_tab_handle_is_not_found_rather_than_a_parse_error() {
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
        let tab = open(&tools).await;
        let mut params = run_params(&tab, "SELECT id, name FROM people ORDER BY id");
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

    /// The app reads `row_limit: 0` as "no limit", so a host returning that setting verbatim
    /// must not get one-row pages out of it — which a bare `clamp(1, MAX)` would give.
    #[tokio::test]
    async fn a_host_default_of_zero_means_no_limit_not_one_row() {
        let (_root, tools) = one_project("no_limit").await;
        tools.host.set_default_page_size(0);
        let tab = open(&tools).await;

        let RunResult::Ok {
            rows, page_size, ..
        } = tools
            .run(Parameters(run_params(&tab, "SELECT id FROM people")))
            .await
            .unwrap()
            .0
        else {
            panic!("expected rows");
        };
        assert_eq!(page_size, MAX_PAGE_SIZE);
        assert_eq!(rows.len(), 5, "every row, not one");
    }

    /// A tab the *user* closes in the app is never reported to the server, so without a bound
    /// the cache keeps a whole result schema per abandoned tab for the life of the process.
    #[tokio::test]
    async fn the_run_cache_evicts_its_oldest_entry() {
        let (_root, tools) = one_project("evict").await;
        let mut tabs = Vec::new();
        for _ in 0..MAX_REMEMBERED_RUNS + 1 {
            let tab = open(&tools).await;
            tools
                .run(Parameters(run_params(&tab, "SELECT id FROM people")))
                .await
                .unwrap();
            tabs.push(tab);
        }
        assert_eq!(tools.runs.lock().unwrap().len(), MAX_REMEMBERED_RUNS);

        // The first tab's result is gone, and reads exactly like a tab that never ran; the
        // newest is still there.
        let evicted = tools
            .read_page(Parameters(ReadPageParams {
                tab: tabs[0].clone(),
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
                tab: tabs.pop().unwrap(),
                page: 1,
                sort: None,
                project: None,
            }))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn an_oversized_page_is_clamped_and_says_so() {
        let (_root, tools) = one_project("clamp").await;
        let tab = open(&tools).await;
        let mut params = run_params(&tab, "SELECT id FROM people");
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
        let tab = open(&tools).await;
        let Err(e) = tools
            .run(Parameters(run_params(
                &tab,
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
        let tab = open(&tools).await;
        assert!(matches!(
            tools
                .run(Parameters(run_params(&tab, "SELECT FROM WHERE )")))
                .await,
            Err(AgentError::Query(_))
        ));
    }

    #[tokio::test]
    async fn run_in_explain_mode_returns_the_plan_and_materializes_nothing() {
        let (_root, tools) = one_project("explain").await;
        let tab = open(&tools).await;
        let mut params = run_params(&tab, "SELECT id FROM people");
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
                tab,
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
        let tab = open(&tools).await;
        let result = tools
            .run(Parameters(run_params(&tab, "SELECT 1")))
            .await
            .expect("a stop is not an error")
            .0;
        let RunResult::Stopped { reason, .. } = result else {
            panic!("{result:?}");
        };
        assert_eq!(reason, CANCELLED);
    }

    #[tokio::test]
    async fn run_against_an_unknown_tab_is_not_found() {
        let (_root, tools) = one_project("run_unknown_tab").await;
        let stray = TabId::new().0.to_string();
        assert!(matches!(
            tools.run(Parameters(run_params(&stray, "SELECT 1"))).await,
            Err(AgentError::NotFound(_))
        ));
    }

    // --- read_page --------------------------------------------------------

    #[tokio::test]
    async fn read_page_walks_the_settled_snapshot() {
        let (_root, tools) = one_project("read_page").await;
        let tab = open(&tools).await;
        let mut params = run_params(&tab, "SELECT id FROM people ORDER BY id");
        params.page_size = Some(2);
        tools.run(Parameters(params)).await.unwrap();

        let page = tools
            .read_page(Parameters(ReadPageParams {
                tab: tab.clone(),
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
                tab,
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

    /// A run in the tab behind retires the snapshot. The read must say so — and say it from
    /// the engine's answer to "is this still there?", never from its prose.
    #[tokio::test]
    async fn read_page_reports_a_result_a_newer_run_replaced() {
        let (root, tools) = one_project("moved").await;
        let tab = open(&tools).await;
        tools
            .run(Parameters(run_params(&tab, "SELECT id FROM people")))
            .await
            .unwrap();

        // The user presses Run again in that very tab — straight at the engine, which is
        // what the app's own press reaches.
        let engine = tools.host.engine(&root).await.unwrap();
        engine
            .query(
                WsId(Uuid::parse_str(&tab).unwrap().as_u128()),
                RunTag(999),
                "SELECT name FROM people".into(),
                10,
            )
            .await
            .unwrap();

        assert!(matches!(
            tools
                .read_page(Parameters(ReadPageParams {
                    tab: tab.clone(),
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
        let tab = open(&tools).await;
        tools
            .run(Parameters(run_params(
                &tab,
                "SELECT id FROM people WHERE id > 99",
            )))
            .await
            .unwrap();

        let page = tools
            .read_page(Parameters(ReadPageParams {
                tab,
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
}
