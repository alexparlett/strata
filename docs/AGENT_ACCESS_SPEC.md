# Agent access — MCP host, agent-driven query tabs, chat pane (AA)

Spec for **agent-driven access** to a Strata project's data: an AI agent (Claude Code today, an
in-app assistant later) lists the catalog, inspects schemas, and runs read-only SQL — with every
query it executes landing as a **real query tab** in the project window, on the ordinary
press → snapshot machinery. Design settled 2026-07-30 with Alex; workstream:
`.claude/tasks/workstream-agent-access/`.

The one-sentence architecture: **one read-only tool vocabulary over one UI bridge, with thin
swappable frontends** — an MCP server first (any MCP client is the chat surface), a native chat
pane as the flagship follow-on, a headless CLI host for app-closed use. The frontends are
sequencing, not architecture: a chat pane's LLM loop needs the same Tokio runtime and the same
UI seam the MCP server does, so nothing built for the first frontend is discarded by the second.

---

## 1. Direction (decided)

- **Agent queries are ordinary runs.** A tool call sets a real `QueryTab`'s
  `QuerySpec` request — the same press a human makes — so freya-query caching, snapshot
  materialization, supersede/retire, the request keeper, history and the event log all apply
  with **zero new lifecycle semantics**. The investigation trail *is* the tab strip: each step
  the agent parks is an immutable snapshot the user can page, sort, export, or take over.
- **Agent-managed tab handles**, not one-tab-per-query and not one reused tab. The vocabulary
  exposes `open_tab` / `run(tab, sql)` / `read_page` / `close_tab` / `list_tabs`, so the agent
  works tabs like a person: scratch iterates in one tab, findings get parked each in their own.
  Tab tidiness is promptable, not hard-coded.
- **Tabs are shared, last-writer-wins.** No ownership state is modeled at all — a tab the agent
  opened is just a tab. The user typing into it, re-running it, or closing it needs no rule
  beyond what tabs already do; the agent's next write to a closed handle gets "tab not found".
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
  list when there is more than one (`project` param disambiguates; a tab handle is unambiguous
  from `open_tab` on).
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
│          │ AgentAsk over            │ Arc<Engine> direct                       │
│          │ tokio mpsc               │ (fetch_page/validate/functions)          │
│  ┌───────▼───────────┐              │                                          │
│  │ per-window bridge │              │                                          │
│  │ use_agent_bridge  │              │                                          │
│  │ driver + keepers  │              │                                          │
│  └───────┬───────────┘              │                                          │
│          │ Radio writes             │                                          │
│  ┌───────▼──────────────────────────▼───────────┐                              │
│  │ project window: SessionState tabs → QuerySpec │                             │
│  │ press → Engine (private Tokio rt) → snapshot  │                             │
│  └───────────────────────────────────────────────┘                             │
└────────────────────────────────────────────────────────────────────────────────┘
   headless: `strata mcp <project>` = same vocabulary + Host over a plain Engine,
   stdio transport, no bridge (no UI to bridge to)
