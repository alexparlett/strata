//! The **`Host` seam** — the union of the vocabulary's questions, and nothing else.
//!
//! The tool layer ([`crate::tools`]) knows what an agent may ask; a [`Host`] knows how to
//! answer it *here*. In the app (AA-03) the control-plane methods travel a channel to the
//! project window and are answered by its Radio state; headless (AA-05) they hit a plain
//! [`Engine`] and the defs the registration pass was replayed over. Both hand back the same
//! `Arc<Engine>` for the data plane, which is why `fetch_page` / `validate` / `functions`
//! never queue behind UI work.
//!
//! Two shapes here earn their oddness:
//!
//! - The trait's methods are declared `-> impl Future<Output = …> + Send` rather than as
//!   `async fn`, because rmcp polls a handler's future on its own runtime and so requires
//!   `Send` — which an `async fn` in a trait does not promise. Implementors still write
//!   plain `async fn`.
//! - [`Host::run`] returns a `Result` inside a `Result` ([`RunSettle`]), and the nesting is
//!   the point: the outer arm is "the run never dispatched" (no such query session, the
//!   window went), the inner one is the engine's own settle. Only the inner one may be a
//!   *stop* rather than a fault, and the tool layer is the single place that judges which
//!   (`strata_core::engine::stopped_on_purpose`) — a host that folded the two would be a
//!   second copy of that rule.
//!
//! ## Every session-scoped question names the agent asking it (AA-03b)
//!
//! An agent addresses **its own** query sessions and nothing else. That is not defensive
//! bookkeeping: AA-03 landed agent runs in the user's own tabs, which meant `list_tabs`
//! handed an agent every open tab — the one the user was typing in included — and a `run`
//! on it replaced their buffer. Passing an [`Agent`] to every session-scoped method makes
//! the ownership check the host's *only* way to answer, so there is no listing that could
//! include somebody else's work and no handle that could be guessed into.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use strata_core::engine::plan::QueryPlan;
use strata_core::engine::{Engine, WsId};
use strata_model::{ColumnInfo, QueryOutput};
use uuid::Uuid;

use crate::error::AgentError;

/// One project an agent can address: what it is called, and where it lives.
///
/// The **root is the identity** — a window is keyed on its project folder and two windows
/// can never hold one project, so a root names at most one host entry. The name is what a
/// person types, and is allowed to collide (`/a/data` and `/b/data` are both "data"), which
/// is why resolution tries the root first and reports an ambiguous *name* rather than
/// picking one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub root: PathBuf,
}

/// How far a def has got with the engine — the app's `Reg<T>` without the payload, because
/// what a *list* row carries is the state and what registration *learned* is
/// [`Host::describe`]'s answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegState {
    /// The engine has not answered for this def yet (a fresh open, or a re-scan in flight).
    Pending,
    Ready,
    /// The engine refused it. The def still exists; there is just nothing behind it.
    Failed(String),
}

/// One row of the catalog **as the store shows it** — never DataFusion introspection, which
/// would hide exactly the failed defs this exists to show (the P3-02 correction).
#[derive(Clone, Debug, PartialEq)]
pub enum CatalogEntry {
    Table {
        name: String,
        /// The reader the def names (`parquet`, `csv`, …).
        format: String,
        /// Source paths **as stored** — relative entries are not resolved here; that is the
        /// registration pass's business and the def is what the user wrote.
        sources: Vec<String>,
        reg: RegState,
    },
    View {
        name: String,
        sql: String,
        reg: RegState,
    },
    /// A saved query: text the user parked, not an object the engine holds — so it has no
    /// registration state to report, and `describe_table` does not answer for it.
    Query { id: Uuid, name: String, sql: String },
}
/// What the catalog knows about one **table or view**, in the four states it can be in.
///
/// Four variants rather than one struct of `Option`s because the states are genuinely
/// exclusive: a def that failed has no schema to report, and a pending one has not been
/// asked yet. Only real facts (P3-08) — every number here was read at registration, none is
/// derived from rows on screen.
#[derive(Clone, Debug, PartialEq)]
pub enum Described {
    Table {
        name: String,
        format: String,
        sources: Vec<String>,
        /// Hive partition columns as `(name, type)`.
        partitions: Vec<(String, String)>,
        /// The free row count, when the source reports one (parquet's footer does; CSV and
        /// JSON do not).
        rows: Option<u64>,
        columns: Vec<ColumnInfo>,
    },
    View {
        name: String,
        sql: String,
        columns: Vec<ColumnInfo>,
        /// The base tables the view scans.
        reads: Vec<String>,
    },
    /// The def is there; the engine refused it.
    Failed { name: String, error: String },
    /// The def is there; registration has not answered yet.
    Pending { name: String },
}

