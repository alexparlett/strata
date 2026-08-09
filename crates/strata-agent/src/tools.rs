//! The **vocabulary** — the ten read-only tools of `docs/AGENT_ACCESS_SPEC.md`, over a
//! [`Host`].
//!
//! [`StrataTools`] is the rmcp `ServerHandler`, and it is deliberately transport-free: the
//! Streamable-HTTP server ([`crate::server`]) serves it, the headless host (AA-05) serves the
//! same value over stdio, and the assistant (AS-01) calls it in-process. One surface, three
//! frontends.
//!
//! ## The vocabulary is methods; the tools are wrappers (AS-01)
//!
//! The file is in two halves. The public methods on [`StrataTools`] *are* the ten tools —
//! plain arguments, plain answers, no rmcp type in any signature — and the `#[tool_router]`
//! block below them is one wrapper each, doing only what a semantic call cannot: resolving
//! which agent the *request* is ([`Caller`]) and holding that agent against the idle sweep.
//! An in-process caller with no MCP peer anywhere therefore reaches the identical body, with
//! the policy gate, the run cache, the scoping key and every message included.
//!
//! [`StrataTools::manifest`] is the other half of that: the vocabulary as data, **derived
//! from the router**, so what an in-process loop offers its model is what `tools/list`
//! advertises, with no second list to keep in step.
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
//! ## One agent per client, and the request says which (AA-03b, corrected by AA-03c)
//!
//! A [`StrataTools`] *is* one agent: it carries a [`Connection`], which mints an [`AgentId`]
//! and retracts it on drop, and every session-scoped answer is scoped by that id rather than
//! by a check somebody has to remember — the AA-03 hole restated as a type, so an agent has
//! no handle on another agent's work, nor on the user's tabs, because it never receives one.
//!
//! What AA-03c corrects is *where the id comes from*. A value's lifetime is the connection's
//! on only some of the transport's paths — rmcp's stateless branch builds one service per
//! **request** — so the id is resolved from the request through [`Caller`], and this value's
//! own connection is used only where it has been earned. The scoping is unchanged; what
//! changed is that it can no longer be silently wrong.
//!
//! `Clone` deliberately *shares* the connection (the transport clones one service across a
//! session's requests); `connection()` is the only thing that starts a new agent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http::request::Parts;
use rmcp::handler::server::common::{AsRequestContext, FromContextPart};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{JsonObject, ProtocolVersion};
use rmcp::service::Peer;
use rmcp::{tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler};
use serde_json::Value;
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

/// One tool as a model is offered it: what it is called, what it does, and the JSON its
/// arguments have to be.
///
/// Plain data on purpose — no rmcp type reaches the caller, so the in-process loop (AS-02)
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
/// event, so its agent is retracted by a `Drop` rather than by a clock. Matched to rmcp's own
/// `SessionConfig::keep_alive`, which reaps an abandoned HTTP session on the same terms and
/// for the same reason — there is no other signal — so the two do not disagree about how long
/// a quiet client stays listed. Long enough that an agent thinking between tool calls is
/// never dropped mid-investigation.
pub const STATELESS_IDLE: Duration = Duration::from_secs(300);

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
/// So the value's own agent is used only where it is *earned*, and the request says which:
///
/// - a call with no HTTP request behind it at all is stdio (AA-05) or the in-process chat
///   pane (AA-06), where the value's lifetime genuinely **is** the connection;
/// - a call rmcp served on its **session** lifecycle, where it is too;
/// - anything else is stateless, and falls back to the only durable thing such a client
///   sends.
///
/// **The branch is decided by rmcp's own predicate, not by `Mcp-Session-Id`.** That header
/// looks like the discriminator and is not one: `use_session = legacy_session_mode &&
/// is_legacy_request(…)` (`tower.rs`), and `is_legacy_request` reads the request's `_meta` and
/// protocol version and never consults it. A client that still echoes a stale session id
/// while sending per-request `_meta` takes the stateless branch, so keying on the header would
/// call it `Owned` and hand it a fresh agent per request — the very bug this type exists to
/// remove, reintroduced for exactly the client most likely to hit it.
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
        // Read, never taken: `RequestMetaObject`'s own extractor swaps `_meta` out of the
        // context, and a tool that wanted it after this one would find it emptied.
        if context.extensions.get::<Parts>().is_none() {
            // No HTTP request behind this call at all — stdio, or in-process.
            return Ok(Caller::Owned);
        }
        // rmcp's `uses_legacy_lifecycle`, restated over the two inputs it reads: the discover
        // lifecycle is taken when `_meta` carries everything `2026-07-28` requires, or when
        // the negotiated version is that new. Mirroring the predicate rather than sniffing a
        // header is the whole point — see the type's note.
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
/// RAII for [`SnapshotPin`](strata_core::engine::SnapshotPin)'s reason: the thing being
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
        // `lock()` rather than `unwrap()` on the guard: this runs during a drop, which may
        // itself be an unwind, and a panic there aborts the process.
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