```

Crate layout:

- **`strata-agent`** (new workspace member) — the tool vocabulary (schemas + semantics + error
  mapping), the `Host` trait, the rmcp server, and the headless host. Depends on `strata-core` +
  `strata-model` + `rmcp`; **no Freya dependency**, which is what keeps the vocabulary reusable
  by the chat pane and testable against a mock host.
- **`strata-freya`** — the in-app `Host` impl: the service directory, the per-window bridge
  (driver hook + agent keepers), server lifecycle, Settings pane.
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
(catalog, tabs, runs) and hands back engine handles for data-plane reads. In-app it is the
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
4. **Settle observation without double execution.** The request keeper (`views/keeper.rs`)
   mounts one invisible `use_query` subscriber per press, built through `QuerySpec::query`
   (the settings are cache identity — a subscriber attaches to the in-flight execution rather
   than re-dispatching). The bridge copies this exactly.

**Control plane** — everything that touches UI state (tab ops, runs, catalog reads) travels as
`AgentAsk { op, reply: oneshot::Sender<…> }` on a `tokio::sync::mpsc` channel per project
window. `use_agent_bridge` (mounted by `ProjectRoot`, beside the keepers) spawns the serial
driver loop: `rx.recv().await` → perform the Radio write (open/close tab, set
`QueryTab::request` — an ordinary press on `Chan::Request(id)`) or read (catalog projection) →
reply, except for runs, whose oneshot is **parked** keyed by the press nonce.

**Agent keepers** — one invisible component per parked run-ask, mounted like `RequestKeepers`:
`use_query(spec.query(&engine))` on the *same* spec the press set (same cache entry; freya-query
attaches to the in-flight execution), and on settle completes the parked oneshot — waking the
server task. The keeper unmounts when its ask is answered; the tab's own request keeper goes on
holding the cache entry, recording history and logging the run exactly as for a human press.

**Data plane** — reads that are engine-scoped and side-effect free skip the UI entirely: the
server holds the window's `Arc<Engine>` (from registration, below) and calls
`fetch_page` (snapshot-scoped, cacheable), `validate` (dry-plan, total by design) and
`functions()` directly from its own runtime. Bulk rows never queue behind UI work.

**Service directory** — each project window registers `(root, name, Arc<Engine> capture,
ask-sender)` with the server on mount and deregisters via `use_drop`. A cross-thread
`Mutex<Vec<…>>` service registry, the `platform::windows` shape — *not* reactive UI state. (The
no-registry rule in AGENTS.md §4 governs UI consumers threading reactive data; this is a DI seam
between threads, the thing context is for in-tree and a directory is for across them.)

**Lifecycle honesty, for free or nearly:**

- A **user re-run** over an agent's in-flight press supersedes it; the agent's awaited settle
  gets the engine's stopped-on-purpose string and the tool reports "your run was replaced in the
  app" (§7) — never a fault.
- A **window close or re-root** drops the bridge and the registration (`use_drop`); parked
  oneshots drop, the server task sees the channel close and answers "project window closed".
  Because re-root *is* the remount path (AGENTS.md §2), no second cleanup path exists to drift.
- Agent runs **count toward the T2 close confirm** automatically (`Engine::watch_inflight` —
  the engine is the only thing that knows), and appear in **history** and the **event log**
  through the tab's own keeper. The bridge driver additionally logs the agent's *actions* it
  alone observes ("Agent opened tab", "Agent ran query") — the observer-records rule.

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
| `open_tab()` | control | Opens a real `QueryTab`; returns the tab handle (its `TabId`). |
| `list_tabs()` | control | Open tabs: handle, title, whether a run is settled/in flight. |
| `run(tab, sql, mode?, page_size?)` | control | The policy gate (§6), then an ordinary press: sets the tab's `QuerySpec` and awaits settle. Returns columns, page-1 rows, exact total, elapsed. `mode` = `run` (default) \| `explain` (returns the plan tree, materializes nothing). `page_size` bounded (default: the app's default row limit setting). |
| `read_page(tab, page, sort?)` | data | Pages the tab's last settled snapshot via `Engine::fetch_page` — snapshot-scoped, side-effect free. A snapshot retired by a newer run in that tab fails cleanly: "result was replaced; re-run" (§7). |
| `close_tab(tab)` | control | Closes the tab (through the same close funnel the UI uses — a running press is cancelled the ordinary way). |

Notes that are rules, not details:

- **`list_tables` answers from the store, never DataFusion introspection.** Introspection would
  surface the `__snap_*` result snapshots and hide defs whose registration failed — precisely
  the rows the catalog exists to show (the P3-02 correction, applied here on day one). In-app
  the bridge projects `ProjectState`; headless the host answers from `load_defs` + the
  registration outcomes it itself produced.
- **`run` never rewrites SQL.** No injected `LIMIT`: the run materializes exactly what the user's
  own press would (same cost, same snapshot), and the *response* is bounded by `page_size` +
  paging. Totals are always exact (the snapshot knows).
- **`run` reuses a tab the way a press does**: dispatch supersedes the tab's in-flight run and
  retires its previous snapshot. That is the point — agent semantics *are* app semantics.
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

## 7. Error taxonomy

Every error an agent can see is one of these, and stopped-on-purpose is never a fault
(AGENTS.md §2 — the engine's `stopped_on_purpose` predicate is the only thing that knows):

| Class | Trigger | Shape |
|---|---|---|
| Policy refusal | `run`/`validate` on blocked DDL/DML | The editor's own message, verbatim; names the owning surface. |
| Query error | The engine's `Err` from a real fault | The engine message, unedited (it already reads like an IDE's). |
| Stopped on purpose | User cancel / user or agent re-run superseding | A distinct non-fault outcome: "the run was cancelled in the app" / "replaced by a newer run". |
| Result moved | `read_page` on a retired snapshot | "The tab's result was replaced; re-run to read it." |
| Not found | Unknown tab handle / table name | Plain statement; `list_tabs` / `list_tables` are the recovery. |
| Ambiguous project | >1 window open, no `project` | Lists the open projects. |
| Window gone | Bridge dropped mid-ask (close / re-root) | "The project window closed." |
| Unauthorized | Bad/missing token | HTTP 401 before any tool runs. |

## 8. State ownership

What lives where — nothing here adds state to the session or project stores:

| State | Owner | Notes |
|---|---|---|
| Server socket, token check, tool router | `strata-agent` server | Own Tokio runtime; the Engine pattern. |
| Service directory (root, name, engine, ask-sender per window) | server, `Mutex` | Windows register on mount, deregister on `use_drop`. |
| Last settled snapshot per agent run (for `read_page`) | server | A cache of a fact the settle reply carried; a retired snapshot fails cleanly, so staleness is honest. |
| Ask channel + driver loop + parked run replies | the window (`use_agent_bridge`) | Dies with the window/re-root — no cleanup path to drift. |
| Agent keepers (settle observers) | the window, per parked ask | The `RequestKeepers` pattern, verbatim. |
| Tabs, requests, snapshots, history, events | **unchanged** | Agent runs are ordinary presses; nothing new is stored anywhere. |
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
- **Execution surface stays the tab strip:** the assistant's queries land as agent tabs exactly
  like MCP-driven ones; the transcript shows compact step cards (SQL + row count + elapsed)
  that focus the tab on press. Inline mini-results (first rows) with "open in tab" for anything
  bigger.
- **The brain is deliberately undecided** — native Anthropic API client (app-owned loop, API
  key in Settings) vs a Claude Agent SDK / CLI sidecar (reuses the user's subscription and
  Anthropic's context management, costs process management + an install dependency). That
  decision is the chat workstream's first task, and nothing in the core prejudges it: both
  brains drive the identical tool layer.

## 10. Headless host (`strata mcp <project>`)

For app-closed use (CI, scripts, a second machine): the same binary, a CLI branch in `main()`
**before** any GUI launches — beside the existing `argv[1]` project-path handling.

- Builds a plain `Engine` and replays the **project registration pass** (load defs → register
  tables → create views), extracted to `strata-core` by AA-01 from its current home in the
  Freya app's project-open hooks. Registration failures become `list_tables` `failed` rows,
  exactly as in-app.
- **stdio transport** (the standard for locally-spawned MCP servers) — no port, no token; the
  client owns the process.
- Touches **no shared state**: never reads or writes app config, `session.json`, or history.
  Snapshot directories are already safe side by side — each engine lock-claims its own dir
  (`claim_snapshot_dir`), so a headless engine beside a running app cannot collide with it.
- Tab handles still exist (a tab is a `WsId` — headless they're just workspaces with no UI),
  so agents see one vocabulary everywhere.

## 11. Open questions

- **Settled-profile exposure.** Profile numbers live in the freya-query cache keyed by the scan
  request; surfacing them through `describe_table` means the bridge reading another surface's
  cache entry. Deferred until wanted — the free tier may be enough.
- **Agent-tab marker.** Last-writer-wins means no ownership state, so a visual "opened by
  agent" marker would be a transient decoration, not state. Whether the tab strip shows one is
  a design call — parked for the designer, not built speculatively.
- **MCP resources** (schema-as-resources beside the tools) — revisit when a client benefits.
- **Curated writes** (register table, save view, export) — the vocabulary grows new, separately
  permissioned tools if ever; `run` never loosens.
- **Claude Desktop pairing** — Desktop favors stdio; a thin stdio↔HTTP proxy mode
  (`strata mcp --connect`) is a small follow-on if wanted (the JetBrains shape).
