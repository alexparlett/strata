//! The **vocabulary** — the eleven agent-access tools, over a [`Host`].
//!
//! Ten of them read; the eleventh, [`export_result`](StrataTools::export_result), writes one file
//! at a path the caller names. It is not a hole in the read-only rule — the gate `run`
//! asks is untouched and refuses the agent's own `COPY` exactly as before — because there is no
//! data behind it that `read_page` does not already hand over byte for byte. What it *does* need
//! guarding is the destination, and that fence is `SnapshotReads::export_to`'s.
//!
//! [`StrataTools`] is the rmcp `ServerHandler`, and it is deliberately transport-free: the
//! Streamable-HTTP server ([`crate::server`]) serves it, the headless host serves the
//! same value over stdio, and the assistant calls it in-process. One surface, three
//! frontends.
//!
//! **The vocabulary is methods; the tools are wrappers.** The public methods on [`StrataTools`]
//! *are* the eleven tools — plain arguments, plain answers, no rmcp type in any signature — and the
//! `#[tool_router]` block below them is one wrapper each, doing only what a semantic call cannot:
//! resolving which agent the *request* is ([`Caller`]) and holding it against the idle sweep. So an
//! in-process caller reaches the identical body, gate and messages included.
//! [`StrataTools::manifest`] is the vocabulary as data, **derived from the router**, so there is no
//! second list to keep in step.
//!
//! Three rules are enforced here and nowhere else:
//!
//! - **The policy gate runs before dispatch.** `Workspace::query` does not enforce the managed-DDL
//!   policy — the editor simply never dispatches what validation flagged, and an agent cannot be
//!   trusted with that discipline. `run` asks `Lang::policy_verdicts` and refuses on any
//!   non-clean answer, an unjudgeable one included: the gate fails closed.
//! - **A stop is not a fault.** `EngineError::Stopped` is matched once, here.
//! - **`run` never rewrites SQL.** No injected `LIMIT`; the *response* is bounded by `page_size`
//!   plus `read_page`.
//!
//! **One agent per client, and the request says which.** A [`StrataTools`] *is* one agent: it
//! carries a [`Connection`] which mints an [`AgentId`] and retracts it on drop, and every
//! session-scoped answer is scoped by that id rather than by a check somebody has to remember. The
//! id is resolved from the *request* through [`Caller`], because a value's lifetime is the
//! connection's on only some of the transport's paths — rmcp's stateless branch builds one service
//! per request. `Clone` therefore *shares* the connection; `connection()` is the only thing that
//! starts a new agent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http::request::Parts;
use rmcp::handler::server::common::{AsRequestContext, FromContextPart};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{JsonObject, ProtocolVersion};
use rmcp::service::Peer;
use rmcp::{tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler};
use serde_json::Value;
use strata_engine::{Engine, EngineError};
use strata_model::{PageQuery, SnapshotId};
use uuid::Uuid;

use crate::describe;
use crate::error::AgentError;
use crate::host::{
    self, Agent, AgentId, AgentIdentity, Described, Host, Project, QuerySessionId, RunMode, Settled,
};
use crate::wire::{
    cells, columns, export_format, functions_result, plan_result, rows_result, tables_result,
    Columns, DescribeResult, DescribeTableParams, DiagnosticWire, ExportResult, ExportResultParams,
    FunctionsResult, ListFunctionsParams, ListTablesParams, PageResult, ProjectParams,
    ProjectsResult, QuerySessionParams, QuerySessionResult, QuerySessionsResult, ReadPageParams,
    RunParams, RunResult, TablesResult, ValidateParams, ValidateResult,
};

/// The most rows one call will hand back, however large a `page_size` is asked for. A cap
/// rather than an error: the response reports the `page_size` actually used, so the clamp is
/// visible in the answer rather than a silent truncation. It is also what a
/// [`Host::default_page_size`] of `0` ("no limit", the app's own reading of that setting)
/// resolves to.
pub const MAX_PAGE_SIZE: usize = 10_000;

/// One tool as a model is offered it: what it is called, what it does, and the JSON its
/// arguments have to be.
///
/// Plain data on purpose — no rmcp type reaches the caller, so the in-process loop
/// can hand these to whatever the provider's tool shape is without depending on the MCP SDK.
/// Built only by [`StrataTools::manifest`], which derives it from the router, so there is one
/// vocabulary with two transports rather than two vocabularies.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    /// The `#[tool]` method's doc comment, verbatim — the same text `tools/list` advertises.
    pub description: String,
    /// JSON Schema for the arguments **object**, exactly as advertised: `{"type": "object",
    /// "properties": {…}}`, empty properties for a tool that takes none.
    pub input_schema: Value,
}

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

/// How long a **stateless** agent may go unheard from before it is retracted.
///
/// Only the discover lifecycle needs this: every other path has a connection whose end is an
/// event, so its agent is retracted by a `Drop` rather than by a clock.
///
/// **Thirty minutes, because the five it replaces were parity with something that does not
/// govern this branch.** The old value matched rmcp's `SessionConfig::keep_alive`, which reaps
/// a *session worker* whose channel has gone quiet
/// (`transport::streamable_http_server::session::local`) — the lifecycle whose agents are
/// already retracted by a `Drop`, so the figure was borrowed from the one path that never
/// needed it. A stateless request holds no rmcp state at all between calls (`get_service()`
/// per request, dropped when the response is written), so this sweep is the only bound there
/// and there is nothing on rmcp's side for it to agree with.
///
/// What five minutes cost in the field was query sessions retired mid-investigation, each
/// taking the result the user was about to promote. The `Busy` guard re-stamps `seen` when a call
/// *finishes*, so what this clock actually measures is the gap between calls — a model
/// reasoning over a large result, or a person reading one, is not an absent client, and the
/// server cannot tell the difference.
///
/// The leak it exists to bound is still bounded, and by the same things as before: what one
/// quiet agent holds is capped by `MAX_REMEMBERED_RUNS` and by the per-agent session cap,
/// and a client that has genuinely gone is still reaped rather than kept for the window's
/// life.
pub const STATELESS_IDLE: Duration = Duration::from_secs(30 * 60);

/// Which agent a call is being made as.
///
/// **Identity comes from the request, because a service value's lifetime is the connection's
/// on only some of the transport's paths.** rmcp 3.0.1 serves Streamable HTTP two ways
/// (`transport::streamable_http_server::tower`): the session lifecycle, where one service
/// value is owned by the session worker for the client's whole life, and — for a client
/// negotiating `2026-07-28` or sending per-request `_meta` — the **stateless** branch, where
/// `get_service()` is called per request and the value is dropped when the response is
/// written. Minting the id from that value's lifetime makes every request a different agent
/// on the second branch: `open_query_session` mints a session under one agent and the next
/// call cannot see it, so the feature is silently dead for that client.
///
/// So the value's own agent is used only where it is *earned* — a call with no HTTP request behind
/// it (stdio, or the in-process chat pane), and a call rmcp served on its **session** lifecycle.
/// Anything else is stateless and falls back to the only durable thing such a client sends.
///
/// **The branch is decided by rmcp's own predicate, not by `Mcp-Session-Id`.** That header looks
/// like the discriminator and is not one: `is_legacy_request` reads the request's `_meta` and
/// protocol version and never consults it, so a client echoing a stale session id while sending
/// per-request `_meta` would be called `Owned` and handed a fresh agent per request.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Caller {
    /// This value's own connection — [`Connection`], retracted by its drop.
    Owned,
    /// A client with no connection to key on, identified by what it says it is (SEP-2575's
    /// `_meta` `clientInfo`). Two windows of one client share this agent, which is a real
    /// loss of resolution and is the honest maximum: the protocol carries nothing else.
    ///
    /// The identity may be **blank**, because `clientInfo` is not among the keys
    /// `2026-07-28` requires (`RequestMetaObject::DRAFT_REQUIRED_KEYS` is `protocolVersion`
    /// and `clientCapabilities`). A blank one is refused the session-scoped tools rather than
    /// pooled — see [`StrataTools::agent`].
    Stateless(AgentIdentity),
}

impl Caller {
    /// What this caller calls itself.
    ///
    /// The request is preferred over the peer on the stateless branch because there is no
    /// peer to speak of: rmcp reconstructs `peer_info` with `Implementation::default()`
    /// (`tower.rs`, `peer_info_for_stateless_request`), and that is **not** a blank —
    /// `Implementation::default` is `from_build_env`, so it reads `rmcp` / the rmcp version.
    /// Falling back to it would label every un-introduced client with the name of the MCP
    /// library it happens to use.
    fn identity(&self, peer: &Peer<RoleServer>) -> AgentIdentity {
        match self {
            Caller::Stateless(identity) => identity.clone(),
            Caller::Owned => peer_identity(peer),
        }
    }
}

/// What a client said it was at `initialize`, or a blank identity if it has not said.
///
/// A blank is rendered honestly by the surface that shows it rather than refused here: a
/// client is not obliged to introduce itself well, and losing its whole session over a
/// missing name would be the app punishing the user for the client's manners.
fn peer_identity(peer: &Peer<RoleServer>) -> AgentIdentity {
    peer.peer_info()
        .map(|info| AgentIdentity {
            name: info.client_info.name.clone(),
            version: info.client_info.version.clone(),
        })
        .unwrap_or_default()
}

impl<C: AsRequestContext> FromContextPart<C> for Caller {
    fn from_context_part(context: &mut C) -> Result<Self, ErrorData> {
        let context = context.as_request_context();
        if context.extensions.get::<Parts>().is_none() {
            return Ok(Caller::Owned);
        }
        let discover = context
            .meta
            .missing_required_keys(&ProtocolVersion::V_2026_07_28)
            .is_empty();
        let modern = context
            .protocol_version()
            .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28);
        if !discover && !modern {
            return Ok(Caller::Owned);
        }
        Ok(Caller::Stateless(
            context
                .meta
                .client_info()
                .map(|info| AgentIdentity {
                    name: info.name,
                    version: info.version,
                })
                .unwrap_or_default(),
        ))
    }
}