impl Described {
    /// The def this describes, whichever state it is in — what a host matches a
    /// `describe_table` name against. On the type rather than beside each host, because
    /// every host has to answer the same question and a second copy of the match is a second
    /// place a new variant can be forgotten.
    pub fn name(&self) -> &str {
        match self {
            Described::Table { name, .. }
            | Described::View { name, .. }
            | Described::Failed { name, .. }
            | Described::Pending { name } => name,
        }
    }
}

/// One connected agent, for as long as its connection lasts.
///
/// Minted per connection wherever there **is** one, rather than derived from what the client
/// calls itself: two Claude Code windows are two agents and report the identical
/// `clientInfo`, so a name is a label and never an identity. Ends when the connection does —
/// a client that disconnects takes its query sessions with it, which is what "the pane shows
/// what is happening now" means.
///
/// One transport path cannot honour that, and the limit is stated rather than hidden: MCP's
/// 2026-07-28 discover lifecycle (SEP-2567) removes sessions altogether, so a client on it
/// has no connection to key on and `clientInfo` is the only thing left. There, two windows of
/// one client really do share an agent — see `tools::Caller`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct AgentId(pub Uuid);

impl AgentId {
    #[allow(clippy::new_without_default)]
    pub fn new() -> AgentId {
        AgentId(Uuid::new_v4())
    }
}

/// What a client says it is: MCP's `clientInfo`, which arrives at `initialize` and is the
/// only thing a client ever tells us about itself.
///
/// Both fields can be empty, and the surface showing them has to survive that — a client is
/// not obliged to introduce itself well, and a blank row is the honest rendering of one that
/// did not.
#[derive(Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct AgentIdentity {
    pub name: String,
    pub version: String,
}

/// An agent introducing itself: its connection's identity, and what it calls itself.
///
/// Only [`Host::open_query_session`] takes one, and that is the design rather than an
/// economy: opening a session is when a host first has anything of this agent's to show, so
/// it is the one call that needs the label. Everything after it is addressed by [`AgentId`]
/// alone — which is also what lets every other tool be called with no MCP peer at all, the
/// property the in-process chat pane (AA-06) will need.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Agent {
    pub id: AgentId,
    pub identity: AgentIdentity,
}

/// One **query session** — an agent's container for a sequence of runs, each replacing the
/// last.
///
/// Deliberately not called a tab or a workspace. Bare "session" is taken twice over (MCP's
/// own `Mcp-Session-Id`, and ours in `session.json`), and this collides with neither while
/// mapping exactly onto the engine's [`WsId`]: a query session *is* an engine workspace, so
/// supersede, retire and cancel are the engine's own, unchanged.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct QuerySessionId(pub Uuid);

impl QuerySessionId {
    #[allow(clippy::new_without_default)]
    pub fn new() -> QuerySessionId {
        QuerySessionId(Uuid::new_v4())
    }
}

impl From<QuerySessionId> for WsId {
    /// The `Uuid` widened, exactly as a `TabId` is — a query session and a tab are two kinds
    /// of workspace on one engine, and v4 randomness is what keeps their key spaces apart.
    fn from(session: QuerySessionId) -> WsId {
        WsId(session.0.as_u128())
    }
}

/// Where a query session's last run got to. Tri-state rather than two booleans, because "in
/// flight and settled" is not a thing a session can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuerySessionState {
    /// Nothing has been run in it.
    Empty,
    Running,
    /// A run finished — with rows, a plan, an error, or a stop. All of those are settled;
    /// only "in flight" is not, and a session whose run failed has certainly not gone back
    /// to [`Empty`](QuerySessionState::Empty).
    Settled,
}

