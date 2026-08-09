# Agent access — how Strata serves MCP

Agent access lets an AI agent — Claude Code, Cursor, Copilot, any MCP client — drive a Strata
project's data: list the catalog, inspect schemas, validate SQL, and run read-only queries.
Every query an agent runs is a **real run** on the project's own engine, producing the same
immutable snapshot a person's press does, in a workspace of the agent's own. The user watches
the agent's work in the sidebar's **Agents pane** and can promote any query it ran into their
own editor with one press.

This document describes the system as built. The user-facing half — turning it on, pointing a
client at it — is the README's *Agent access* section; this is the engineering reference.

---

## One vocabulary, three deployments

The whole feature is one read-only tool vocabulary, `StrataTools`
(`crates/strata-agent/src/tools.rs`), over one seam, the `Host` trait
(`crates/strata-agent/src/host.rs`). `Host` answers the project-scoped questions — which
projects are open, what the catalog holds, who owns which query session — and hands back
engine handles for the data reads. The vocabulary is deployed:

- **In the app** — an MCP server over Streamable HTTP, loopback-only, bearer-authenticated,
  off by default. The `Host` here is the app's service directory
  (`crates/strata-freya/src/agent/directory.rs`), which every project window joins on mount.
- **Headless** — `strata mcp <project>` serves one project over stdio with no window at all
  (`crates/strata-agent/src/headless.rs`; the CLI branch is in
  `crates/strata-freya/src/main.rs`). The `Host` here is a plain `Engine` with the project's
  registration pass replayed over it.
