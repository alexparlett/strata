# Agent access — MCP host, agent query sessions, chat pane (AA)

Spec for **agent-driven access** to a Strata project's data: an AI agent (Claude Code today, an
in-app assistant later) lists the catalog, inspects schemas, and runs read-only SQL — with every
query it executes a **real run** on the project's own engine, on the ordinary
press → snapshot machinery, shown in the window's **Agents pane** and promotable into a tab with
one press. Design settled 2026-07-30 with Alex; workstream:
`.claude/tasks/workstream-agent-access/`.

The one-sentence architecture: **one read-only tool vocabulary over one UI bridge, with thin
swappable frontends** — an MCP server first (any MCP client is the chat surface), a native chat
pane as the flagship follow-on, a headless CLI host for app-closed use. The frontends are
sequencing, not architecture: a chat pane's LLM loop needs the same Tokio runtime and the same
UI seam the MCP server does, so nothing built for the first frontend is discarded by the second.

---

## 1. Direction (decided)


- **Agent queries are ordinary runs** — same engine, same materialization, same
  supersede/retire, same snapshot the user can page, sort, export or take over. That half is
  settled and is what makes this better than a generic read-only-SQL MCP server (§2).
- **~~The investigation trail *is* the tab strip.~~ Reversed for MCP by AA-03b.** Landing agent
  runs in the user's own `QueryTab`s was built, used, and found wrong for a frontend that is not
  *in* the window: a Claude Code session in a terminal opened tabs the user was not watching,
  moved the editor out from under them, and cost a diagnostics pass per tab on the very engine
  the user's own press needed. An agent that is not in the window gets **its own surface** — the
  Agents pane in the sidebar — and a press promotes its SQL into a **new** tab through
  `actions::open_sql`. A new tab rather than the active one is the load-bearing half: the
  History drawer loads into the tab you are in because being there *is* the ask, while an
  agent's run arrives in a surface you were only looking at. The chat pane (§9) is the opposite case and
  keeps the tab gesture, because it is in the window and the user is looking at it. The general
  rule: **a surface's state belongs to whoever is looking at that surface**, and "shared,
  last-writer-wins" is a fine rule for *content* and a bad one for *attention*.
- **Agent-managed handles**, not one-per-query and not one reused: the agent opens them, runs in
  them and closes them, so scratch iterates in one and findings get parked each in their own.
  Those handles are **query sessions** (`open_query_session` / `run(query_session, sql)` /
  `close_query_session` — §5), deliberately not "sessions" — MCP's own `Mcp-Session-Id` is an
  agent's *connection*, and ours (`SessionState` / `session.json`) is the window's tabs — and
  not "tabs", which they are not. A query session maps onto the engine's `WsId`, which is what
  keeps its runs real while keeping them out of the tab strip.
- **The user's tabs are the user's.** An agent never opens, focuses or closes one, and anything
  of the agent's the user wants they promote — an ordinary press, with its own nonce and its own
  cache entry. This is structural rather than a rule anybody has to keep: every session-scoped
  tool is scoped to the calling agent's own `AgentId`, so an agent is never handed a handle on
  another agent's work, let alone on a tab.
- **A query session lives as long as its connection.** A client that disconnects takes its
  sessions with it — RAII on the per-connection service value, which is the only signal a
  transport gives — and their engine workspaces are retired with them.
- **Read-only v1**, the editor's managed-DDL policy exactly: queries / `EXPLAIN` / `SHOW` /
  `DESCRIBE` pass; everything else is refused with the same message the editor shows. No table
  registration, no view creation, no export, and **no profiling** (§6). The vocabulary is shaped
  so curated writes could arrive later as new tools, never by loosening `run`.