/// The stateless agents this server has minted: what each is called, and whether it is idle.
///
/// Empty on every path but one, and that is the point: a [`Caller::Owned`] never reaches here
/// because its id is its connection's and its retraction is that connection's drop.
#[derive(Default)]
struct Roster {
    live: Mutex<HashMap<AgentIdentity, Live>>,
}

/// One stateless agent's id, when it was last heard from, and how many of its calls are still
/// running.
struct Live {
    agent: AgentId,
    seen: Instant,
    /// **Why a clock alone will not do.** `seen` is stamped when a call is *resolved*, and a
    /// `run` can then sit on the engine for minutes. Retiring on the stamp alone would abort
    /// the agent's own query — `agent_gone` releases its sessions and `cleanup_ws` aborts
    /// whatever is in flight — and the engine settles an abort as `cancelled`, which the
    /// vocabulary reports back as "you stopped this" for a cancellation the *timer*
    /// performed. That is the failure `Agents::opened`'s eviction gate exists to refuse, and
    /// a housekeeping sweep has even less business causing it.
    busy: usize,
}

/// One stateless call in flight, holding its agent against the sweeper.
///
/// RAII for [`SnapshotPin`](strata_engine::SnapshotPin)'s reason: the thing being
/// protected outlives the statement that starts it, and every early return, `?` and dropped
/// request future has to release it. Dropping re-stamps `seen`, so a long call leaves the
/// agent's idle window starting from when it *finished*.
///
/// [`none`](Busy::none) is the whole of the non-stateless case — a `Caller::Owned` has no
/// roster entry to hold, so its guard holds nothing and its drop does nothing.
struct Busy {
    roster: Option<Arc<Roster>>,
    identity: AgentIdentity,
}

impl Busy {
    fn none() -> Busy {
        Busy {
            roster: None,
            identity: AgentIdentity::default(),
        }
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        let Some(roster) = self.roster.take() else {
            return;
        };
        let Ok(mut live) = roster.live.lock() else {
            return;
        };
        if let Some(entry) = live.get_mut(&self.identity) {
            entry.busy = entry.busy.saturating_sub(1);
            entry.seen = Instant::now();
        }
    }
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
    /// This value is the app's own assistant rather than something that dialled in — see
    /// [`Agent::in_app`]. Set once, by [`StrataTools::in_app`], and inherited by nothing: a
    /// transport builds its values through [`StrataTools::new`] and [`StrataTools::connection`],
    /// so every agent on a wire is false here by construction.
    in_app: bool,
}

impl<H: Host> Drop for Connection<H> {
    fn drop(&mut self) {
        self.host.agent_gone(self.agent);
    }
}

/// What a remembered run is filed under: the agent that ran it, the project it ran in, and the
/// query session it belongs to. Named because it is stated twice — here and by `remember`'s
/// return — and the three parts only mean anything together.
type RunKey = (AgentId, PathBuf, QuerySessionId);

/// The tool vocabulary over one [`Host`], **as one agent**.
pub struct StrataTools<H: Host> {
    host: Arc<H>,
    /// Keyed by `(agent, project root, query session)`. The agent is in the key rather than
    /// checked against it: a cache shared by every connection would otherwise let a handle
    /// that leaked between two agents read the other's rows, and a key is a check that
    /// cannot be forgotten.
    runs: Arc<Mutex<HashMap<RunKey, LastRun>>>,
    /// Stamps each remembered run so the oldest can be found. Shared with every clone of the
    /// service, like the map it orders.
    seq: Arc<AtomicU64>,
    /// Shared with every connection *and* every clone, because its whole job is to outlive
    /// the per-request values the stateless branch mints — see [`Caller`].
    roster: Arc<Roster>,
    connection: Arc<Connection<H>>,
}

impl<H: Host> Clone for StrataTools<H> {
    fn clone(&self) -> Self {
        StrataTools {
            host: Arc::clone(&self.host),
            runs: Arc::clone(&self.runs),
            seq: Arc::clone(&self.seq),
            roster: Arc::clone(&self.roster),
            connection: Arc::clone(&self.connection),
        }
    }
}

impl<H: Host> StrataTools<H> {
    /// The vocabulary over `host`, as one agent — what a transport clones connections from.
    pub fn new(host: Arc<H>) -> Self {
        Self::rooted(host, false)
    }

    /// The vocabulary over `host` **as the app's own assistant** — the same eleven tools,
    /// marked so every [`Host`] can tell it from a client that dialled in.
    ///
    /// The mark rides [`Agent::in_app`] to `open_query_session`, which is where a host first
    /// learns an agent exists, so a host that has to name the caller of a tool can tell the
    /// two apart without holding an id to compare. It changes nothing else: the assistant is
    /// still one more agent to the policy gate, the run cache, the scoping key and the query
    /// sessions.
    pub fn in_app(host: Arc<H>) -> Self {
        Self::rooted(host, true)
    }

    fn rooted(host: Arc<H>, in_app: bool) -> Self {
        StrataTools {
            connection: Arc::new(Connection {
                host: Arc::clone(&host),
                agent: AgentId::new(),
                in_app,
            }),
            host,
            runs: Arc::new(Mutex::new(HashMap::new())),
            seq: Arc::new(AtomicU64::new(0)),
            roster: Arc::new(Roster::default()),
        }
    }

    /// The same vocabulary for a **new** client: a fresh [`AgentId`], the shared host and
    /// the shared run cache. This is what a transport's per-session service factory calls.
    pub fn connection(&self) -> Self {
        StrataTools {
            host: Arc::clone(&self.host),
            runs: Arc::clone(&self.runs),
            seq: Arc::clone(&self.seq),
            roster: Arc::clone(&self.roster),
            connection: Arc::new(Connection {
                host: Arc::clone(&self.host),
                agent: AgentId::new(),
                in_app: false,
            }),
        }
    }

    /// Which [`AgentId`] this call is made under — the one place [`Caller`] is resolved — and
    /// the guard that keeps it alive for the call's duration.
    ///
    /// A stateless caller's id is minted on first sight and kept, so the *same* client asking twice
    /// is the same agent. The [`Busy`] guard stops [`retire_idle`](Self::retire_idle) reaping it
    /// mid-call, and dropping it re-stamps the entry so the idle window is measured from when the
    /// call finished.
    ///
    /// **A blank stateless identity is refused the session-scoped tools.** `clientInfo` is optional
    /// on the discover lifecycle, and pooling every un-introduced client under one minted id would
    /// put two processes behind one [`AgentId`] — the whole of both isolation checks — so one could
    /// list, page and close the other's query sessions. There is nothing to split them on, so the
    /// honest answer is to say so and keep the project-scoped tools working. The line is whether a
    /// tool has to know *whose* agent is asking, not whether it is read-only.
    fn agent(&self, caller: &Caller) -> Result<(AgentId, Busy), AgentError> {
        let Caller::Stateless(identity) = caller else {
            return Ok((self.connection.agent, Busy::none()));
        };
        if *identity == AgentIdentity::default() {
            return Err(AgentError::Query(
                "Your client sent no 'clientInfo', and this protocol version has no session \
                 for the server to tell it apart by. Send '_meta' with \
                 'io.modelcontextprotocol/clientInfo' on each request to use query sessions."
                    .into(),
            ));
        }
        let mut live = self.roster.live.lock().unwrap();
        let entry = live.entry(identity.clone()).or_insert_with(|| Live {
            agent: AgentId::new(),
            seen: Instant::now(),
            busy: 0,
        });
        entry.seen = Instant::now();
        entry.busy += 1;
        Ok((
            entry.agent,
            Busy {
                roster: Some(Arc::clone(&self.roster)),
                identity: identity.clone(),
            },
        ))
    }

    /// Stamp a caller as heard from without needing its id — what the tools that take no
    /// [`AgentId`] call, so **every** tool keeps its agent alive.
    ///
    /// Without this only the five session-scoped tools would refresh the clock, and an agent
    /// following this server's own instructions — start with `list_tables` and
    /// `describe_table`, `validate` the SQL, *then* open a session and run — would be retired
    /// in the middle of exactly the workflow it was told to follow.
    ///
    /// Returns a [`Busy`] guard for the same reason `agent()` does, and the distinction is
    /// worth keeping: a stamp covers the gap *between* calls, the guard covers a call that is
    /// slow in itself. Both are needed, and only having the first is what this signature used
    /// to be.
    fn touch(&self, caller: &Caller) -> Busy {
        let Caller::Stateless(identity) = caller else {
            return Busy::none();
        };
        let mut live = self.roster.live.lock().unwrap();
        let Some(entry) = live.get_mut(identity) else {
            return Busy::none();
        };
        entry.seen = Instant::now();
        entry.busy += 1;
        Busy {
            roster: Some(Arc::clone(&self.roster)),
            identity: identity.clone(),
        }
    }

    /// Retract every stateless agent unheard from for longer than `ttl` and with nothing in
    /// flight, releasing its query sessions exactly as a disconnection would.
    ///
    /// **A poll, because nothing on our side can observe the fact**: a client
    /// on the discover lifecycle has no connection, so its departure is not an event anywhere
    /// — there is no socket close, no `DELETE`, and no value whose drop means anything. The
    /// staleness is therefore bounded and stated rather than hidden — and the bound is `ttl`
    /// **plus one caller's polling interval**, because retraction can only land on a tick:
    /// [`crate::server::SWEEP_INTERVAL`] is half the window, so such an agent stays on the
    /// roster for between one and one and a half `ttl`s after its last call, and never longer.
    ///
    /// Driven by whichever transport can afford a timer — the HTTP server's own runtime
    /// (`crate::server`). Stopping is [`retire_all`](Self::retire_all)'s job, **not** this one
    /// with a zero `ttl`: `idle` requires `busy == 0`, so a zero `ttl` would skip precisely the
    /// agents with work in flight. There is nothing to drive for stdio or in-process, which is
    /// why it is called from there rather than started here.
    pub fn retire_idle(&self, ttl: Duration) {
        let now = Instant::now();
        self.retire(|entry| entry.busy == 0 && now.duration_since(entry.seen) > ttl);
    }