- **In-process** — `StrataTools`' own public methods, with no MCP hop and no rmcp type in any
  signature. The ten tools *are* those methods; the `#[tool]` items are wrappers that add the
  two things a semantic call cannot have (which agent the request is, and holding it against
  the idle sweep) and then delegate. `StrataTools::manifest()` is the vocabulary as plain data
  — name, description, argument schema per tool — **derived from the same router that answers
  `tools/list`**, so an in-process loop offers a model exactly what an MCP client is offered,
  with no second list. An in-process caller is the *owned* case: it holds the value's own
  connection, so its `AgentId` lives as long as its mount and retracts by RAII, and it
  introduces itself with an identity of its own (`AgentIdentity::assistant()` for the
  assistant). The seam is built and tested (`crates/strata-agent/tests/facade.rs`); the loop
  and the pane over it are not (see [What is not built](#what-is-not-built)).

`strata-agent` has **no Freya dependency**, and that is the property doing the work: it is
what lets one implementation of the vocabulary serve HTTP, serve stdio, be called in-process,
and be tested against a mock host with no window or renderer. The crate owns the tool
schemas and semantics, the policy gate's application, the error taxonomy, the HTTP server,
and the headless host; everything that touches a window lives in `strata-freya`.

**Why Streamable HTTP and not a Unix socket:** the transport menu is the client's, not ours.
MCP clients speak two spec transports — stdio, where the *client spawns* the server process,
and Streamable HTTP. Stdio is structurally impossible for the in-app host (the server lives
inside an already-running GUI app; nothing spawns it), and no client dials a Unix socket, so
a socket server would force a proxy into every connection. Hence the embedded-localhost-HTTP
shape, compensated with the loopback bind and the bearer token. The headless host uses stdio
because there the client really does own the process.

## The ten tools

All tools are project-scoped unless noted. `project` is optional everywhere it appears and
only required when more than one project window is open — the error lists the candidates. A
project is resolved by its **root** first (the identity — a window is keyed on its project
folder, so a root names at most one) and then by its **name**, which may collide and reports
ambiguity rather than guessing.

| Tool | Answers |
|---|---|
| `list_projects` | The open projects: name and root. |
| `list_tables` | The catalog **as the app shows it**: tables, views and saved queries, each with its source and registration state (`ready` / `failed` with the failure message / `pending`). |
| `describe_table(name)` | One table or view: columns and types, nested fields, Hive partition columns, sources and format, plus the row count and column statistics the source reports for free. Only facts that were read — nothing is scanned or estimated. |
| `list_functions` | The engine's live function registry: names, overload signatures, docs. What is registered is what exists — there is no second list to drift. |
| `validate(sql)` | Lints, the read-only policy, and a dry plan against the real catalog, without executing. The cheap way to find a typo before spending a run. |
| `open_query_session()` | Mints a query session for the calling agent and returns its handle. |
| `list_query_sessions()` | **The caller's own** sessions: handle, and whether a run is in flight, settled, or has never happened. |
| `run(query_session, sql, mode?, page_size?)` | The policy gate, then a dispatch straight at the engine on that session's workspace. Returns columns, page-1 rows, the exact total, and elapsed time. `mode: "explain"` returns the logical and physical plan as text and materializes nothing. |
| `read_page(query_session, page, sort?)` | Pages the session's last settled result — an immutable snapshot, so paging (and re-sorting) never re-runs the query. A snapshot retired by a newer run in the session reports "the result was replaced; re-run". |
| `close_query_session(query_session)` | Drops the session and tears its engine workspace down, cancelling any run still in flight. Tidy rather than required — every session goes when its connection does. |

Rules that hold across the vocabulary:

- **`run` never rewrites SQL.** No `LIMIT` is injected: the run materializes exactly what the
  user's own press would, and the *response* is bounded by `page_size` plus paging. Totals
  are always exact, because the snapshot knows. `page_size` defaults to the app's row-limit
  setting and is capped at `MAX_PAGE_SIZE` (10,000 rows); the response reports the size
  actually used, so the clamp is visible rather than a silent truncation.
- **`list_tables` answers from the app's own catalog, never DataFusion introspection.**
  Introspection would surface the engine's internal `__snap_*` result snapshots and hide defs
  whose registration failed — precisely the rows the catalog exists to show. A table that is
  merely broken must not look like a table that was never registered.
- **`explain` goes over the wire as text** — what `EXPLAIN` prints, the form every SQL tool
  shows and the one an agent can read. The app's structured plan tree exists to be *drawn*
  (it carries accent colours and time-share bars); off-screen it would be the same tree
  twice, once in a shape nothing can use. `mode: "explain"` means "plan this statement": the
  host wraps the SQL itself, exactly as the app's own Run does, and never `ANALYZE`, which
  would execute the query the caller was avoiding.
- **Every session-scoped tool is scoped to the calling agent** — a property of the types, not
  a check anyone has to remember (see [Identity and teardown](#identity-and-teardown)).

## Agent runs are real runs

A query session is an agent-managed handle: the agent opens sessions, runs in them, and
closes them, so scratch work iterates in one while findings get parked each in their own.
Each session maps onto an engine workspace id (`WsId`) — the same unit a user's editor tab
runs against — which is what keeps an agent's runs *real*: same engine, same snapshot
materialization, same supersede-and-retire when a newer run lands in the same session, same
cancel. A second `run` in a session replaces the first, exactly as a second press in a tab
does.

What a query session is **not** is a tab. An earlier version of this feature landed agent
runs in the user's own editor tabs, on the premise that the tab strip is the investigation
trail. That premise holds for someone watching the window and fails for an MCP client in a
terminal on another desktop: twenty agent queries moved the editor out from under whoever was
typing, left twenty tabs to close, and cost a diagnostics pass each on the engine the user's
own press was waiting for. The rule that replaced it: **a surface's state belongs to whoever
is looking at that surface.** "Shared, last-writer-wins" is a fine rule for content and a bad
one for attention. So an agent that is not in the window gets a surface of its own — the
Agents pane — and nothing it does opens, focuses, or closes a tab.

Consequences, all deliberate:

- **The user can take over anything the agent found.** A press on a run row in the Agents
  pane opens its SQL in a **new** tab (through the editor's ordinary `actions::open_sql`
  funnel) — never the tab the user is working in, because arriving unasked in the active tab
  is exactly the harm the pane exists to prevent. The result snapshot itself is pageable,
  sortable and exportable like any other.
- **Agent activity reaches neither `session.json` nor `.strata/history.jsonl`.** The session
  store is tabs, layout and geometry, and an agent owns no tabs — so reopening a project
  cannot restore work the user never asked for. History is capped and deduplicated before
  the cap, so exploratory agent queries would evict runs the user actually made; history
  records what *the user* ran. A promoted query the user presses Run on enters history the
  ordinary way. The Agents pane's record is ephemeral and bounded, like the event log.
- **Agent runs still count as work.** The close confirm asks before destroying a run in
  flight, agent or not — the predicate is the engine's own in-flight answer. What changes is
  the sentence: the dialog asks about the tabs' workspaces and the agents' workspaces
  separately, and says "An agent is running a query. Stop it and exit?" when only the latter
  answers, because "Queries are running" shown to somebody who pressed nothing sends them
  hunting for a query they never started.
- **The event log records the agent's actions** — a session opened, an agent disconnected —
  written by the window's driver, which is the layer that observed them.

## The policy gate

Agent access is read-only: queries, `EXPLAIN`, `SHOW` and `DESCRIBE` run; every write-shaped
statement is refused. The refusal is the app's own: the engine's statement classification —
the same one the editor's validation uses — is read at the agent capability, and `run` asks
`Engine::policy_verdicts` **before dispatch**, refusing any flagged statement with the same
message text the editor shows ("CREATE TABLE is not supported in the editor. Register tables
in Table Config", and kin). One predicate, two surfaces, zero copies. The editor's own
refusal list has since narrowed as statement support landed there; the agent capability
deliberately keeps the full read-only set and the original wording.

The gate fails closed: SQL that cannot be judged (it did not parse) is refused with the
engine's own parse wording rather than dispatched to fail downstream, and an empty statement
is refused outright rather than left as a failed run the user never made. `run` never edits
the SQL to make it pass.

A **stopped run is not a fault.** A cancel from the app, or a supersede by a newer run, is
reported as a distinct non-error outcome (`stopped`, with the reason) — the judgment is
`stopped_on_purpose`, the engine's own predicate, asked in exactly one place. An agent that
reads "the run was cancelled in the app" can decide to re-run; an agent that reads a fault
would apologise and give up.

**Profiling is not exposed.** A profile is the most expensive thing the app does, gated
behind a per-entry cost confirm; a tool call that blocks on a user dialog is a bad tool, and
skipping the confirm would bypass the app's own gate. `describe_table` reports the free tier
only — what registration read from footers and metadata.

## How a run travels: two planes

Inside the app, the server lives on its own small Tokio runtime (rmcp needs a reactor and the
UI thread is not one), while tabs, the catalog store and the Agents pane are UI-thread state.
Traffic between them is split by what it touches:

- **Control plane** — anything that reads or writes window state travels as an `AgentAsk` on
  a **bounded** `tokio::sync::mpsc` channel per project window, each variant carrying its own
  oneshot reply. Beside it runs an **unbounded** one-way `AgentNotice` channel for facts that
  carry no answer. The split is load-bearing rather than tidy: a `send().await` with
  backpressure is right for a tool call, but the most important notice of all — an MCP
  connection ending — is sent from a `Drop`, which has nothing to await on and nowhere to
  report a failure to.
- **Data plane** — the engine, direct. The server holds each window's `Arc<Engine>` and calls
  it from its own runtime: `fetch_page`, `validate` and the function listing are side-effect
  free and snapshot- or engine-scoped, so bulk rows never queue behind UI work. The engine
  facade's futures are executor-agnostic, which is what makes awaiting it from a foreign
  runtime ordinary.

Each project window registers itself — root, name, engine handle, ask and notice senders —
with the process-wide service directory on mount and deregisters on unmount, so a window
close or a re-root tears the seam down through the same path an open built it. Per window,
`use_agent_bridge` (`crates/strata-freya/src/apps/project/state/agent.rs`) spawns **one
serial driver** that drains both channels, asks first. It records what agents are doing in
the **agents satellite** (`state/agents.rs`) — the bounded, ephemeral record behind the
Agents pane, capped per session and per agent the way the event log is capped.

A run itself is dispatched **by the caller and bracketed by the window**. The dispatch goes
straight at the engine on the session's own workspace; the window owns only the half that
only it can answer, and that half travels either side of the dispatch: `RunStarting` first —
the ownership check (does this agent hold this session?) plus the record of what is running,
replying with a **sequence number** the dispatch minted — then `RunSettled` after, naming
that same sequence number, so a slow query's outcome can never land on a faster one the agent
pressed after it. Because the driver awaits the ask before the engine is touched, a settle
cannot overtake its own dispatch — the ordering is structural.

```mermaid
sequenceDiagram
    participant C as MCP client (Claude Code)
    participant S as strata-agent server<br/>(own Tokio runtime)
    participant D as use_agent_bridge driver<br/>(UI executor, per window)
    participant A as state::agents satellite<br/>(→ the Agents pane)
    participant E as Engine<br/>(private Tokio runtime)

    Note over S: token checked, project resolved<br/>(default single; error lists when >1)<br/>one StrataTools per connection = one AgentId

    C->>S: open_query_session()
    S->>D: AgentAsk{OpenQuerySession, agent + clientInfo}
    Note over D: send invokes the receiver's waker →<br/>FuturesWaker → EventLoopProxy(PollFutures)
    D->>A: record the agent (first time) and its session
    D-->>S: QuerySessionId — which *is* the engine WsId it runs on
    S-->>C: handle

    C->>S: run(query_session, sql)
    S->>S: policy gate (exported strata-core verdict)<br/>refuse blocked DDL/DML before dispatch
    S->>D: AgentAsk{RunStarting} — does this agent hold this session?
    D->>A: record the query as in flight
    D-->>S: Ok(seq) — the run's sequence number
    S->>E: query(WsId::from(session), sql) — data plane, direct,<br/>awaited on the server's own runtime
    Note over E: a real execution: same snapshot lifecycle,<br/>same supersede + retire-on-dispatch, per WsId
    E-->>S: settle (QueryOutput | Err)
    S->>D: AgentNotice{RunSettled, seq, outcome}<br/>one-way — by now there is nobody to refuse it
    D->>A: resolve that run in place, **by seq**<br/>(a slow settle must not land on a newer query)
    S-->>C: columns, page-1 rows, exact total, elapsed<br/>(stopped-on-purpose mapped to non-fault)

    C->>S: read_page(query_session, page, sort)
    S->>E: fetch_page(snapshot, …) — data plane, no UI hop
    E-->>S: page rows (retired snapshot → "result moved; re-run")
    S-->>C: page

    Note over C,S: the connection ends → Connection::drop → Host::agent_gone<br/>(sync, unbounded send, broadcast to every window)
    S->>D: AgentNotice{AgentGone}
    D->>A: drop the agent; retire each session's engine workspace

    Note over D,A: window close / re-root drops bridge + registration (use_drop);<br/>reply channels drop → server answers "project window closed"
    Note over A: a press on a run row opens its SQL in a **new** tab (actions::open_sql)<br/>— never the tab the user is working in, which is the harm the pane exists to prevent
```

For `read_page`, the server keeps a small cache of each session's last settled result — the
snapshot id, the wire schema, the exact total — keyed by `(agent, project root, session)`,
with the agent *in* the key rather than checked against it. It is deliberately **not** a
snapshot pin: pinning is right for an export window, which owes the user the rows it was
opened on, and wrong for a long-lived server, which would keep dead results alive. A snapshot
retired by a newer run fails the read cleanly ("the result was replaced; re-run"), judged by
asking the engine whether the snapshot is live — never by matching on error prose. The cache
also remembers **which engine** minted the snapshot, because snapshot ids are a per-engine
counter: after an engine restart at the same root, a remembered id would otherwise resolve
against whatever the user has since run.

## Identity and teardown

One `StrataTools` **is** one agent. The transport builds one per client connection; it mints
an `AgentId` on creation and retracts it from a `Drop` when the connection ends — RAII,
because a connection ending is not an event anything on our side is told about, so the drop
of the value the transport owns is the only honest place to notice. Every session-scoped
answer is scoped by that id, so "only your own sessions" is a property of the type rather
than a check: an agent is never handed a handle on another agent's work, let alone on the
user's tabs, because it never receives one. A handle that belongs to another agent gets the
same "no open query session" a made-up handle does — a distinct "that is not yours" would
confirm the session exists, which is a fact an agent has no business learning.

The wrinkle is that a service value's lifetime is the connection's on only *some* of the
transport's paths: rmcp's stateless branch (taken for clients negotiating the newer protocol
lifecycle) builds one service per **request**. So identity is resolved from the request,
through a `Caller` value that mirrors rmcp's own lifecycle predicate — never the
`Mcp-Session-Id` header, which looks like the discriminator and is not one, and never the
peer info, whose stateless default reads as the rmcp library rather than the client. A call
with no HTTP request behind it at all (stdio, or in-process) is the value's own connection. A
stateless client is identified by the `clientInfo` it sends per request; a **blank** one is
refused the session-scoped tools rather than pooled, because pooling every anonymous client
into one shared agent would hand them each other's sessions.

Teardown follows the same honesty:

- A connection's drop retracts its agent everywhere: every window's driver drops the agent's
  rows and retires each of its sessions' engine workspaces.
- Stateless agents, having no connection to key on, are retired by an **idle sweep**
  (5 minutes, matching rmcp's own session keep-alive). The sweep skips a busy agent — a run
  can sit on the engine for minutes, and a timer must not cancel an agent's own query — and
  runs once more when the server itself is dropped, so nothing leaks on shutdown.
- A close racing a dispatch is a **tombstone**: the handle stops answering immediately and
  the engine is aborted immediately (a runaway scan must not burn to completion with no
  handle left to stop it), but the internal row waits for the last settle to sweep it, so a
  late settle finds its bracket instead of a hole.
- A window close or re-root drops the bridge and the registration; parked replies see their
  channels close and the server answers "the project window closed". Because re-root *is* the
  remount path, no second cleanup path exists to drift.

## Error taxonomy

Every error an agent can see is one of these (`crates/strata-agent/src/error.rs`):

| Class | Trigger | Shape |
|---|---|---|
| Policy refusal | `run` / `validate` on a write-shaped statement | The editor's own message, verbatim; names the owning surface. |
| Query error | The engine's `Err` from a real fault | The engine message, unedited — it already reads like an IDE's. |
| Stopped on purpose | User cancel, or a newer run superseding | **Not an error**: a distinct outcome shape with the reason. |
| Result moved | `read_page` on a retired snapshot | "The result was replaced; re-run." |
| Not found | Unknown session handle or table name | Plain statement; `list_query_sessions` / `list_tables` are the recovery. Another agent's handle gets this same answer, deliberately. |
| Ambiguous project | More than one window open, no `project` (or a name two windows share) | Lists the open projects. |
| No project | Nothing open to address | "No project is open." |
| Window gone | Bridge dropped mid-ask (close / re-root) | "The project window closed." |
| Unauthorized | Bad or missing token | HTTP 401, answered by the transport before any tool runs. |

Everything but the last is an MCP tool result with `isError: true`, not a JSON-RPC protocol
error: these are conditions an agent should read and recover from, and the listing tools are
the recovery. Protocol errors stay for what they are for — a malformed request.

## The in-app server

The server is one app-wide Streamable-HTTP listener: bound to `127.0.0.1` only, serving MCP
at `/mcp`, requiring a bearer token on every request. The 401 is answered in front of the
router, so an unauthorized request never reaches a tool. **It is off by default** — the
capability ships dark until the user enables it.

**Settings ▸ Agent access** holds the three controls: the enable switch, the port, and the
token (shown masked, with reveal, copy and regenerate). The port defaults to **47821** —
above the registered range, claimed by nothing common — and is fixed rather than ephemeral so
a pasted client configuration keeps working across launches. The token is minted on first
enable and **persisted** in app config for the same reason: a token regenerated per launch
would invalidate the configuration the user pasted last time. Regenerate edits the settings
draft like every other control, so Cancel is its undo. Applying settings starts, stops or
restarts the server in place; no app restart is involved.

The window header shows a **status dot** whenever the feature is enabled: amber when the
setting is on but nothing is listening (the port was taken — the one case where the user
asked for agent access and has not got it), grey when listening with no client, green with
the connected-client count in its tooltip. The paired-client count is the one polled fact in
the app, because it is rmcp's own and changes below our seam; the poll exists only while the
feature is on.

Client configuration, in whole:

```bash
claude mcp add --transport http strata http://127.0.0.1:47821/mcp --header "Authorization: Bearer <token>"
```

The README's *Agent access* section covers other clients, including the stdio-only ones that
need a proxy in front of an HTTP server.

## The headless host: `strata mcp <project>`

For app-closed use — CI, scripts, a second machine — the same binary serves the same
vocabulary over **stdio**: `strata mcp <project folder>`. The CLI branch is taken in `main`
before anything app-global is built, because none of it exists for a server with no window.
No port and no token: the client spawns and owns the process, and process ownership is the
authentication.

`HeadlessHost` builds a plain `Engine` and replays the project's **registration pass** over
it — connect the project's object stores, register tables, create views — the same pass a
window runs at open, exported from `strata-core` so there is exactly one. The pass's outcomes
*are* the catalog: folded once at startup into the same shapes the app projects from its
store, so a def the engine refused is a `failed` row carrying its error, exactly as in-app.
The pass completes before anything is served, so there is no registration window to race.

What it deliberately does not touch:

- **No app config, no `session.json`, no history.** Two stated consequences of not reading
  app config: the engine runs DataFusion's defaults (no `datafusion.*` overrides), and the
  default page size is the shipped default rather than the user's setting.
- **No writes to the project.** A folder with no project in it is **refused** with a message,
  never scaffolded — a server the user cannot see should not create the files the app owns.
- **Logging goes to stderr**, always: stdout is the MCP transport's, and one stray log line
  on it is a parse error at the client.

Running beside the live app is safe for the reason two app windows are safe side by side:
every engine lock-claims its own snapshot directory. Query sessions work identically —
headless they are engine workspaces with no UI at all — so an agent sees one vocabulary
everywhere, the close-racing-a-dispatch tombstone included. One project by construction, so
the `project` argument resolves to it or to nothing. There is no idle sweep here: stdio has
exactly one client and its departure closes the transport, so the service value's drop *is*
the disconnection.

## What is not built

Stated so the reader does not go looking:

- **The in-process assistant pane** — a native conversation surface in the project window.
  The vocabulary underneath it is built and driveable with no MCP peer at all (AS-01, above),
  but there is no agentic loop, no provider client and no pane. That shape is settled: the app
  owns the loop, and the provider is pluggable (`genai` — Anthropic, OpenAI, Gemini, Ollama,
  OpenAI-compatible), chosen in Settings. Binding a model's tool call *by name* to a facade
  method belongs with the loop, where the provider's own tool-call type lives. The design and
  decision record are in `.claude/tasks/workstream-assistant/`.
- **MCP resources** — the vocabulary is tools only.
- **Curated writes** (register a table, save a view, export). If they ever arrive, they
  arrive as new, separately permissioned tools; `run` never loosens.
- **A stdio↔HTTP proxy mode** for stdio-only clients pairing with the in-app server; today
  such clients use a generic proxy like `mcp-remote`.