// A manual `Clone`: the derive would demand `H: Clone`, and the whole point of the `Arc` is
// that the host is shared, never copied. A clone is the **same** agent — see the module
// note; `connection()` is what starts a new one.
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
            roster: Arc::new(Roster::default()),
        }
    }

    /// Which agent this value **is** — the id its [`Connection`] minted.
    ///
    /// The app's own answer to "is this the in-app assistant?", and the reason that question
    /// has an honest answer at all: the chat pane's `StrataTools` is one the app constructed,
    /// so it holds an id nothing else can claim. **The Agents pane will use it to leave the
    /// assistant out** — that pane says which external clients are connected to the project
    /// right now, and the assistant is not connected to anything, it is part of the app (its
    /// runs show as step cards in the transcript instead). Future tense on purpose: the filter
    /// is AS-04's, with the pane it belongs to, and this accessor is the seam it will read.
    ///
    /// Deliberately **not** a name comparison against [`AgentIdentity::assistant`]. An
    /// identity is a claim a client makes at `initialize`, so keying a "hide this from the
    /// pane" rule on one would let any MCP client make itself invisible by calling itself
    /// `strata-assistant` — the worst possible version of this rule. The identity stays what
    /// it is for: attribution.
    pub fn agent_id(&self) -> AgentId {
        self.connection.agent
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
            }),
        }
    }

    /// Which [`AgentId`] this call is made under — the one place [`Caller`] is resolved — and
    /// the guard that keeps it alive for the call's duration.
    ///
    /// A stateless caller's id is minted on first sight and kept, so the whole point of the
    /// enum holds: the *same* client asking twice is the same agent. The [`Busy`] guard is
    /// what stops [`retire_idle`](Self::retire_idle) reaping it out from under a call still
    /// running — hold it for the body, and dropping it re-stamps the entry so the idle window
    /// is measured from when the call *finished* rather than when it started.
    ///
    /// **A blank stateless identity is refused the session-scoped tools.** `clientInfo` is
    /// optional on the discover lifecycle, so pooling every un-introduced client under one
    /// minted id would put two different processes behind one [`AgentId`] — and that id is
    /// the whole of both isolation checks (`Agents::holds` and this value's run-cache key).
    /// One would list, page and close the other's query sessions: the AA-03 hole, reopened by
    /// a bucket meant for display. There is nothing to split them on, so the honest answer is
    /// to say so and keep the project-scoped tools working. (Not "the read-only tools" —
    /// every tool here is read-only, and two of the refused five carry `read_only_hint`. The
    /// line is whether a tool has to know *whose* agent is asking: `list_query_sessions` and
    /// `read_page` mean nothing without an identity, which is exactly what is missing.)
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
        // **Held for the call, not just stamped at its start.** A stamp alone only protects the
        // gap *between* calls: a `validate` or `list_functions` slower than the sweep interval
        // would still look idle, because nothing re-stamps while it runs and `busy` stays zero
        // for a tool that never resolves an `AgentId`. The sweeper would then retract the agent
        // mid-request and tear its query sessions down under a connection that is still there.
        //
        // Deliberately does **not** mint. A client that has never opened a query session has no
        // roster entry, and giving it one for a catalog read would put a row in the pane for an
        // agent doing nothing an agent needs an identity for.
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
    /// **A poll, because nothing on our side can observe the fact** (AGENTS.md §2): a client
    /// on the discover lifecycle has no connection, so its departure is not an event anywhere
    /// — there is no socket close, no `DELETE`, and no value whose drop means anything. The
    /// staleness is therefore bounded and stated rather than hidden: such an agent stays in
    /// the pane for at most `ttl` after its last call, and never longer.
    ///
    /// Driven by whichever transport can afford a timer — the HTTP server's own runtime
    /// (`crate::server`). Stopping is [`retire_all`](Self::retire_all)'s job, **not** this one
    /// with a zero `ttl`: `idle` requires `busy == 0`, so a zero `ttl` would skip precisely the
    /// agents with work in flight. There is nothing to drive for stdio or in-process, which is
    /// why it is called from there rather than started here.
    pub fn retire_idle(&self, ttl: Duration) {
        let now = Instant::now();
        // Never a working agent, however long the call has taken: see [`Live::busy`].
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
        // Outside the lock: `agent_gone` reaches every window, and a host is not something to
        // call with one of our mutexes held.
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
        // The wording is `AgentError::no_such_query_session`'s, but the handle never parsed,
        // so there is no id to hand it — the text the caller sent is what has to be echoed.
        .map_err(|_| AgentError::NotFound(format!("No open query session '{text}'.")))
}