/// One of the agent's own query sessions, as `list_query_sessions` reports it.
///
/// **No title.** A tab had one because a person names and reads tabs; a query session has
/// nothing to be called — what it is, is what has run in it, which the agent already knows
/// and the Agents pane shows.
#[derive(Clone, Debug, PartialEq)]
pub struct QuerySessionInfo {
    pub session: QuerySessionId,
    pub state: QuerySessionState,
}

/// What a `run` asks for. `Explain` materializes nothing and leaves the session's settled
/// snapshot alone, so a plan does not cost the agent its readable page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunMode {
    Run,
    Explain,
}

/// What a dispatched press produced.
#[derive(Clone, Debug)]
pub enum Settled {
    /// A run: the snapshot handle, page 1, the exact total.
    Rows(QueryOutput),
    /// An explain: the plan trees, nothing materialized.
    Plan(QueryPlan),
}

/// The engine's own answer to a press that **did** dispatch: `Ok` is a result, `Err` is the
/// engine's settled error string — which may be a fault or may be a stop
/// (`strata_core::engine::stopped_on_purpose`). Judging it is the tool layer's, once.
pub type RunSettle = Result<Settled, String>;

/// Whatever answers the vocabulary for a given deployment.
///
/// Every method is exactly one question a tool asks. Do not grow it speculatively: a
/// capability with no tool behind it has no impl to keep honest.
pub trait Host: Send + Sync + 'static {
    /// Every project a tool call can address. Empty is a legitimate answer (the app is open
    /// with no project window); the tool layer turns it into "no project is open".
    fn projects(&self) -> impl Future<Output = Vec<Project>> + Send;

    /// The page size a `run` uses when the caller names none — the app's default row-limit
    /// setting, read per call so a change in Settings lands without restarting the server.
    /// Sync, because it is a number the host already holds and never a question it has to
    /// ask a window.
    ///
    /// **`0` means no limit**, matching `strata_core::config::Settings::row_limit`, which
    /// documents its own zero that way. A host returning that setting verbatim is therefore
    /// correct, and the tool layer resolves the zero to
    /// [`MAX_PAGE_SIZE`](crate::tools::MAX_PAGE_SIZE) — the largest page it will ever hand
    /// back. Naming it here rather than leaving it to each host is the point: the obvious
    /// wiring (`self.config.row_limit`) would otherwise ship one-row pages to every agent
    /// whose user had turned the limit off.
    fn default_page_size(&self) -> usize;

    /// The **data plane**: the engine serving `project`. Reads that are engine-scoped and
    /// side-effect free (`fetch_page`, `validate`, `functions`) go straight to it.
    fn engine(
        &self,
        project: &Path,
    ) -> impl Future<Output = Result<Arc<Engine>, AgentError>> + Send;

    fn catalog(
        &self,
        project: &Path,
    ) -> impl Future<Output = Result<Vec<CatalogEntry>, AgentError>> + Send;

    /// One table or view in full. Separate from [`catalog`](Host::catalog) rather than a
    /// field on its rows, because a schema can be enormous (a 19,311-field struct is a real
    /// file here) and a listing that carried every one of them would clone all of it to
    /// render a name and a state.
    fn describe(
        &self,
        project: &Path,
        name: &str,
    ) -> impl Future<Output = Result<Described, AgentError>> + Send;

    /// `agent`'s own query sessions in `project`, and **only** those — see the module note.
    fn query_sessions(
        &self,
        project: &Path,
        agent: AgentId,
    ) -> impl Future<Output = Result<Vec<QuerySessionInfo>, AgentError>> + Send;

    /// Open a query session for `agent`. The identity travels with it because this is where
    /// a host first learns the agent exists — there is no separate "an agent connected"
    /// call, since an agent that opens nothing has done nothing to show.
    fn open_query_session(
        &self,
        project: &Path,
        agent: &Agent,
    ) -> impl Future<Output = Result<QuerySessionId, AgentError>> + Send;

    /// Close one of `agent`'s query sessions: a running query in it is cancelled, and the
    /// engine workspace is torn down the way closing a tab tears down a tab's.
    fn close_query_session(
        &self,
        project: &Path,
        agent: AgentId,
        session: QuerySessionId,
    ) -> impl Future<Output = Result<(), AgentError>> + Send;

    /// Run `sql` in `session` and await its settle — **on the engine directly**, against the
    /// session's own [`WsId`], so it is the same execution a person's press makes (same
    /// snapshot lifecycle, same supersede, same cancel) without being one of their presses.
    /// The SQL arrives already past the policy gate; a host never re-judges it, and never
    /// rewrites it (no injected `LIMIT` — the response is bounded by `page_size` and paging,
    /// and the total stays exact).
    fn run(
        &self,
        project: &Path,
        agent: AgentId,
        session: QuerySessionId,
        sql: String,
        mode: RunMode,
        page_size: usize,
    ) -> impl Future<Output = Result<RunSettle, AgentError>> + Send;

    /// The agent's connection ended: retract it and everything it was showing.
    ///
    /// **Sync, and it must not block** — the caller is a `Drop` on whatever runtime the
    /// transport happens to be running the session worker on, with no way to await and
    /// nowhere to report a failure to. It is also *not* project-scoped: an agent may hold
    /// query sessions in several windows and none of them survives its connection, so a
    /// host retracts it everywhere it lent something.
    fn agent_gone(&self, agent: AgentId);
}