    /// Retract **every** stateless agent, working or not — what the server calls as it stops.
    ///
    /// Not `retire_idle(Duration::ZERO)`, which was the first attempt and skips exactly the
    /// agents that matter: `idle` requires `busy == 0`, so a zero `ttl` changes nothing for an
    /// agent with a call in flight. The runtime is about to be dropped underneath it, and
    /// [`Busy`]'s drop only decrements a counter — it does not, and must not, retract, because
    /// on the ordinary path the connection is still there. So the one place that knows every
    /// connection is ending has to say so itself.
    ///
    /// The `busy` guard exists to stop a *housekeeping* sweep destroying live work. Shutdown is
    /// not housekeeping: the work is going away regardless, and the choice is only whether the
    /// window hears about it.
    pub fn retire_all(&self) {
        self.retire(|_| true);
    }

    /// Drop every roster entry `doomed` accepts and tell the host about each.
    fn retire(&self, doomed: impl Fn(&Live) -> bool) {
        let mut retired = Vec::new();
        {
            let mut live = self.roster.live.lock().unwrap();
            live.retain(|_, entry| {
                let going = doomed(entry);
                if going {
                    retired.push(entry.agent);
                }
                !going
            });
        }
        for agent in retired {
            self.host.agent_gone(agent);
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

    fn key(&self, agent: AgentId, root: &Path, session: QuerySessionId) -> RunKey {
        (agent, root.to_path_buf(), session)
    }

    #[allow(clippy::too_many_arguments)]
    fn remember(
        &self,
        agent: AgentId,
        root: &Path,
        session: QuerySessionId,
        snapshot: Option<SnapshotId>,
        engine: u64,
        columns: Columns,
        total: usize,
        page_size: usize,
    ) {
        let mut runs = self.runs.lock().unwrap();
        runs.insert(
            self.key(agent, root, session),
            LastRun {
                snapshot,
                engine,
                columns,
                total,
                page_size,
                seq: self.seq.fetch_add(1, Ordering::Relaxed),
            },
        );
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

    fn forget(&self, agent: AgentId, root: &Path, session: QuerySessionId) {
        self.runs
            .lock()
            .unwrap()
            .remove(&self.key(agent, root, session));
    }

    fn recall(&self, agent: AgentId, root: &Path, session: QuerySessionId) -> Option<LastRun> {
        self.runs
            .lock()
            .unwrap()
            .get(&self.key(agent, root, session))
            .cloned()
    }
}

/// A handle is a [`QuerySessionId`]'s `Uuid` as text. Anything else never named a session, so
/// it gets the same answer an expired handle does — `list_query_sessions` is the recovery
/// either way.
fn session_handle(text: &str) -> Result<QuerySessionId, AgentError> {
    Uuid::parse_str(text)
        .map(QuerySessionId)
        .map_err(|_| AgentError::NotFound(format!("No open query session '{text}'.")))
}

/// The formats JSON Schema itself defines (2020-12 §7.3). Everything else in a `format`
/// keyword was written for a vocabulary the reader may not share, which for a schema crossing
/// to an unknown client is the same as writing nothing.
const JSON_SCHEMA_FORMATS: &[&str] = &[
    "date-time",
    "date",
    "time",
    "duration",
    "email",
    "idn-email",
    "hostname",
    "idn-hostname",
    "ipv4",
    "ipv6",
    "uri",
    "uri-reference",
    "iri",
    "iri-reference",
    "uuid",
    "uri-template",
    "json-pointer",
    "relative-json-pointer",
    "regex",
];

/// Drop every `format` this schema states that JSON Schema does not define, at any depth.
///
/// Cheap when there is nothing to drop — the walk is over a schema of at most a few hundred
/// nodes, built once per `tools/list`, and the schema is only cloned out of its `Arc` when a
/// format actually goes.
fn plain_json_schema(schema: &mut Arc<JsonObject>) {
    fn strip(value: &mut Value) -> bool {
        let mut dropped = false;
        match value {
            Value::Object(map) => {
                if map
                    .get("format")
                    .and_then(Value::as_str)
                    .is_some_and(|format| !JSON_SCHEMA_FORMATS.contains(&format))
                {
                    map.remove("format");
                    dropped = true;
                }
                for node in map.values_mut() {
                    dropped |= strip(node);
                }
            }
            Value::Array(items) => {
                for node in items {
                    dropped |= strip(node);
                }
            }
            _ => {}
        }
        dropped
    }

    let mut plain = Value::Object(JsonObject::clone(schema));
    if strip(&mut plain) {
        let Value::Object(plain) = plain else {
            unreachable!("an object stays an object")
        };
        *schema = Arc::new(plain);
    }
}

/// **The vocabulary itself** — the eleven tools as plain methods, with no rmcp type in any
/// signature.
///
/// Everything a tool *does* is here; the `#[tool_router]` block below is wrappers. A wrapper
/// resolves [`Caller`] to an [`AgentId`] and a [`Busy`] guard, then delegates — so an MCP client
/// and the in-process chat loop reach the same body, gate, run cache and messages.
///
/// **The in-process caller is the owned case.** It holds this value's own [`Connection`], so its
/// [`AgentId`] retracts by RAII: there is no roster entry to hold and nothing for the idle sweep to
/// reap, which is why these bodies take no [`Busy`] guard.
///
/// Answers are the wire types unchanged: a facade unwrapping them into tidier in-process shapes
/// would be a second vocabulary, and the loop serializes them back for the model anyway.
impl<H: Host> StrataTools<H> {
    /// The router every surface advertises from, **with Rust's integer widths taken back out of
    /// the schemas**.
    ///
    /// schemars writes a `usize` as `"format": "uint"` and a `u64` as `"format": "uint64"`, which
    /// are Rust widths and not JSON Schema formats — the `minimum` beside them already says
    /// everything the width promises about the value. What a client does with a format it does
    /// not know is the client's choice and neither is good: the reference SDK logs a line per
    /// field it reads, and a validator in strict mode refuses to compile the schema at all —
    /// which loses the tool, and with it the `tools/list` that carried it. One pass here rather
    /// than an attribute on two dozen fields, because the next count added would carry the
    /// width again.
    ///
    /// The seam is rmcp's own: `#[tool_handler(router = ...)]` and [`manifest`](Self::manifest)
    /// both read the vocabulary through this, so `tools/list`, a `tools/call` and the in-process
    /// loop are handed the same schemas.
    fn advertised() -> ToolRouter<Self> {
        let mut router = Self::tool_router();
        for route in router.map.values_mut() {
            plain_json_schema(&mut route.attr.input_schema);
            if let Some(schema) = route.attr.output_schema.as_mut() {
                plain_json_schema(schema);
            }
        }
        router
    }

    /// **The vocabulary as data** — what a model is handed so it can ask for these tools by
    /// name.
    ///
    /// Derived from `tool_router()`, never a second list: a tool added to the block below appears
    /// here with no further edit, carrying the name, doc comment and argument schema an MCP client
    /// reads out of `tools/list`.
    ///
    /// **Ordered by name here rather than trusted from the router.** `ToolRouter` is a `HashMap`
    /// and `list_all` only happens to sort on the way out, so inheriting that order would reorder
    /// the list on the day it changed — and the tool block sits at the head of every request, so a
    /// shuffle invalidates the provider's prompt cache every turn.
    ///
    /// A method for the caller's sake: the router *is* the vocabulary, not this value's state.
    pub fn manifest(&self) -> Vec<ToolSpec> {
        let mut tools: Vec<ToolSpec> = Self::advertised()
            .list_all()
            .into_iter()
            .map(|tool| ToolSpec {
                name: tool.name.into_owned(),
                description: tool.description.unwrap_or_default().into_owned(),
                input_schema: Value::Object(JsonObject::clone(&tool.input_schema)),
            })
            .collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        tools
    }

    pub async fn list_projects(&self) -> ProjectsResult {
        ProjectsResult {
            projects: self
                .host
                .projects()
                .await
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }

    /// The project's catalog, plus the database catalogs its connections registered.
    ///
    /// **The entries are the store's defs and only those** (introspection
    /// would hide exactly the failed rows an agent most needs to see). A database connection has
    /// no defs to show — the whole catalog comes through the connection — so listing its
    /// relations here would mean an unbounded remote enumeration inside a paged listing of
    /// something else. Naming the catalogs is the honest middle: the answer says the databases
    /// exist and how to reach into them, and `describe_table` answers for one relation at a
    /// time.
    pub async fn list_tables(&self, params: ListTablesParams) -> Result<TablesResult, AgentError> {
        let (project, engine) = self.engine(params.project.as_deref()).await?;
        let entries = self.host.catalog(&project.root).await?;
        Ok(tables_result(
            entries,
            engine.sources().listing().catalog_names(),
            params.matching.as_deref(),
            params.page,
        ))
    }

    /// One table or view in full — **or one relation in a database connection's catalog**.
    ///
    /// The store is asked first and wins: a def is the project's own row, failure states included,
    /// and only a def can be addressed by a bare name. A **qualified** name the store has no row
    /// for is the remote case, and answering `not found` would be false about a relation the agent
    /// can perfectly well query. The columns come from the provider the connection already caches,
    /// so this pays the same introspection validating such a query pays, once.
    ///
    /// **Only [`AgentError::NotFound`] falls through**, never any error: the host's other answers
    /// are facts about the *call* rather than the name, and the engine handle this method holds
    /// outlives a closed window, so a blanket fallback would answer successfully and throw the real
    /// failure away. A failed *introspection* is likewise not a not-found — the relation is in the
    /// listing and the server went wrong — so its `Err` travels as the engine's own sentence.
    pub async fn describe_table(
        &self,
        params: DescribeTableParams,
    ) -> Result<DescribeResult, AgentError> {
        let (project, engine) = self.engine(params.project.as_deref()).await?;
        let described = match self.host.describe(&project.root, &params.name).await {
            Ok(described) => described,
            Err(AgentError::NotFound(absent)) => {
                let remote = engine
                    .sources()
                    .describe_remote(params.name.clone())
                    .await
                    .map_err(|e| AgentError::Query(e.to_string()))?;
                match remote {
                    Some(remote) => Described::Remote {
                        name: format!("{}.{}", remote.connection, remote.relation),
                        connection: remote.connection,
                        view: remote.view,
                        columns: remote.columns,
                    },
                    None => return Err(AgentError::NotFound(absent)),
                }
            }
            Err(other) => return Err(other),
        };
        describe::describe_result(described, &params)
    }

    pub async fn list_functions(
        &self,
        params: ListFunctionsParams,
    ) -> Result<FunctionsResult, AgentError> {
        let (_, engine) = self.engine(params.project.as_deref()).await?;
        Ok(functions_result(
            engine.lang().functions().as_ref(),
            params.matching.as_deref(),
        ))
    }

    pub async fn validate(&self, params: ValidateParams) -> Result<ValidateResult, AgentError> {
        let (_, engine) = self.engine(params.project.as_deref()).await?;
        let diagnostics = engine.lang().validate(params.sql).await;
        Ok(ValidateResult {
            diagnostics: diagnostics.iter().map(DiagnosticWire::from).collect(),
        })
    }

    /// **Open a query session**, as an agent that knows what it is called.
    ///
    /// The identity is an argument here and read off the peer in the wrapper, because it is
    /// the one thing a caller with no MCP connection has to say for itself — everything after
    /// this call is addressed by [`AgentId`] alone. The assistant's is
    /// [`AgentIdentity::assistant`].
    pub async fn open_query_session(
        &self,
        identity: AgentIdentity,
        params: ProjectParams,
    ) -> Result<QuerySessionResult, AgentError> {
        self.open_query_session_as(self.connection.agent, identity, params)
            .await
    }

    /// The same, for a caller whose agent the *request* decided rather than this value's own
    /// lifetime — see [`Caller`]. Every `_as` method below is that same split, and the public
    /// one above it is always this value's own agent.
    async fn open_query_session_as(
        &self,
        agent: AgentId,
        identity: AgentIdentity,
        params: ProjectParams,
    ) -> Result<QuerySessionResult, AgentError> {
        let project = self.project(params.project.as_deref()).await?;
        let agent = Agent {
            id: agent,
            identity,
            in_app: self.connection.in_app,
        };
        let session = self.host.open_query_session(&project.root, &agent).await?;
        Ok(QuerySessionResult {
            query_session: session.0.to_string(),
        })
    }

    pub async fn list_query_sessions(
        &self,
        params: ProjectParams,
    ) -> Result<QuerySessionsResult, AgentError> {
        self.list_query_sessions_as(self.connection.agent, params)
            .await
    }

    async fn list_query_sessions_as(
        &self,
        agent: AgentId,
        params: ProjectParams,
    ) -> Result<QuerySessionsResult, AgentError> {
        let project = self.project(params.project.as_deref()).await?;
        let sessions = self.host.query_sessions(&project.root, agent).await?;
        Ok(QuerySessionsResult {
            query_sessions: sessions.into_iter().map(Into::into).collect(),
        })
    }

    /// The page size a run will use for what the caller asked — the **one** copy of the
    /// resolution, called by `run` and by the assistant's dispatch, which lowers its own
    /// ceiling over this answer rather than restating the rule.
    ///
    /// A `0` from the host is the app's "no limit", not a request for empty pages — see
    /// [`Host::default_page_size`]. A `0` the *caller* asked for is nothing at all, and the
    /// clamp's floor answers it with one row.
    pub fn resolved_page_size(&self, asked: Option<usize>) -> usize {
        match asked {
            Some(asked) => asked.clamp(1, MAX_PAGE_SIZE),
            None => match self.host.default_page_size() {
                0 => MAX_PAGE_SIZE,
                limit => limit.min(MAX_PAGE_SIZE),
            },
        }
    }

    pub async fn run(&self, params: RunParams) -> Result<RunResult, AgentError> {
        self.run_as(self.connection.agent, params).await
    }

    async fn run_as(&self, agent: AgentId, params: RunParams) -> Result<RunResult, AgentError> {
        let session = session_handle(&params.query_session)?;
        let (project, engine) = self.engine(params.project.as_deref()).await?;

        if params.sql.trim().is_empty() {
            return Err(AgentError::Query("The query is empty.".into()));
        }

        match engine.lang().policy_verdicts(params.sql.clone()).await {
            Err(e) => return Err(AgentError::Query(e.to_string())),
            Ok(refusals) if !refusals.is_empty() => return Err(AgentError::Policy(refusals)),
            Ok(_) => {}
        }

        let mode = RunMode::from(params.mode.unwrap_or_default());
        let page_size = self.resolved_page_size(params.page_size);

        let settled = self
            .host
            .run(&project.root, agent, session, params.sql, mode, page_size)
            .await?;
        if mode == RunMode::Run {
            self.forget(agent, &project.root, session);
        }
        let handle = params.query_session;
        match settled {
            Ok(Settled::Rows(output)) => {
                let cols = columns(&output.columns);
                self.remember(
                    agent,
                    &project.root,
                    session,
                    output.snapshot,
                    engine.id(),
                    Columns::clone(&cols),
                    output.total,
                    output.page_size,
                );
                Ok(rows_result(handle, cols, output))
            }
            Ok(Settled::Plan(plan)) => Ok(plan_result(handle, plan)),
            Err(EngineError::Stopped(stop)) => Ok(RunResult::Stopped {
                query_session: handle,
                reason: stop.to_string(),
            }),
            Err(e) => Err(AgentError::Query(e.to_string())),
        }
    }

    pub async fn read_page(&self, params: ReadPageParams) -> Result<PageResult, AgentError> {
        self.read_page_as(self.connection.agent, params).await
    }

    async fn read_page_as(
        &self,
        agent: AgentId,
        params: ReadPageParams,
    ) -> Result<PageResult, AgentError> {
        let session = session_handle(&params.query_session)?;
        let (project, engine) = self.engine(params.project.as_deref()).await?;
        let Some(last) = self.recall(agent, &project.root, session) else {
            return Err(AgentError::no_result(&params.query_session));
        };
        if last.engine != engine.id() {
            self.forget(agent, &project.root, session);
            return Err(AgentError::ResultMoved);
        }
        let page = params.page.max(1);

        let Some(snapshot) = last.snapshot else {
            return Ok(PageResult {
                query_session: params.query_session,
                columns: last.columns,
                rows: Vec::new(),
                total: 0,
                page,
                page_size: last.page_size,
            });
        };

        let sort = params.sort.map(|s| (s.column, s.ascending));
        let q = PageQuery {
            page,
            page_size: last.page_size,
            sort,
        };
        let display = engine.display();
        match engine.snapshot(snapshot).page(q, display).await {
            Ok(read) => Ok(PageResult {
                query_session: params.query_session,
                columns: last.columns,
                rows: cells(&read.rows),
                total: last.total,
                page,
                page_size: last.page_size,
            }),
            Err(e) => {
                if engine.snapshot(snapshot).live() {
                    Err(AgentError::Query(e.to_string()))
                } else {
                    self.forget(agent, &project.root, session);
                    Err(AgentError::ResultMoved)
                }
            }
        }
    }

    /// **The vocabulary's one write**: a query session's settled result, on the user's
    /// disk, at a path the caller names.
    ///
    /// Reaches the engine directly, exactly as [`read_page`](Self::read_page) does and for the
    /// same reason — the source is the session's own snapshot, which this layer already holds,
    /// and no host has anything to add to a write that touches no window state. So the app, the
    /// headless server and the in-process assistant all answer it from the engine they already
    /// hand over, with no [`Host`] method and no channel hop behind it.
    ///
    /// **It is not a loosening of [`run`](Self::run).** A typed `COPY` is still refused the
    /// agent's own `COPY`, the classification is untouched, and this writes nowhere a statement
    /// could reach anyway: `SnapshotReads::export_to`'s fence is the whole of what a caller-named
    /// path is allowed to be. What made a consent gate pointless is that `read_page` already
    /// hands over every byte — which is why a consent gate was considered and declined.
    ///
    /// A run that returned **no rows** materialized nothing, so there is no snapshot table to copy
    /// from and this refuses rather than writing an empty file. That is the one place it parts
    /// company with [`read_page`](Self::read_page), whose empty page is the honest answer to the
    /// same state: a file claims to be the result, and a header row over no rows is a claim about
    /// data the run never produced.
    pub async fn export_result(
        &self,
        params: ExportResultParams,
    ) -> Result<ExportResult, AgentError> {
        self.export_result_as(self.connection.agent, params).await
    }

    async fn export_result_as(
        &self,
        agent: AgentId,
        params: ExportResultParams,
    ) -> Result<ExportResult, AgentError> {
        let session = session_handle(&params.query_session)?;
        let (project, engine) = self.engine(params.project.as_deref()).await?;
        let Some(last) = self.recall(agent, &project.root, session) else {
            return Err(AgentError::no_result(&params.query_session));
        };
        if last.engine != engine.id() {
            self.forget(agent, &project.root, session);
            return Err(AgentError::ResultMoved);
        }
        let Some(snapshot) = last.snapshot else {
            return Err(AgentError::Query(format!(
                "The result in query session '{}' has no rows, so there is nothing to write.",
                params.query_session
            )));
        };
        let format = export_format(&params.format, &engine.formats())?;
        match engine
            .snapshot(snapshot)
            .export_to(params.path, format)
            .await
        {
            Ok(report) => Ok(ExportResult::from((params.query_session, report))),
            Err(e) if engine.snapshot(snapshot).live() => Err(AgentError::Query(e.to_string())),
            Err(_) => {
                self.forget(agent, &project.root, session);
                Err(AgentError::ResultMoved)
            }
        }
    }

    pub async fn close_query_session(
        &self,
        params: QuerySessionParams,
    ) -> Result<QuerySessionResult, AgentError> {
        self.close_query_session_as(self.connection.agent, params)
            .await
    }

    async fn close_query_session_as(
        &self,
        agent: AgentId,
        params: QuerySessionParams,
    ) -> Result<QuerySessionResult, AgentError> {
        let session = session_handle(&params.query_session)?;
        let project = self.project(params.project.as_deref()).await?;
        self.host
            .close_query_session(&project.root, agent, session)
            .await?;
        self.forget(agent, &project.root, session);
        Ok(QuerySessionResult {
            query_session: params.query_session,
        })
    }
}

/// **The tools as MCP advertises and serves them** — a wrapper each, over the bodies above.
///
/// A wrapper's whole job is the two concerns a semantic call cannot have: resolving which
/// agent the *request* is (see [`Caller`]) and holding that agent against the idle sweep for
/// the call's length. `#[tool(name = …)]` keeps the wire name, so renaming these methods to
/// leave the plain names to the facade changes nothing a client sees.
///
/// **The doc comments here are model-facing prose, on two transports now**: rmcp harvests
/// them as each tool's `description`, and [`StrataTools::manifest`] hands that same text to
/// the assistant's model. Written for exactly that register already — terse, second person,
/// naming the recovery — so they read the same either way.
#[allow(clippy::doc_markdown)]
#[tool_router]
impl<H: Host> StrataTools<H> {
    /// List the open Strata projects: name and root folder. Every other tool takes an
    /// optional 'project' naming one of these, needed only when more than one is open.
    #[tool(name = "list_projects", annotations(read_only_hint = true))]
    async fn list_projects_tool(&self, caller: Caller) -> Json<ProjectsResult> {
        let _busy = self.touch(&caller);
        Json(self.list_projects().await)
    }

    /// List a project's catalog: registered tables, saved views and saved queries, each with
    /// its source and whether the engine accepted it. This is the catalog as the app shows
    /// it, so a def the engine refused is listed with its error rather than silently missing.
    /// The answer states its total; a large catalog is paged (50 per page, 'page' reads on)
    /// and 'matching' narrows by name substring. A view row carries a one-line SQL preview;
    /// describe_table returns the full text. 'databases' names the catalogs the project's
    /// database connections registered: their relations are not entries here, are read with a
    /// three-part name ('pg.public.orders'), are listed by SHOW TABLES, and are described one
    /// at a time by describe_table.
    #[tool(name = "list_tables", annotations(read_only_hint = true))]
    async fn list_tables_tool(
        &self,
        caller: Caller,
        Parameters(params): Parameters<ListTablesParams>,
    ) -> Result<Json<TablesResult>, AgentError> {
        let _busy = self.touch(&caller);
        Ok(Json(self.list_tables(params).await?))
    }

    /// Describe one table or view: its columns and types, nested fields, Hive partition
    /// columns, source paths and format, plus the row count and column statistics the source
    /// reports for free. Only facts that were read — nothing is scanned or estimated. A deep
    /// or wide schema is bounded: elided children appear as counts, 'path' (name segments,
    /// exactly as an answer printed them) descends to any nested column, 'matching' finds
    /// fields by name substring anywhere in the tree and answers with their paths, and
    /// 'page' reads more columns or matches. An answer with no totals in it is complete.
    /// Where an object is keyed by data — thousands of same-shaped fields under UUID keys —
    /// the keys collapse into one entry named `<key>`, carrying 'keys_total' (how many keys
    /// share that shape) and 'key_examples' (a few of them, spelled as the file spells them).
    /// `<key>` is a placeholder, not a field name: to descend into one of those keys, put a
    /// real key from 'key_examples' in the path. A 'matching' row through a collapsed set is
    /// likewise one row, with 'matched_keys' saying how many fields it stands for. Every field
    /// name here is spelled the way the file spells it, and SQL lowercases an unquoted
    /// identifier by default, so a mixed-case name has to be quoted to resolve. A
    /// three-part name describes a relation in a database connection's catalog instead: its
    /// columns and types, and the connection it is in, with no def facts because it has none.
    #[tool(name = "describe_table", annotations(read_only_hint = true))]
    async fn describe_table_tool(
        &self,
        caller: Caller,
        Parameters(params): Parameters<DescribeTableParams>,
    ) -> Result<Json<DescribeResult>, AgentError> {
        let _busy = self.touch(&caller);
        Ok(Json(self.describe_table(params).await?))
    }

    /// List the SQL functions this project's engine has registered. What is registered is
    /// what exists. The answer states its total; a set of 30 or fewer comes back in full
    /// (overload signatures, return type, documentation), a larger one names only — narrow
    /// with 'matching', a case-insensitive name substring, to read a function in full.
    #[tool(name = "list_functions", annotations(read_only_hint = true))]
    async fn list_functions_tool(
        &self,
        caller: Caller,
        Parameters(params): Parameters<ListFunctionsParams>,
    ) -> Result<Json<FunctionsResult>, AgentError> {
        let _busy = self.touch(&caller);
        Ok(Json(self.list_functions(params).await?))
    }

    /// Check SQL without running it: lints, the read-only policy, and a dry plan against the
    /// real catalog. The cheap way to find a typo or a missing column before spending a run.
    #[tool(name = "validate", annotations(read_only_hint = true))]
    async fn validate_tool(
        &self,
        caller: Caller,
        Parameters(params): Parameters<ValidateParams>,
    ) -> Result<Json<ValidateResult>, AgentError> {
        let _busy = self.touch(&caller);
        Ok(Json(self.validate(params).await?))
    }

    /// Open a query session and return its handle: a place your queries run in sequence,
    /// each replacing the last. It is yours, not one of the user's editor tabs — nothing you
    /// do here disturbs what they are working on. Where Strata's window is open, the user can
    /// see what you run and promote any query you ran into their own editor. A session you
    /// have not used for 30 minutes may be retired: list_query_sessions is what you still
    /// hold, and opening a session again and re-running is cheap.
    #[tool(name = "open_query_session")]
    async fn open_query_session_tool(
        &self,
        peer: Peer<RoleServer>,
        caller: Caller,
        Parameters(params): Parameters<ProjectParams>,
    ) -> Result<Json<QuerySessionResult>, AgentError> {
        let (agent, _busy) = self.agent(&caller)?;
        Ok(Json(
            self.open_query_session_as(agent, caller.identity(&peer), params)
                .await?,
        ))
    }

    /// List your own query sessions in this project: handle, and whether a run is in flight,
    /// settled, or has never happened.
    #[tool(name = "list_query_sessions", annotations(read_only_hint = true))]
    async fn list_query_sessions_tool(
        &self,
        caller: Caller,
        Parameters(params): Parameters<ProjectParams>,
    ) -> Result<Json<QuerySessionsResult>, AgentError> {
        let (agent, _busy) = self.agent(&caller)?;
        Ok(Json(self.list_query_sessions_as(agent, params).await?))
    }

    /// Run read-only SQL in one of your query sessions and wait for it to settle. It runs on
    /// the project's real engine, so it costs and behaves exactly like a query the user
    /// presses Run on, and it replaces whatever that session last produced. Returns page 1
    /// plus the exact total; use read_page for the rest. Set mode to 'explain' for the query
    /// plan without executing.
    #[tool(name = "run")]
    async fn run_tool(
        &self,
        caller: Caller,
        Parameters(params): Parameters<RunParams>,
    ) -> Result<Json<RunResult>, AgentError> {
        let (agent, _busy) = self.agent(&caller)?;
        Ok(Json(self.run_as(agent, params).await?))
    }

    /// Read another page of a query session's last settled result. Pages are 1-based and use
    /// the page size that run used. The result is an immutable snapshot, so paging never
    /// re-runs the query — but a newer run in that session replaces it, and then this reports
    /// that.
    #[tool(name = "read_page", annotations(read_only_hint = true))]
    async fn read_page_tool(
        &self,
        caller: Caller,
        Parameters(params): Parameters<ReadPageParams>,
    ) -> Result<Json<PageResult>, AgentError> {
        let (agent, _busy) = self.agent(&caller)?;
        Ok(Json(self.read_page_as(agent, params).await?))
    }

    /// Write a query session's last settled result to a file on the user's machine, in csv,
    /// ndjson, parquet or arrow. The whole result, in result order, from the snapshot the
    /// session already holds: nothing is re-run and no row limit applies, so this is how a
    /// result larger than you want to read gets to disk. Give an absolute path to a file that
    /// does not exist, with an extension, in a folder that does exist: an export never
    /// overwrites, never creates folders, and cannot write inside the project's own '.strata'
    /// directory. A path with no extension, a trailing slash, or a '?', '*' or '[' in it is
    /// refused, because none of those name one file. Format options are the
    /// format's defaults; the app's Export window is where the user picks others. Reports the
    /// path written, the row count and the file's size.
    #[tool(name = "export_result", annotations(destructive_hint = false))]
    async fn export_result_tool(
        &self,
        caller: Caller,
        Parameters(params): Parameters<ExportResultParams>,
    ) -> Result<Json<ExportResult>, AgentError> {
        let (agent, _busy) = self.agent(&caller)?;
        Ok(Json(self.export_result_as(agent, params).await?))
    }

    /// Close one of your query sessions. A run still in flight in it is cancelled. Closing is
    /// tidy rather than required — every session you hold goes when you disconnect.
    #[tool(name = "close_query_session", annotations(destructive_hint = false))]
    async fn close_query_session_tool(
        &self,
        caller: Caller,
        Parameters(params): Parameters<QuerySessionParams>,
    ) -> Result<Json<QuerySessionResult>, AgentError> {
        let (agent, _busy) = self.agent(&caller)?;
        Ok(Json(self.close_query_session_as(agent, params).await?))
    }
}

#[tool_handler(
    router = Self::advertised(),
    name = "strata",
    instructions = "Strata is a local parquet/CSV/JSON query workspace over Apache DataFusion. \
SQL is read-only: SELECT, EXPLAIN, SHOW and DESCRIBE run; everything else is refused. \
Start with list_tables and describe_table to learn the catalog, validate to check SQL \
cheaply, then open_query_session and run. Your work lives in query sessions of your own, \
which the user can watch and promote into their editor wherever Strata's window is open — so \
it never disturbs the tabs they are working in. Open a session per line of investigation; each run \
in a session replaces the last one's result. export_result saves a session's whole result to a \
file on the user's machine, which is how a result too large to read reaches disk."
)]
impl<H: Host> ServerHandler for StrataTools<H> {}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::{env, process};

    use strata_engine::{DenyCode, Form, Reason, StmtKind};
    use strata_engine::{EngineError, RunTag, StopReason, TableSpec, WsId};
    use strata_model::SourceFormat;

    use crate::assistant::SYSTEM;
    use crate::host::{CatalogEntry, Described, QuerySessionState, RegState};
    use crate::mock::{MockHost, MockProject};
    use crate::wire::{EntryWire, Mode, QuerySessionStateWire, Sort, StateWire};

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
            .catalog()
            .register(TableSpec {
                name: "people".into(),
                paths: vec![root.join("people.csv").display().to_string()],
                format: SourceFormat::from_name("csv"),
                partitions: Vec::new(),
                connection: None,
                internal: false,
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
            .open_query_session(claude(), no_project())
            .await
            .unwrap()
            .query_session
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

    /// **The in-app mark is minted, never claimed.** It rides `Agent::in_app` to the host on
    /// the call that opens a session, so a surface can tell the app's own assistant from a
    /// client without holding an id — and a transport-built value is false by construction, so
    /// a client calling itself `strata-assistant` cannot set it.
    #[tokio::test]
    async fn only_an_in_app_value_opens_in_app_agents() {
        let root = scratch("inapp");
        let host = MockHost::new(vec![MockProject::new("sales", &root)]);

        let assistant = StrataTools::in_app(Arc::clone(&host));
        assistant
            .open_query_session(AgentIdentity::assistant(), no_project())
            .await
            .unwrap();

        let dialled = StrataTools::new(Arc::clone(&host));
        dialled
            .open_query_session(claude(), no_project())
            .await
            .unwrap();

        let liar = dialled.connection();
        liar.open_query_session(AgentIdentity::assistant(), no_project())
            .await
            .unwrap();

        let opened = host.opened();
        assert_eq!(opened.len(), 3);
        assert_eq!(
            opened.iter().filter(|a| a.in_app).count(),
            1,
            "only the value built by `in_app` is marked: {opened:?}"
        );
        assert!(opened[0].in_app, "the assistant opened first");
    }

    #[tokio::test]
    async fn list_projects_names_the_open_windows() {
        let tools = StrataTools::new(MockHost::new(vec![
            MockProject::new("sales", "/w/sales"),
            MockProject::new("ops", "/w/ops"),
        ]));
        let listed = tools.list_projects().await.projects;
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
        let Err(e) = tools.list_tables(ListTablesParams::default()).await else {
            panic!("expected an ambiguous-project error");
        };
        let text = e.to_string();
        assert!(text.contains("sales (/w/sales)"), "{text}");
        assert!(text.contains("ops (/w/ops)"), "{text}");

        let named = tools
            .list_tables(ListTablesParams {
                project: Some("ops".into()),
                ..ListTablesParams::default()
            })
            .await
            .unwrap();
        assert!(named.entries.is_empty());
    }

    /// The catalog as the store shows it: a def the engine refused is a row with its error,
    /// not a missing row.
    #[tokio::test]
    async fn list_tables_reports_a_failed_def_with_its_error() {
        let (_root, tools) = one_project("list_tables").await;
        let entries = tools
            .list_tables(ListTablesParams::default())
            .await
            .unwrap()
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
            .describe_table(DescribeTableParams {
                name: "people".into(),
                ..DescribeTableParams::default()
            })
            .await
            .unwrap();
        assert_eq!(described.name, "people");
        assert_eq!(described.format.as_deref(), Some("csv"));
        let columns: Vec<&str> = described.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(columns, vec!["id", "name"]);
    }

    #[tokio::test]
    async fn describe_table_on_an_unknown_name_is_not_found() {
        let (_root, tools) = one_project("describe_unknown").await;
        let Err(AgentError::NotFound(message)) = tools
            .describe_table(DescribeTableParams {
                name: "nope".into(),
                ..DescribeTableParams::default()
            })
            .await
        else {
            panic!("expected a not-found error");
        };
        assert!(message.contains("'nope'"), "{message}");
    }

    /// **The remote fallback does not swallow the store's answer**. A qualified name
    /// with no database behind it resolves nowhere, so what comes back is the host's own
    /// not-found — never a blank remote row, and never a different error for the same fault
    /// spelled with dots.
    ///
    /// What the fallback does when a catalog *is* registered is the engine's
    /// (`engine::remote_catalog_tests`, and `tests/postgres_federation.rs` against a server);
    /// what it renders is `describe`'s (`a_remote_relation_describes_as_itself`). This is the
    /// glue between them, and the only thing it can be wrong about is which error wins.
    #[tokio::test]
    async fn a_qualified_name_with_no_database_is_still_not_found() {
        let (_root, tools) = one_project("describe_qualified").await;
        let Err(AgentError::NotFound(message)) = tools
            .describe_table(DescribeTableParams {
                name: "pg.public.orders".into(),
                ..DescribeTableParams::default()
            })
            .await
        else {
            panic!("expected a not-found error");
        };
        assert!(message.contains("'pg.public.orders'"), "{message}");
    }

    /// A project with no database connection says so by omission, and the field is what a
    /// later one will appear in — pinned so the listing cannot start inventing catalogs.
    #[tokio::test]
    async fn list_tables_names_no_databases_when_there_are_none() {
        let (_root, tools) = one_project("list_databases").await;
        let listed = tools
            .list_tables(ListTablesParams::default())
            .await
            .unwrap();
        assert!(listed.databases.is_empty());
        assert!(!listed.entries.is_empty(), "and the defs are still listed");
    }

    /// The function list is the live registry, so it carries DataFusion's built-ins and the
    /// JSON accessors `build_context` registers — no second list to keep in step.
    #[tokio::test]
    async fn list_functions_is_the_live_registry() {
        let (_root, tools) = one_project("functions").await;
        let functions = tools
            .list_functions(ListFunctionsParams::default())
            .await
            .unwrap();
        assert!(functions.scalar.iter().any(|f| f.name == "json_get"));
        assert!(functions.aggregate.iter().any(|f| f.name == "count"));
        assert!(!functions.window.is_empty());
        assert_eq!(
            functions.total,
            functions.scalar.len() + functions.aggregate.len() + functions.window.len()
        );
        assert!(functions
            .note
            .as_deref()
            .is_some_and(|n| n.contains("'matching'")));
    }

    #[tokio::test]
    async fn validate_finds_a_missing_table_without_running_anything() {
        let (_root, tools) = one_project("validate").await;
        let clean = tools
            .validate(ValidateParams {
                sql: "SELECT id FROM people".into(),
                project: None,
            })
            .await
            .unwrap();
        assert!(clean.diagnostics.is_empty(), "{clean:?}");

        let broken = tools
            .validate(ValidateParams {
                sql: "SELECT id FROM nope".into(),
                project: None,
            })
            .await
            .unwrap();
        assert!(
            broken
                .diagnostics
                .iter()
                .any(|d| d.message.contains("nope")),
            "{broken:?}"
        );
    }

    #[tokio::test]
    async fn query_sessions_open_list_and_close() {
        let (_root, tools) = one_project("sessions").await;
        let session = open(&tools).await;

        let listed = tools
            .list_query_sessions(no_project())
            .await
            .unwrap()
            .query_sessions;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].query_session, session);
        assert!(matches!(listed[0].state, QuerySessionStateWire::Empty));

        tools
            .close_query_session(QuerySessionParams {
                query_session: session.clone(),
                project: None,
            })
            .await
            .unwrap();
        assert!(tools
            .list_query_sessions(no_project())
            .await
            .unwrap()
            .query_sessions
            .is_empty());

        let Err(AgentError::NotFound(_)) = tools
            .close_query_session(QuerySessionParams {
                query_session: session,
                project: None,
            })
            .await
        else {
            panic!("expected a not-found error");
        };
    }

    /// **An agent sees its own sessions and nothing else.** An earlier `list_tabs` handed over
    /// every open tab, including the user's.
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
            .list_query_sessions(no_project())
            .await
            .unwrap()
            .query_sessions;
        let theirs = second
            .list_query_sessions(no_project())
            .await
            .unwrap()
            .query_sessions;
        assert_eq!(mine.len(), 1);
        assert_eq!(theirs.len(), 1);
        assert_ne!(mine[0].query_session, theirs[0].query_session);

        for reached in [
            second.run(run_params(&borrowed, "SELECT 1")).await.err(),
            second
                .close_query_session(QuerySessionParams {
                    query_session: borrowed.clone(),
                    project: None,
                })
                .await
                .err(),
        ] {
            assert!(
                matches!(reached, Some(AgentError::NotFound(_))),
                "another agent's session is simply not there: {reached:?}"
            );
        }
    }