- **MCP first, chat pane flagship later, headless third.** MCP ships the working agent
  experience for the cost of a thin [rmcp](https://github.com/modelcontextprotocol/rust-sdk)
  shim — Claude Code brings the brain, the context management and the conversation UI. The chat
  pane (§9) reuses the vocabulary in-process; its "brain" choice is deferred to its own task.
- **One app-wide server**, localhost + bearer token, stable port. Multi-project is kept
  deliberately small: tools default to the single open project and error with the open-project
  list when there is more than one (`project` param disambiguates; a query-session handle is
  unambiguous from `open_query_session` on).
- **SQL-syntax extension rejected** for investigation — an investigation is multi-step by
  nature, not a statement. LLM-as-UDF (an `llm_extract(col, …)` scalar) is a different feature
  and out of scope here.

## 2. Prior art (what shaped the shape)

- **Snowflake Copilot** — chat panel docked right of the worksheet; SQL flows *to* the
  worksheet to run. Conversation surface and execution surface are separate.
- **DataGrip AI Assistant** — chat tool-window; `@dbObject` attaches specific schema objects as
  context; generated SQL moves to the editor console via attach-to-editor actions.
- **Databricks Assistant agent mode** — multi-step agentic work whose steps land in the
  notebook/SQL editor, visibly.
- **JetBrains MCP server** — the desktop-app-hosts-MCP pattern: an HTTP server embedded in the
  running IDE, a thin stdio proxy for clients that need it, port-based pairing.
- **DuckDB/Postgres/SQLite MCP servers** — the settled read-only tool vocabulary: list tables,
  describe, query with row caps.

The synthesis all of them agree on: **chat is the conversation surface; the editor/worksheet is
the execution surface.** Strata's differentiator is that its execution surface is snapshot-backed
— every agent step is an immutable result the user can inspect later, which none of the above
retain.

## 3. Architecture overview

```
┌────────────────────────────── Strata app process ──────────────────────────────┐
│                                                                                │
│  strata-agent (own small Tokio runtime — the Engine pattern)                   │
│  ┌──────────────────────────────────────────────┐                              │
│  │ rmcp streamable-HTTP server  127.0.0.1:port  │   later: chat-pane LLM loop  │
│  │ tool router → vocabulary → Host trait        │   (same runtime, same Host)  │
│  └───────┬──────────────────────────┬───────────┘                              │
│          │ control plane            │ data plane                               │
│          │ AgentAsk / AgentNotice   │ Arc<Engine> direct — reads AND an        │
│          │ over tokio mpsc          │ agent's own runs, on its query           │
│  ┌───────▼───────────┐              │ session's WsId                           │
│  │ per-window bridge │              │                                          │
│  │ use_agent_bridge  │              │                                          │
│  │ one serial driver │              │                                          │
│  └───────┬───────────┘              │                                          │
│          │ satellite writes         │                                          │
│  ┌───────▼───────────┐   ┌──────────▼──────────────────────────────┐           │
│  │ state::agents     │   │ Engine (private Tokio rt) → snapshot     │           │
│  │ → Agents pane     │   │ — the same one the user's press reaches  │           │
│  └───────────────────┘   └──────────────────────────────────────────┘          │
│     the user's tabs are not in this picture at all, which is the point         │
└────────────────────────────────────────────────────────────────────────────────┘
   headless: `strata mcp <project>` = same vocabulary + Host over a plain Engine,
   stdio transport, no bridge (no UI to bridge to)
```

Crate layout:

- **`strata-agent`** (new workspace member) — the tool vocabulary (schemas + semantics + error
  mapping), the `Host` trait, the rmcp server, and the headless host. Depends on `strata-core` +
  `strata-model` + `rmcp`; **no Freya dependency**, which is what keeps the vocabulary reusable
  by the chat pane and testable against a mock host.
- **`strata-freya`** — the in-app `Host` impl: the service directory (which also dispatches an
  agent's runs), the per-window bridge (one serial driver), the agents satellite and the Agents
  pane, server lifecycle, Settings pane.
- **`strata-core`** — two exported seams (AA-01): the managed-DDL policy verdict, and the
  project registration pass the headless host replays.

**Why Streamable HTTP and not a Unix socket:** the transport menu is the client's, not ours. MCP
clients speak two spec transports — stdio, where the *client spawns* the server process, and
Streamable HTTP. Stdio is structurally impossible for the in-app host (the server lives inside
an already-running GUI app; nothing spawns it), and no client dials a Unix socket — so a UDS
server, for all its nicer access model, would force a stdio↔socket proxy into every connection:
more machinery than the HTTP it avoids. Hence the JetBrains shape: embedded localhost HTTP,
compensated with the loopback bind + bearer token (§6). rmcp ships both spec transports natively;
the headless host (§10) uses stdio because there the client *does* own the process.

The `Host` trait is the one deliberate abstraction: `Host` answers project-scoped questions
(catalog, query sessions, runs) and hands back engine handles for data-plane reads. In-app it is the
service directory + bridge; headless it is a plain `Engine` + the loaded defs. Two impls that
really exist — not speculative generality.

## 4. The bridge (the Tokio ↔ Freya seam)

Verified end-to-end against the fork (checkout `bf651044`) and the app; every claim below names
its source.

**Why a bridge at all:** the server lives on a Tokio runtime (rmcp and any HTTP/LLM client need
a reactor); tabs, the catalog store and presses are UI-thread Radio state. The engine facade
already proves the *outbound* direction (UI awaits Tokio `JoinHandle`s — executor-agnostic);
agent access needs the *inbound* one.

**The four verified hops:**

1. **Foreign thread → UI wake.** Freya's UI futures are polled on the winit loop and woken by
   `ArcWake for FuturesWaker(EventLoopProxy)` → `send_event(PollFutures)`
   (`crates/freya/crates/freya-winit/src/lib.rs`). `EventLoopProxy` is `Send`, so a waker
   invoked from the server thread wakes the UI correctly.
2. **Carrier.** `tokio::sync` mpsc/oneshot are runtime-agnostic (strata-core already ships
   tokio "full"). A send from the server thread invokes the receiver-future's waker → hop 1.
3. **UI-side driver.** `spawn` (freya prelude) runs a UI future from a hook; the diagnostics
   driver (`state/diagnostics.rs`) is the existing in-repo shape — one spawned serial loop
   awaiting async work and writing Radio state.
4. **Awaiting the engine from the server's own runtime.** The engine facade is a direct-call
   async facade whose futures are executor-agnostic, so the server task awaits a run the same
   way the UI does. (AA-03 could not: its runs rode a tab's `QuerySpec`, so the settle had to be
   observed through freya-query by an *agent keeper* — one invisible subscriber per parked
   reply, the `RequestKeepers` pattern on a second question. AA-03b deleted all of it, because a
   run no longer rides a tab.)

**Control plane** — everything that touches window state travels as `AgentAsk` on a bounded
`tokio::sync::mpsc` channel per project window, each variant carrying its own reply channel.
Beside it runs an **unbounded** `AgentNotice` channel for the facts that carry no answer, and
the split is load-bearing rather than tidy: a `send().await` is right for a tool call, but the
most important notice of all is sent from a `Drop` — an MCP connection ending, which has nothing
to await on and nowhere to report a failure to. `use_agent_bridge` (mounted by `ProjectRoot`)
spawns one serial loop draining both, **asks first**, so a settle can never overtake the
dispatch that minted its sequence number.

**A run is dispatched by the caller and bracketed by the window** (AA-03b). The dispatch goes
straight to `Arc<Engine>` against the query session's own `WsId`, on the server's runtime. What
the window still owns is the half only it can answer, and it travels either side:
`AgentAsk::RunStarting` first — the ownership check (does this agent hold this session?) plus
the record of what is running, replying with the run's sequence number — then
`AgentNotice::RunSettled` after, naming that same sequence number so a slow query's outcome
cannot land on a faster one the agent pressed after it.

That ordering is what makes AA-03's three costs vanish by construction rather than by
mitigation: no tab is opened, so nothing steals focus and nothing is left to close, and the
window's diagnostics driver has nothing extra to validate on the engine the user's own press is
waiting for.

**Data plane** — reads that are engine-scoped and side-effect free skip the UI entirely: the
server holds the window's `Arc<Engine>` (from registration, below) and calls
`fetch_page` (snapshot-scoped, cacheable), `validate` (dry-plan, total by design) and
`functions()` directly from its own runtime. Bulk rows never queue behind UI work.

**Service directory** — each project window registers `(root, name, Arc<Engine> capture,
ask-sender, notice-sender)` with the server on mount and deregisters via `use_drop`. A cross-thread
`Mutex<Vec<…>>` service registry, the `platform::windows` shape — *not* reactive UI state. (The
no-registry rule in AGENTS.md §4 governs UI consumers threading reactive data; this is a DI seam
between threads, the thing context is for in-tree and a directory is for across them.)

**Lifecycle honesty, for free or nearly:**

- An agent's **own** newer run in a session supersedes its older one; the awaited settle gets
  the engine's stopped-on-purpose string and the tool reports it as an outcome (§7) — never a
  fault. A *user's* press can no longer supersede an agent's, and vice versa: they are different
  workspaces, which is also why an agent's run can no longer gate closing a user's tab.
- A **window close or re-root** drops the bridge and the registration (`use_drop`); the reply
  channels drop, the server task sees them close and answers "project window closed". Because
  re-root *is* the remount path (AGENTS.md §2), no second cleanup path exists to drift.
- Agent runs **count toward the T2 close confirm** (`Engine::watch_inflight` — the engine is the
  only thing that knows, and the predicate stays the engine's answer, never one derived from
  mounted UI). What changes is the *sentence*: "Queries are running" shown to somebody who
  pressed Run on nothing sends them looking for a query they never started, so the dialog asks
  the engine about the tab `WsId`s and the query-session `WsId`s separately and says "An agent
  is running a query" when only the latter answers. Not confirming at all for agent-only work
  was considered and rejected: it costs the one property that makes the confirm trustworthy —
  that the app never destroys work in flight without saying so.
- Agent runs appear in **neither** history nor `session.json`, and that is the decision rather
  than an omission: history is capped and deduped before the cap, so exploratory agent queries
  would evict runs the user actually made. History records what *the user* ran — and a promoted
  agent query, run by a press, enters it the ordinary way. The bridge driver logs the agent's
  *actions* it alone observes ("… opened a query session", "An agent disconnected") — the
  observer-records rule.

## 5. Tool vocabulary (v1)

All tools are project-scoped unless noted; `project` is optional everywhere it appears and only
required when more than one project window is open (the error lists them). Identifiers follow
MCP conventions; responses are structured JSON.

| Tool | Plane | Semantics |
|---|---|---|
| `list_projects` | server | The open project windows: name + root. |
| `list_tables` | control | The **catalog as the store shows it** (see below): tables, views, saved queries — name, kind, source, and registration state (`ready` / `failed` + the failure message). |
| `describe_table(name)` | control | Schema (column name, type, nullable), row count when known, partition columns, source + format — **only real facts** (P3-08): what registration read, never derivations. |
| `list_functions` | data | The engine's live `FunctionCatalog` — names, signatures, docs. What's registered is what exists (no second list). |
| `validate(sql)` | data | `Engine::validate` — lexical lints + policy verdicts + dry-plan diagnostics, never executes. The cheap way for an agent to check SQL before burning a run. |
| `open_query_session()` | control | Mints a query session for the calling agent; returns its handle (a `QuerySessionId`, which *is* the engine `WsId` it runs against). |
| `list_query_sessions()` | control | **The caller's own** sessions: handle, and whether a run is settled / in flight / never happened. |
| `run(query_session, sql, mode?, page_size?)` | control+data | The policy gate (§6), then a dispatch straight to the engine against that session's `WsId` — a real execution, with the window bracketing it (ownership check + record before, outcome after). Returns columns, page-1 rows, exact total, elapsed. `mode` = `run` (default) \| `explain` (returns the plan tree, materializes nothing). `page_size` bounded (default: the app's default row limit setting; capped at `MAX_PAGE_SIZE` = 10000, and the response reports the size actually used). |
| `read_page(query_session, page, sort?)` | data | Pages that session's last settled snapshot via `Engine::fetch_page` — snapshot-scoped, side-effect free, at the page size that `run` settled with. A snapshot retired by a newer run in the session fails cleanly: "result was replaced; re-run" (§7). |
| `close_query_session(query_session)` | control | Drops the session and tears its engine workspace down (a running query in it is cancelled). Tidy rather than required — every session goes when the connection does. |

Notes that are rules, not details:

- **`list_tables` answers from the store, never DataFusion introspection.** Introspection would
  surface the `__snap_*` result snapshots and hide defs whose registration failed — precisely
  the rows the catalog exists to show (the P3-02 correction, applied here on day one). In-app
  the bridge projects `ProjectState`; headless the host answers from `load_defs` + the
  registration outcomes it itself produced.
- **Every session-scoped tool is scoped to the calling agent.** `StrataTools` *is* one agent —
  the transport asks for one value per client connection, which mints an `AgentId` and retracts
  it on drop — so "only your own sessions" is a property of the type rather than a check
  somebody has to remember. That is the AA-03 hole closed structurally: `list_tabs` used to hand
  an agent every open tab, the user's included.
- **Only `open_query_session` needs the client's `clientInfo`**, because opening is when a host
  first has anything of this agent's to show. Everything after it is addressed by `AgentId`
  alone, which is also what lets the whole vocabulary be driven with no MCP peer at all — the
  property the chat pane (§9) needs.
- **`run` never rewrites SQL.** No injected `LIMIT`: the run materializes exactly what the user's
  own press would (same cost, same snapshot), and the *response* is bounded by `page_size` +
  paging. Totals are always exact (the snapshot knows).
- **`run` reuses its handle the way a press reuses a tab**: dispatch supersedes whatever that
  handle had in flight and retires its previous snapshot. That is the point — agent semantics
  *are* app semantics. (Today the handle *is* a tab; after AA-03b it is a query session on a
  surface of the agent's own. The supersede rule is unchanged either way.)
- **After AA-03b, nothing an agent does reaches disk** — `session.json` cannot hold it (an agent
  will own no tabs), and `.strata/history.jsonl` deliberately will not: history is capped and
  deduped *before* the cap, so exploratory agent queries would evict the runs the user actually
  made. The Agents pane is the agent's record and is ephemeral, like the event log; history
  stays the user's. A query the user **promotes** and runs enters history the ordinary way,
  which is the honest rule — history records what the user ran. **Today this is not yet true**:
  an agent run rides a real tab, so it persists in `session.json` and its tab's request keeper
  records it in history. That is one of the four costs AA-03b exists to remove (§1).
- **`explain` goes over the wire as text.** `QueryPlan`'s structured `PlanNode` list exists to
  be *drawn* (it carries accent colours and time-share bars); off-screen it would be the same
  tree twice, one copy in a shape nothing can use. `run(mode: "explain")` answers with
  `logical` / `physical` — what `EXPLAIN` prints — plus `analyze`. The host wraps the
  statement with `plan::as_explain`, exactly as the app's own Run capability does, so
  `mode: "explain"` means "plan this", not "I already typed EXPLAIN".
- **No MCP resources in v1** — tools only. Resources (schema-as-resource) are an open question
  (§11); every current client consumes tools, and one surface is one thing to keep honest.

## 6. Policy & safety

- **The DDL gate is the editor's, through one funnel.** The managed-DDL policy currently lives
  as a private classification inside `strata-core`'s validation
  (`engine/sql/validate.rs::policy_block`) — **`Engine::query` itself does not enforce it**, the
  editor simply never dispatches what validation flagged. The agent path cannot rely on caller
  discipline: AA-01 exports the verdict from `strata-core`, and the tool layer refuses any
  flagged statement **before dispatch**, with the same message text the editor shows ("CREATE
  TABLE is not supported in the editor. Register tables in Table Config" and kin). One
  predicate, two consumers, zero copies.
- **Profiling is not exposed.** A profile is the most expensive thing the app does, gated
  behind a per-entry cost confirm with one entry point (`ProfileActions::ask`, P3-10). A tool
  call that blocks on a user dialog is a bad tool, and an unrestricted scan bypasses the app's
  own gate — so v1 exposes neither. `describe_table` reports the free tier only (footer stats,
  registration row counts). Whether settled profile numbers should surface later is an open
  question (§11).
- **Transport security:** bind `127.0.0.1` only; bearer token required on every request. The
  token is minted once, persisted in app config, shown (and regenerable) in Settings — persisted
  so MCP clients don't need reconfiguring every app launch. No pairing file in `.strata/`
  (ephemeral state in a shareable directory).
- **Off by default.** Agent access is an opt-in setting; the server does not listen until it is
  enabled.
- **Reads are still work.** A `run` is a real query with real cost; the close confirm, cancel,
  and supersede all apply to it exactly as to a human press. Nothing agent-initiated is
  invisible: the event log records every agent action, history records every successful run.

Client configuration (the whole of it):

```bash
claude mcp add --transport http strata http://127.0.0.1:<port>/mcp --header "Authorization: Bearer <token>"
```

The **README's Agent access section** is the same thing for every other client — Claude Desktop
(which speaks stdio only, so it needs an `mcp-remote` proxy), VS Code, Cursor, Gemini CLI, Codex
CLI — plus what the header's status dot is telling you. It is the user-facing half and points at
Settings ▸ Agent access for the switch and the token; nothing user-facing documents the config
file, which is the app's to write.

## 7. Error taxonomy

Every error an agent can see is one of these, and stopped-on-purpose is never a fault
(AGENTS.md §2 — the engine's `stopped_on_purpose` predicate is the only thing that knows):

| Class | Trigger | Shape |
|---|---|---|
| Policy refusal | `run`/`validate` on blocked DDL/DML | The editor's own message, verbatim; names the owning surface. |
| Query error | The engine's `Err` from a real fault | The engine message, unedited (it already reads like an IDE's). |
| Stopped on purpose | User cancel / user or agent re-run superseding | A distinct non-fault outcome: "the run was cancelled in the app" / "replaced by a newer run". |
| Result moved | `read_page` on a retired snapshot | "The tab's result was replaced; re-run to read it." |
| Not found | Unknown query-session handle / table name | Plain statement; `list_query_sessions` / `list_tables` are the recovery. **A handle belonging to another agent gets this same answer**, deliberately: a distinct "that is not yours" would confirm the session exists, which is a fact an agent has no business learning and no way to act on. |
| Ambiguous project | >1 window open, no `project` (or a `project` name two windows share) | Lists the open projects. |
| No project | Nothing open to address | "No project is open." Added by AA-02: an "ambiguous" error listing nothing reads as a bug, and a project-scoped tool has to say something. |
| Window gone | Bridge dropped mid-ask (close / re-root) | "The project window closed." |
| Unauthorized | Bad/missing token | HTTP 401 before any tool runs. |

Everything above **but the last** is an MCP tool result with `isError: true`, not a JSON-RPC
protocol error: these are conditions an agent should read and recover from, and the listing
tools are the recovery. Protocol errors stay for what they are for — a malformed request.
`Unauthorized` is answered by the transport, in front of the router, so it never reaches a
tool at all.

Two things a project is resolved by, in this order: its **root** (the identity — a window is
keyed on its project folder, so a root names at most one) and then its **name**, which is
allowed to collide (`/a/data` and `/b/data` are both "data") and reports ambiguity rather
than guessing.

## 8. State ownership

What lives where — nothing here adds state to the session or project stores:

| State | Owner | Notes |
|---|---|---|
| Server socket, token check, tool router | `strata-agent` server | Own Tokio runtime; the Engine pattern. |
| Service directory (root, name, engine, ask + notice senders per window) | server, `Mutex` | Windows register on mount, deregister on `use_drop`. Two senders because one producer cannot wait: asks are bounded and awaited (honest backpressure for a tool call), notices are unbounded and one-way, because the most important of them is sent from a `Drop` — a connection ending, with nothing to await on. |
| Last settled snapshot per agent run (for `read_page`) | server | A cache of a fact the settle carried, keyed `(agent, project root, query session)` — the agent is *in* the key rather than checked against it, since a key is a check that cannot be forgotten. **Never a `SnapshotPin`** — a pin is right for an export window, which owes the user the rows it was opened on, and wrong for a long-lived server, which would keep dead results alive; a retired snapshot fails cleanly, so staleness is honest. "Retired vs a real read failure" is `Engine::snapshot_live` (AA-02, `strata-core`), asked *after* the read fails so it cannot race the dispatch — never a match on DataFusion's prose. Dropped at the next `run` in that session (an `explain` materializes nothing, so it leaves the entry) and on `close_query_session`. |
| Ask + notice channels and the driver loop | the window (`use_agent_bridge`) | One serial loop draining both, asks first so a settle can never overtake the dispatch it belongs to. Dies with the window/re-root — no cleanup path to drift. |
| The agent's run trail (what the Agents pane shows) | `state::agents`, a satellite | Ephemeral and capped (runs per session, sessions per agent), like `state::log` — **never** `SessionState`, so it cannot reach `session.json`, and **never** `history.jsonl`, which stays the user's. Recorded by its observer: the driver took the ask that opened the session and the notice that settled the run. |
| Tabs, requests, snapshots, history, events | **unchanged, and now untouched** | An agent's run is a real execution on its own `WsId`; it opens no tab, raises no diagnostics pass and writes nothing the user owns. |
| ~~`QueryTab::agent`~~, ~~`SessionState::open_background`~~, ~~`views::agent_keeper`~~ | — | AA-03's badge, its don't-steal-focus open and its parked-reply settle observers, all deleted by AA-03b: an agent opens no tabs, so there is nothing to badge, nothing to open in the background, and no press to observe through freya-query. |
| Whether a client is paired (the header dot) | rmcp's `LocalSessionManager`, **sampled** | The one polled thing in the app, because the fact is rmcp's and a session is created below our seam. `AgentServer::clients` is a non-blocking `try_read`; the dot mounts its timer only while the setting is on. Over-reports for at most `keep_alive` (5 min) after a client dies without its `DELETE`, and never under-reports. |
| `agent_access` settings (enabled, port, token) | app config (`Settings`) | Via `settings_merge!` — a new field that isn't merged is a build error. |

## 9. Chat pane (flagship follow-on — forward design)

The native conversation surface, built **after** the MCP host proves the core, reusing the
vocabulary in-process (no MCP hop — the chat loop calls the same tool layer the rmcp router
does, on the same `strata-agent` runtime, through the same bridge).

- **Placement:** a right-side pane in the project window (the Snowflake/DataGrip position),
  toggleable from the activity rail; conversation-first, streaming.
- **Context attachment:** `@`-mention catalog objects (tables, views, saved queries) to pin
  their schemas into context — the DataGrip `@dbObject` pattern, answered from the same
  `describe_table` path.
- **Execution surface: real runs, and the tab gesture is the chat pane's to keep.** The
  assistant's queries are real runs in query sessions of its own, exactly like MCP-driven ones
  (AA-03b). What differs is what the surface may then *do* with them: chat lives in the window
  and the user is looking at it, so "open this in a tab" is a wanted gesture here rather than an
  intrusion — the transcript shows compact step cards (SQL + row count + elapsed), with inline
  mini-results and an explicit open-in-tab for anything bigger, through the same
  `actions::open_sql` funnel the Agents pane uses. §1's distinction is exactly this: an agent
  that is *in* the window versus one that is not.
- **The brain is deliberately undecided** — native Anthropic API client (app-owned loop, API
  key in Settings) vs a Claude Agent SDK / CLI sidecar (reuses the user's subscription and
  Anthropic's context management, costs process management + an install dependency). That
  decision is the chat workstream's first task, and nothing in the core prejudges it: both
  brains drive the identical tool layer.

## 10. Headless host (`strata mcp <project>`) — built (AA-05)

For app-closed use (CI, scripts, a second machine): the same binary, a CLI branch in `main()`
**before** any GUI launches — beside the existing project-path handling, and before it, since
nothing app-global exists for a server with no window.

- `strata_agent::headless::HeadlessHost` builds a plain `Engine` and replays the **project
  registration pass** (load defs → register tables → create views), extracted to `strata-core`
  by AA-01. Its outcomes **are** the catalog: folded once at open into the same `CatalogEntry` /
  `Described` shapes the app projects from its store, so a registration failure is a `failed`
  `list_tables` row exactly as in-app. The pass completes before anything is served, which is
  why this host needs no equivalent of the app's scan claim.
- **stdio transport** (the standard for locally-spawned MCP servers) — no port, no token; the
  client owns the process. `tools::Caller` reads that as `Owned` (no HTTP request behind the
  call), so the service value's lifetime genuinely is the connection and there is no idle sweep
  to run.
- Touches **no shared state**: never reads or writes app config, `session.json`, or history.
  Snapshot directories are already safe side by side — each engine lock-claims its own dir
  (`claim_snapshot_dir`), so a headless engine beside a running app cannot collide with it. Two
  consequences of not reading app config, both stated rather than papered over: the engine runs
  DataFusion's defaults (a `--config` flag can arrive if wanted), and `default_page_size` is the
  **shipped** `Settings::default().row_limit` rather than the user's setting.
- It writes nothing to the project either: a folder with **no** project in it is refused with a
  message rather than scaffolded, which is where this deliberately parts from the GUI open path.
- Query-session handles still exist (a session *is* a `WsId` — headless they are workspaces with
  no UI at all), so agents see one vocabulary everywhere, AA-03c's close-vs-dispatch tombstone
  included.
- One project by construction, so the `project` argument resolves to it or to nothing and the
  host consults it nowhere.

## 11. Open questions

- **Settled-profile exposure.** Profile numbers live in the freya-query cache keyed by the scan
  request; surfacing them through `describe_table` means the bridge reading another surface's
  cache entry. Deferred until wanted — the free tier may be enough.
- ~~**Agent-tab marker.**~~ **Dissolved by AA-03b.** It was built (an `AGENT` badge on the editor
  toolbar, set by the bridge and retracted by the user's own press) and then removed with the
  premise underneath it: an MCP agent opens no tabs, so there is no tab to mark. The question was
  always a symptom — needing a badge to say "this tab isn't really yours" is the tell that the
  tab should not have been the user's in the first place.
- ~~**A canvas for the Agents pane.**~~ **Answered.** The design landed one (`Strata.dc.html`,
  `data-pane="agents"`) and it settled three things the first build had guessed: the rail order
  is Catalog · Agents · Connections with a live-agent count on the button, a run promotes into a
  **new** tab, and the pane header carries the ⓘ that explains the query-session model.
- **MCP resources** (schema-as-resources beside the tools) — revisit when a client benefits.
- **Curated writes** (register table, save view, export) — the vocabulary grows new, separately
  permissioned tools if ever; `run` never loosens.
- **Claude Desktop pairing** — Desktop favors stdio; a thin stdio↔HTTP proxy mode
  (`strata mcp --connect`) is a small follow-on if wanted (the JetBrains shape).