/// Resolve the optional `project` argument against what is open.
///
/// The whole rule, in one place, for every project-scoped tool: the **root** is tried first
/// (it is the identity), then the name; a name matching more than one open project is
/// ambiguous rather than a guess. With no argument, a single open project is the answer and
/// more than one is ambiguous — the default-to-single-project scoping of the spec.
pub(crate) fn resolve(projects: Vec<Project>, want: Option<&str>) -> Result<Project, AgentError> {
    if projects.is_empty() {
        return Err(AgentError::NoProject);
    }
    let Some(want) = want else {
        return match <[Project; 1]>::try_from(projects) {
            Ok([only]) => Ok(only),
            Err(many) => Err(AgentError::Ambiguous(many)),
        };
    };
    if let Some(by_root) = projects.iter().find(|p| p.root == Path::new(want)) {
        return Ok(by_root.clone());
    }
    let by_name: Vec<Project> = projects
        .iter()
        .filter(|p| p.name == want)
        .cloned()
        .collect();
    match <[Project; 1]>::try_from(by_name) {
        Ok([only]) => Ok(only),
        Err(none) if none.is_empty() => Err(AgentError::NotFound(format!(
            "No open project named '{want}'."
        ))),
        Err(many) => Err(AgentError::Ambiguous(many)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, root: &str) -> Project {
        Project {
            name: name.into(),
            root: PathBuf::from(root),
        }
    }

    #[test]
    fn no_argument_takes_the_single_open_project() {
        let one = vec![project("sales", "/w/sales")];
        assert_eq!(resolve(one, None).unwrap(), project("sales", "/w/sales"));
    }

    #[test]
    fn no_argument_with_two_open_is_ambiguous_and_lists_them() {
        let two = vec![project("sales", "/w/sales"), project("ops", "/w/ops")];
        let Err(AgentError::Ambiguous(listed)) = resolve(two.clone(), None) else {
            panic!("expected an ambiguous-project error");
        };
        assert_eq!(listed, two);
    }

    #[test]
    fn nothing_open_is_its_own_answer() {
        assert!(matches!(
            resolve(Vec::new(), None),
            Err(AgentError::NoProject)
        ));
        assert!(matches!(
            resolve(Vec::new(), Some("sales")),
            Err(AgentError::NoProject)
        ));
    }

    /// The root is the identity, so it resolves even when a *name* would be ambiguous.
    #[test]
    fn a_root_resolves_past_a_colliding_name() {
        let two = vec![project("data", "/a/data"), project("data", "/b/data")];
        assert_eq!(
            resolve(two.clone(), Some("/b/data")).unwrap(),
            project("data", "/b/data")
        );
        let Err(AgentError::Ambiguous(listed)) = resolve(two.clone(), Some("data")) else {
            panic!("a colliding name is ambiguous, never a guess");
        };
        assert_eq!(listed, two);
    }

    #[test]
    fn a_name_resolves_when_it_is_unique() {
        let two = vec![project("sales", "/w/sales"), project("ops", "/w/ops")];
        assert_eq!(resolve(two, Some("ops")).unwrap(), project("ops", "/w/ops"));
    }

    #[test]
    fn an_unknown_project_is_not_found() {
        let one = vec![project("sales", "/w/sales")];
        assert!(matches!(
            resolve(one, Some("nope")),
            Err(AgentError::NotFound(_))
        ));
    }
}