    /// **The bound a model plans against is the constant that enforces it.** Two surfaces
    /// state the idle window in prose — this tool description and the assistant's system
    /// prompt — and a doc comment cannot interpolate a `Duration`, so the agreement is
    /// checked rather than generated.
    ///
    /// The wording is deliberately "may be retired": the sweep is the *stateless* branch's, and
    /// a connected client's sessions live until its connection drops. A ceiling is honest for
    /// both, a promise of retirement would be false for one of them, and the description is one
    /// text that reaches every caller.
    #[test]
    fn the_stated_idle_bound_is_the_constant() {
        let minutes = STATELESS_IDLE.as_secs() / 60;
        let stated = format!("{minutes} minutes");

        let tools = StrataTools::new(MockHost::new(Vec::new()));
        let open = tools
            .manifest()
            .into_iter()
            .find(|spec| spec.name == "open_query_session")
            .expect("the router offers open_query_session");
        assert!(
            open.description.contains(&stated),
            "the tool description states the window: {}",
            open.description
        );
        assert!(
            SYSTEM.contains(&stated),
            "the assistant's system prompt states the same window"
        );
    }

    /// **The idle sweep never takes an agent with a call in flight**, and that is what makes
    /// `retire_all` a separate method rather than `retire_idle(ZERO)`.
    ///
    /// The guard is held by `agent()` for the length of a tool call, so a `run` slower than the
    /// idle window is not retired out from under itself — the engine would abort its query and
    /// the vocabulary would report that back as "you stopped this".
    #[tokio::test]
    async fn the_idle_sweep_spares_an_agent_with_a_call_in_flight() {
        let (root, tools) = one_project("busy_sweep").await;
        let who = AgentIdentity {
            name: "claude-code".into(),
            version: "2.1.4".into(),
        };
        let caller = Caller::Stateless(who);
        let (agent, busy) = tools.agent(&caller).unwrap();
        tools
            .open_query_session_as(agent, AgentIdentity::default(), no_project())
            .await
            .unwrap();

        tools.retire_idle(Duration::ZERO);
        assert_eq!(
            tools.host.query_sessions(&root, agent).await.unwrap().len(),
            1,
            "a working agent survives the sweep however long it has taken"
        );

        drop(busy);
        tools.retire_idle(STATELESS_IDLE);
        assert_eq!(
            tools.host.query_sessions(&root, agent).await.unwrap().len(),
            1,
            "and the idle window restarts when the call ends"
        );

        tools.retire_idle(Duration::ZERO);
        assert!(tools
            .host
            .query_sessions(&root, agent)
            .await
            .unwrap()
            .is_empty());
    }