/// **The vocabulary itself** — the ten tools as plain methods, with no rmcp type in any
/// signature.
///
/// Everything a tool *does* is here; the `#[tool_router]` block below is wrappers. A wrapper
/// resolves [`Caller`] to an [`AgentId`] and a [`Busy`] guard, then delegates — so an MCP
/// client and the in-process chat loop (AS-02) reach the same body, policy gate, run cache,
/// scoping key and messages included, with nothing copied between them. The pattern is
/// `open_query_session`'s, generalized: it was the one tool that already had the split,
/// because it is the one tool with something to read off the peer.
///
/// **The in-process caller is the owned case.** It holds this value's own [`Connection`], so
/// its [`AgentId`] lives exactly as long as its mount and retracts by RAII — precisely
/// [`Caller::Owned`]'s semantics. There is no roster entry to hold and nothing for the idle
/// sweep to reap, which is why these bodies take no [`Busy`] guard at all, exactly as
/// `Caller::Owned` short-circuits [`agent`](Self::agent) to [`Busy::none`].
///
/// Answers are the wire types the tools answer with, unchanged: a facade that unwrapped them
/// into tidier in-process shapes would be a second vocabulary, and the loop has to serialize
/// them back for the model anyway.
impl<H: Host> StrataTools<H> {
    /// **The vocabulary as data** — what a model is handed so it can ask for these tools by
    /// name.
    ///
    /// Derived from `tool_router()`, the router that serves MCP, never a second list: a
    /// tool added to the block below appears here with no further edit, carrying the same
    /// name, the same doc comment and the same schemars-generated argument schema an MCP
    /// client reads out of `tools/list`.
    ///
    /// **Ordered by name here rather than trusted from the router**, and that is a promise
    /// this list has to keep itself. `ToolRouter` is backed by a `HashMap`, whose iteration
    /// order is randomized per process; `list_all` happens to sort on the way out
    /// (rmcp 3.0.1, `handler/server/router/tool.rs`), but a manifest that inherited its order
    /// from a dependency's listing would reorder on the day that changed — and a *model-facing*
    /// list that reorders is not a cosmetic problem: the tool block sits at the head of every
    /// request, so shuffling it invalidates the provider's prompt cache on every turn and
    /// silently doubles what a conversation costs. Cheap to guarantee, expensive to discover.
    ///
    /// A method for the caller's sake — the answer is the same for every value of this type,
    /// since the router *is* the vocabulary and not this value's state.
    pub fn manifest(&self) -> Vec<ToolSpec> {
        let mut tools: Vec<ToolSpec> = Self::tool_router()
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

    pub async fn list_tables(&self, params: ProjectParams) -> Result<TablesResult, AgentError> {
        let project = self.project(params.project.as_deref()).await?;
        let entries = self.host.catalog(&project.root).await?;
        Ok(TablesResult {
            entries: entries.into_iter().map(EntryWire::from).collect(),
        })
    }

    pub async fn describe_table(
        &self,
        params: DescribeTableParams,
    ) -> Result<DescribeResult, AgentError> {
        let project = self.project(params.project.as_deref()).await?;
        let described = self.host.describe(&project.root, &params.name).await?;
        Ok(DescribeResult::from(described))
    }

    pub async fn list_functions(
        &self,
        params: ProjectParams,
    ) -> Result<FunctionsResult, AgentError> {
        let (_, engine) = self.engine(params.project.as_deref()).await?;
        Ok(FunctionsResult::from(engine.functions()))
    }

    pub async fn validate(&self, params: ValidateParams) -> Result<ValidateResult, AgentError> {
        let (_, engine) = self.engine(params.project.as_deref()).await?;
        let diagnostics = engine.validate(params.sql).await;
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

    pub async fn run(&self, params: RunParams) -> Result<RunResult, AgentError> {
        self.run_as(self.connection.agent, params).await
    }

    async fn run_as(&self, agent: AgentId, params: RunParams) -> Result<RunResult, AgentError> {
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
            .run(&project.root, agent, session, params.sql, mode, page_size)
            .await?;
        // **After** the dispatch, never before: a run refused at the ownership gate (or lost
        // to a window that went) never retired anything, so forgetting first would throw away
        // the page of a result that is still there to read. An explain materializes nothing
        // and leaves the previous result alone either way.
        if mode == RunMode::Run {
            self.forget(agent, &project.root, session);
        }
        let handle = params.query_session;
        match settled {
            Ok(Settled::Rows(output)) => {
                // Converted once and shared: the response and every later page describe the
                // same schema, so nothing re-walks it.
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
            // The one place stopped-vs-failed is judged.
            Err(e) if stopped_on_purpose(&e) => Ok(RunResult::Stopped {
                query_session: handle,
                reason: e,
            }),
            Err(e) => Err(AgentError::Query(e)),
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
            self.forget(agent, &project.root, session);
            return Err(AgentError::ResultMoved);
        }
        let page = params.page.max(1);

        let Some(snapshot) = last.snapshot else {
            // A run that produced no rows materialized nothing. Reporting an empty page is
            // the truth; a "not found" would read as a lost result.
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
        match engine
            .fetch_page(snapshot, page, last.page_size, sort)
            .await
        {
            Ok((rows, _)) => Ok(PageResult {
                query_session: params.query_session,
                columns: last.columns,
                rows: cells(&rows),
                total: last.total,
                page,
                page_size: last.page_size,
            }),
            // Ask the engine, never its prose: a snapshot that is gone is a replaced result,
            // anything else is a real read failure. Asked *after* the read, so the answer
            // cannot race the dispatch that retired it.
            Err(e) => {
                if engine.snapshot_live(snapshot) {
                    Err(AgentError::Query(e))
                } else {
                    self.forget(agent, &project.root, session);
                    Err(AgentError::ResultMoved)
                }
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
// `doc_markdown` is off for this block alone, and the paragraph above is the reason: these doc
// comments are not documentation, they are the tool `description` rmcp advertises and `manifest`
// hands to a model. A backtick clippy adds here is markup in a wire string, and it contradicts
// both the register they are written in and AGENTS.md §3's "no backticks in user-facing text".
// Everywhere else in this file `read_page` and friends are backticked, correctly, because there
// they really are prose about code.
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
    #[tool(name = "list_tables", annotations(read_only_hint = true))]
    async fn list_tables_tool(
        &self,
        caller: Caller,
        Parameters(params): Parameters<ProjectParams>,
    ) -> Result<Json<TablesResult>, AgentError> {
        let _busy = self.touch(&caller);
        Ok(Json(self.list_tables(params).await?))
    }

    /// Describe one table or view: its columns and types, nested fields, Hive partition
    /// columns, source paths and format, plus the row count and column statistics the source
    /// reports for free. Only facts that were read — nothing is scanned or estimated.
    #[tool(name = "describe_table", annotations(read_only_hint = true))]
    async fn describe_table_tool(
        &self,
        caller: Caller,
        Parameters(params): Parameters<DescribeTableParams>,
    ) -> Result<Json<DescribeResult>, AgentError> {
        let _busy = self.touch(&caller);
        Ok(Json(self.describe_table(params).await?))
    }

    /// List the SQL functions this project's engine has registered: names, overload
    /// signatures and documentation. What is registered is what exists.
    #[tool(name = "list_functions", annotations(read_only_hint = true))]
    async fn list_functions_tool(
        &self,
        caller: Caller,
        Parameters(params): Parameters<ProjectParams>,
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
    /// see what you run and promote any query you ran into their own editor.
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
        // Held for the whole call, dispatch included: the sweeper must not retire this agent
        // while its query is on the engine — see `Live::busy`.
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
    name = "strata",
    instructions = "Strata is a local parquet/CSV/JSON query workspace over Apache DataFusion. \
Read-only: SELECT, EXPLAIN, SHOW and DESCRIBE run; everything else is refused. \
Start with list_tables and describe_table to learn the catalog, validate to check SQL \
cheaply, then open_query_session and run. Your work lives in query sessions of your own, \
which the user can watch and promote into their editor wherever Strata's window is open — so \
it never disturbs the tabs they are working in. Open a session per line of investigation; each run \
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

    // --- projects ---------------------------------------------------------

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
        let Err(e) = tools.list_tables(no_project()).await else {
            panic!("expected an ambiguous-project error");
        };
        let text = e.to_string();
        assert!(text.contains("sales (/w/sales)"), "{text}");
        assert!(text.contains("ops (/w/ops)"), "{text}");

        // Naming one resolves it.
        let named = tools
            .list_tables(ProjectParams {
                project: Some("ops".into()),
            })
            .await
            .unwrap();
        assert!(named.entries.is_empty());
    }

    // --- catalog ----------------------------------------------------------

    /// The catalog as the store shows it: a def the engine refused is a row with its error,
    /// not a missing row.
    #[tokio::test]
    async fn list_tables_reports_a_failed_def_with_its_error() {
        let (_root, tools) = one_project("list_tables").await;
        let entries = tools.list_tables(no_project()).await.unwrap().entries;
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
                project: None,
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
                project: None,
            })
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
        let functions = tools.list_functions(no_project()).await.unwrap();
        assert!(functions.scalar.iter().any(|f| f.name == "json_get"));
        assert!(functions.aggregate.iter().any(|f| f.name == "count"));
        assert!(!functions.window.is_empty());
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

    // --- query sessions ---------------------------------------------------

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

        // The handle is now stale, and the answer is the plain statement
        // `list_query_sessions` recovers from — the same one a handle that never existed gets.
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

        // Everything is stale by any measure, but the call has not finished.
        tools.retire_idle(Duration::ZERO);
        assert_eq!(
            tools.host.query_sessions(&root, agent).await.unwrap().len(),
            1,
            "a working agent survives the sweep however long it has taken"
        );

        // The guard's drop re-stamps, so the window is measured from when the call finished —
        // an immediate sweep at the real TTL must still spare it.
        drop(busy);
        tools.retire_idle(STATELESS_IDLE);
        assert_eq!(
            tools.host.query_sessions(&root, agent).await.unwrap().len(),
            1,
            "and the idle window restarts when the call ends"
        );

        // Only once it is genuinely idle.
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

        // The guard is deliberately still held — this is the shutdown-mid-request case.
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

    // --- run --------------------------------------------------------------

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
            .run(run_params(&session, "SELECT id FROM people"))
            .await
            .unwrap()
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

        // The first session's result is gone, and reads exactly like a session that never
        // ran; the newest is still there.
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

        // The other agent fills its own quota and then some.
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
                .register(TableSpec {
                    name: "people".into(),
                    paths: vec![root.join("people.csv").display().to_string()],
                    format: SourceFormat::from_name("csv"),
                    partitions: Vec::new(),
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
        assert_eq!(e.to_string(), Blocked::CreateTable.editor_message());
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

        // Nothing was materialized, so there is nothing to page.
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
        let tools = StrataTools::new(MockHost::new(vec![
            MockProject::new("sales", &root).settling(CANCELLED)
        ]));
        let session = open(&tools).await;
        let result = tools
            .run(run_params(&session, "SELECT 1"))
            .await
            .expect("a stop is not an error");
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
            tools.run(run_params(&stray, "SELECT 1")).await,
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
        // Page size follows the run that settled it, so paging is consistent.
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

    // --- the manifest -----------------------------------------------------

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
        let mut router = StrataTools::<MockHost>::tool_router().list_all();
        let manifest = tools.manifest();

        // Paired by name rather than by position, because `manifest` sorts for itself and
        // the router's listing order is the router's business — the point of this test is
        // that every tool crosses over intact, not that two iterators happen to agree.
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