    /// **Stopping the server retracts a working agent too.** `retire_idle(ZERO)` cannot do this
    /// — `idle` requires `busy == 0`, so it skips exactly the agents that matter — and nothing
    /// else would ever say `agent_gone` for them: the runtime is dropped a moment later, and
    /// `Busy`'s drop only decrements a counter. Without this the agent's row and its query
    /// sessions would outlive the server that minted them, holding engine workspaces for the
    /// window's life.
    #[tokio::test]
    async fn stopping_retracts_even_an_agent_mid_call() {
        let (root, tools) = one_project("retire_all").await;
        let caller = Caller::Stateless(AgentIdentity {
            name: "claude-code".into(),
            version: "2.1.4".into(),
        });
        let (agent, _busy) = tools.agent(&caller).unwrap();
        tools
            .open_query_session_as(agent, AgentIdentity::default(), no_project())
            .await
            .unwrap();

        tools.retire_all();

        assert!(
            tools
                .host
                .query_sessions(&root, agent)
                .await
                .unwrap()
                .is_empty(),
            "stopping the server releases a working agent's sessions"
        );
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
        let Err(AgentError::NotFound(message)) =
            tools.run(run_params("not-a-uuid", "SELECT 1")).await
        else {
            panic!("expected a not-found error");
        };
        assert!(message.contains("not-a-uuid"), "{message}");
    }

    #[tokio::test]
    async fn run_returns_page_one_and_the_exact_total() {
        let (_root, tools) = one_project("run").await;
        let session = open(&tools).await;
        let mut params = run_params(&session, "SELECT id, name FROM people ORDER BY id");
        params.page_size = Some(2);

        let result = tools.run(params).await.unwrap();
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
            .run(run_params(&session, "SELECT id FROM people"))
            .await
            .unwrap()
        else {
            panic!("expected rows");
        };

        let engine = tools.host.engine(&root).await.unwrap();
        let ws = WsId::from(QuerySessionId(Uuid::parse_str(&session).unwrap()));

        engine
            .ws(ws)
            .query(RunTag(4242), "SELECT name FROM people".into(), 10)
            .await
            .unwrap();
        assert!(
            matches!(
                tools
                    .read_page(ReadPageParams {
                        query_session: session,
                        page: 1,
                        sort: None,
                        project: None,
                    })
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
            .run(run_params(&session, "SELECT id FROM people"))
            .await
            .unwrap()
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
                .run(run_params(&session, "SELECT id FROM people"))
                .await
                .unwrap();
            sessions.push(session);
        }
        assert_eq!(tools.runs.lock().unwrap().len(), MAX_REMEMBERED_RUNS);

        let evicted = tools
            .read_page(ReadPageParams {
                query_session: sessions[0].clone(),
                page: 1,
                sort: None,
                project: None,
            })
            .await;
        assert!(
            matches!(evicted, Err(AgentError::NotFound(_))),
            "the oldest is evicted"
        );
        assert!(tools
            .read_page(ReadPageParams {
                query_session: sessions.pop().unwrap(),
                page: 1,
                sort: None,
                project: None,
            })
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
        mine.run(run_params(&kept, "SELECT id FROM people"))
            .await
            .unwrap();

        for _ in 0..MAX_REMEMBERED_RUNS + 4 {
            let session = open(&theirs).await;
            theirs
                .run(run_params(&session, "SELECT id FROM people"))
                .await
                .unwrap();
        }

        assert!(
            mine.read_page(ReadPageParams {
                query_session: kept,
                page: 1,
                sort: None,
                project: None,
            })
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
        async fn register(engine: &Engine, root: &Path) {
            engine
                .catalog()
                .register(TableSpec {
                    name: "people".into(),
                    paths: vec![root.join("people.csv").display().to_string()],
                    format: SourceFormat::from_name("csv"),
                    partitions: Vec::new(),
                    connection: None,
                    internal: false,
                })
                .await
                .unwrap();
        }

        let root = scratch("engine_swap");
        fs::write(root.join("people.csv"), "id,name\n1,ana\n2,ben\n").unwrap();
        let project = MockProject::new("sales", &root);
        register(&project.engine, &root).await;
        let tools = StrataTools::new(MockHost::new(vec![project]));

        let session = open(&tools).await;
        tools
            .run(run_params(&session, "SELECT id FROM people"))
            .await
            .unwrap();

        let replacement = MockProject::new("sales", &root);
        register(&replacement.engine, &root).await;
        tools.host.replace_engine(&root, replacement.engine.clone());
        replacement
            .engine
            .ws(WsId(9))
            .query(RunTag(1), "SELECT name FROM people".into(), 10)
            .await
            .unwrap();

        assert!(
            matches!(
                tools
                    .read_page(ReadPageParams {
                        query_session: session,
                        page: 1,
                        sort: None,
                        project: None,
                    })
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

        let RunResult::Ok { page_size, .. } = tools.run(params).await.unwrap() else {
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
            .run(run_params(
                &session,
                "CREATE TABLE copy AS SELECT * FROM people",
            ))
            .await
        else {
            panic!("expected a policy refusal");
        };
        assert_eq!(
            e.to_string(),
            Reason::Policy {
                form: Form::Statement(StmtKind::CreateTable),
                code: DenyCode::NotGranted,
            }
            .message()
        );
    }

    /// Fail closed: input that cannot be judged is never a policy pass.
    #[tokio::test]
    async fn run_refuses_input_it_cannot_parse() {
        let (_root, tools) = one_project("unparseable").await;
        let session = open(&tools).await;
        assert!(matches!(
            tools.run(run_params(&session, "SELECT FROM WHERE )")).await,
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
        } = tools.run(params).await.unwrap()
        else {
            panic!("expected a plan");
        };
        assert!(!analyze);
        assert!(logical.contains("people"), "{logical}");
        assert!(!physical.is_empty());

        let Err(AgentError::NotFound(_)) = tools
            .read_page(ReadPageParams {
                query_session: session,
                page: 1,
                sort: None,
                project: None,
            })
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
        let tools =
            StrataTools::new(MockHost::new(vec![MockProject::new("sales", &root)
                .settling(EngineError::Stopped(StopReason::Cancelled))]));
        let session = open(&tools).await;
        let result = tools
            .run(run_params(&session, "SELECT 1"))
            .await
            .expect("a stop is not an error");
        let RunResult::Stopped { reason, .. } = result else {
            panic!("{result:?}");
        };
        assert_eq!(reason, StopReason::Cancelled.to_string());
    }

    #[tokio::test]
    async fn run_against_an_unknown_query_session_is_not_found() {
        let (_root, tools) = one_project("run_unknown").await;
        let stray = QuerySessionId::new().0.to_string();
        assert!(matches!(
            tools.run(run_params(&stray, "SELECT 1")).await,
            Err(AgentError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn read_page_walks_the_settled_snapshot() {
        let (_root, tools) = one_project("read_page").await;
        let session = open(&tools).await;
        let mut params = run_params(&session, "SELECT id FROM people ORDER BY id");
        params.page_size = Some(2);
        tools.run(params).await.unwrap();

        let page = tools
            .read_page(ReadPageParams {
                query_session: session.clone(),
                page: 3,
                sort: None,
                project: None,
            })
            .await
            .unwrap();
        assert_eq!(page.page_size, 2);
        assert_eq!(page.total, 5);
        assert_eq!(page.rows, vec![vec![Some("5".to_string())]]);

        let sorted = tools
            .read_page(ReadPageParams {
                query_session: session,
                page: 1,
                sort: Some(Sort {
                    column: "id".into(),
                    ascending: false,
                }),
                project: None,
            })
            .await
            .unwrap();
        assert_eq!(sorted.rows[0], vec![Some("5".to_string())]);
    }

    /// A newer run in that session retires the snapshot. The read must say so — and say it
    /// from the engine's answer to "is this still there?", never from its prose.
    #[tokio::test]
    async fn read_page_reports_a_result_a_newer_run_replaced() {
        let (root, tools) = one_project("moved").await;
        let session = open(&tools).await;
        tools
            .run(run_params(&session, "SELECT id FROM people"))
            .await
            .unwrap();

        let engine = tools.host.engine(&root).await.unwrap();
        engine
            .ws(WsId(Uuid::parse_str(&session).unwrap().as_u128()))
            .query(RunTag(999), "SELECT name FROM people".into(), 10)
            .await
            .unwrap();

        assert!(matches!(
            tools
                .read_page(ReadPageParams {
                    query_session: session.clone(),
                    page: 1,
                    sort: None,
                    project: None,
                })
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
            .run(run_params(&session, "SELECT id FROM people WHERE id > 99"))
            .await
            .unwrap();

        let page = tools
            .read_page(ReadPageParams {
                query_session: session,
                page: 1,
                sort: None,
                project: None,
            })
            .await
            .unwrap();
        assert_eq!(page.total, 0);
        assert!(page.rows.is_empty());
    }

    /// **The one write in the vocabulary, end to end**: the session's settled result on
    /// disk, at a path the caller named, with the figures the write pass produced.
    ///
    /// The file is read back rather than trusted: the ordinal column must not be in it, the row
    /// order must be the result's, and `bytes` must be the size of the file that is actually
    /// there. Driven through a real engine, because the claim is that this is the export the
    /// window makes, reached by a caller with no dialog in front of it.
    #[tokio::test]
    async fn export_result_writes_the_settled_result_to_the_path_it_was_given() {
        let (root, tools) = one_project("export").await;
        let session = open(&tools).await;
        tools
            .run(run_params(
                &session,
                "SELECT id, name FROM people ORDER BY id DESC",
            ))
            .await
            .unwrap();

        let out = root.join("people.parquet");
        let written = tools
            .export_result(export_params(&session, &out))
            .await
            .expect("exported");
        assert_eq!(written.query_session, session);
        assert_eq!(written.path, out.display().to_string());
        assert_eq!(written.rows, 5);
        assert_eq!(written.bytes, Some(fs::metadata(&out).unwrap().len()));

        let csv = root.join("people.csv2");
        tools
            .export_result(ExportResultParams {
                format: "csv".into(),
                ..export_params(&session, &csv)
            })
            .await
            .expect("exported");
        assert_eq!(
            fs::read_to_string(&csv).unwrap(),
            "id,name\n5,eli\n4,dev\n3,cara\n2,ben\n1,ana\n"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **Every refusal names its own reason**, because each one has a different recovery: run
    /// something, re-run it, or pick another path. The path rules are the engine's fence
    /// (`SnapshotReads::export_to`); the two session ones are this layer's, and "no result" is the
    /// same sentence `read_page` gives for the same condition.
    #[tokio::test]
    async fn export_result_names_the_reason_it_refused() {
        let (root, tools) = one_project("export_refusals").await;
        let session = open(&tools).await;
        let out = root.join("out.parquet");

        let stray = QuerySessionId::new().0.to_string();
        assert!(matches!(
            tools.export_result(export_params(&stray, &out)).await,
            Err(AgentError::NotFound(_))
        ));

        let unrun = tools
            .export_result(export_params(&session, &out))
            .await
            .expect_err("nothing has run in it");
        assert_eq!(unrun, AgentError::no_result(&session));

        tools
            .run(run_params(&session, "SELECT id FROM people WHERE id > 99"))
            .await
            .unwrap();
        let empty = tools
            .export_result(export_params(&session, &out))
            .await
            .expect_err("no rows were materialized")
            .to_string();
        assert!(empty.contains("has no rows"), "{empty}");

        tools
            .run(run_params(&session, "SELECT id FROM people"))
            .await
            .unwrap();
        tools
            .export_result(export_params(&session, &out))
            .await
            .expect("exported");
        let taken = tools
            .export_result(export_params(&session, &out))
            .await
            .expect_err("already there")
            .to_string();
        assert!(taken.contains("never overwrites"), "{taken}");

        let relative = tools
            .export_result(ExportResultParams {
                path: "out.parquet".into(),
                ..export_params(&session, &out)
            })
            .await
            .expect_err("relative")
            .to_string();
        assert!(relative.contains("absolute path"), "{relative}");

        let extensionless = tools
            .export_result(export_params(&session, &root.join("results")))
            .await
            .expect_err("would be written as a folder of part files")
            .to_string();
        assert!(
            extensionless.contains("no file extension"),
            "{extensionless}"
        );
        assert!(
            !root.join("results").exists(),
            "and the refusal comes before the write"
        );

        let owned = tools
            .export_result(export_params(
                &session,
                &root.join(".strata/tables/sales/rows.parquet"),
            ))
            .await
            .expect_err("inside .strata")
            .to_string();
        assert!(owned.contains("this project's own data"), "{owned}");
        let _ = fs::remove_dir_all(&root);
    }

    /// A newer run in the session retires the snapshot the export would have read, and the
    /// answer is the same "re-run to read it" a page gets — never a half-written file.
    #[tokio::test]
    async fn export_result_reports_a_result_a_newer_run_replaced() {
        let (root, tools) = one_project("export_moved").await;
        let session = open(&tools).await;
        tools
            .run(run_params(&session, "SELECT id FROM people"))
            .await
            .unwrap();

        let engine = tools.host.engine(&root).await.unwrap();
        engine
            .ws(WsId(Uuid::parse_str(&session).unwrap().as_u128()))
            .query(RunTag(999), "SELECT name FROM people".into(), 10)
            .await
            .unwrap();

        let out = root.join("stale.parquet");
        assert!(matches!(
            tools.export_result(export_params(&session, &out)).await,
            Err(AgentError::ResultMoved)
        ));
        assert!(!out.exists(), "a refusal writes nothing");
        let _ = fs::remove_dir_all(&root);
    }

    /// An export of `session`'s result to `path`, as parquet.
    fn export_params(session: &str, path: &Path) -> ExportResultParams {
        ExportResultParams {
            query_session: session.into(),
            path: path.display().to_string(),
            format: "parquet".into(),
            project: None,
        }
    }

    /// **The manifest is the router, not a copy of it.** Asserted here rather than only in
    /// `tests/facade.rs` because this is the only side of the crate boundary that can see the
    /// router at all — outside it, a manifest that had quietly become a hand-kept list would
    /// look exactly the same until the two disagreed.
    ///
    /// What the names, descriptions and schemas *say* is `tests/facade.rs`'s: it checks them
    /// against the wire, which is the thing they have to agree with.
    #[test]
    fn the_manifest_is_derived_from_the_router_that_serves_mcp() {
        let tools = StrataTools::new(MockHost::new(Vec::new()));
        let mut router = StrataTools::<MockHost>::advertised().list_all();
        let manifest = tools.manifest();

        router.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(manifest.len(), router.len());
        for (spec, tool) in manifest.iter().zip(&router) {
            assert_eq!(spec.name, tool.name);
            assert_eq!(
                Some(spec.description.as_str()),
                tool.description.as_deref(),
                "{} carries the router's description",
                spec.name
            );
            assert_eq!(
                spec.input_schema,
                Value::Object(JsonObject::clone(&tool.input_schema)),
                "{} carries the router's schema",
                spec.name
            );
        }
    }

    /// **Nothing a tool advertises states a `format` JSON Schema does not define.**
    ///
    /// The widths schemars writes for Rust's integers (`uint`, `uint64`) are the ones that get
    /// here on their own, and both schemas are checked because both are compiled by whoever
    /// reads them: a client that validates what it is handed refuses a tool over a format it
    /// does not know, and refuses the `tools/list` that carried it.
    #[test]
    fn a_tool_advertises_no_format_json_schema_does_not_define() {
        fn formats(value: &Value, into: &mut Vec<String>) {
            match value {
                Value::Object(map) => {
                    if let Some(format) = map.get("format").and_then(Value::as_str) {
                        into.push(format.to_string());
                    }
                    map.values().for_each(|node| formats(node, into));
                }
                Value::Array(items) => items.iter().for_each(|node| formats(node, into)),
                _ => {}
            }
        }

        for tool in StrataTools::<MockHost>::advertised().list_all() {
            let mut stated = Vec::new();
            formats(
                &Value::Object(JsonObject::clone(&tool.input_schema)),
                &mut stated,
            );
            if let Some(schema) = &tool.output_schema {
                formats(&Value::Object(JsonObject::clone(schema)), &mut stated);
            }
            let unknown: Vec<&String> = stated
                .iter()
                .filter(|format| !JSON_SCHEMA_FORMATS.contains(&format.as_str()))
                .collect();
            assert!(unknown.is_empty(), "{} advertises {unknown:?}", tool.name);
        }
    }

    /// The mock's `QuerySessionState` reaches the wire verbatim — pinned because the enum is
    /// the one shape a well-meaning "unknown" arm could be added to.
    #[test]
    fn a_session_state_crosses_the_wire_unchanged() {
        let wire = crate::wire::QuerySessionWire::from(host::QuerySessionInfo {
            session: QuerySessionId::new(),
            state: QuerySessionState::Running,
        });
        assert!(matches!(wire.state, QuerySessionStateWire::Running));
    }
}
