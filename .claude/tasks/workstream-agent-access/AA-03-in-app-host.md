# AA-03 · In-app host: service directory · bridge · agent keepers · server lifecycle

**Workstream:** Agent access · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** AA-02

## Goal
The centerpiece: the running app implements AA-02's `Host`, so an MCP client (Claude Code)
investigates a live project and **its queries land as real tabs** in the window. Everything here
is the verified bridge design of `docs/AGENT_ACCESS_SPEC.md` §4 + the dataflow diagram —
re-read both before starting; the seam was checked hop by hop against the fork
(`FuturesWaker`/`EventLoopProxy` wake, runtime-agnostic `tokio::sync` channels, the `spawn`
driver shape, keeper-style settle observation) and the design is not to be re-derived.

## Current state
AA-02 ships the vocabulary + server, tested against a mock host. The app has no agent code. The
patterns to copy all exist: the diagnostics driver
(`apps/project/state/diagnostics.rs` — spawned serial loop writing Radio state), the request
keepers (`apps/project/views/keeper.rs` — invisible `use_query` subscriber per press), the
windows registry (`platform/windows.rs` — the service-directory shape).

## What to build

### Service directory
A cross-thread `Mutex` registry on the server side: each project window registers
`(root, name, Arc<Engine> capture, ask-sender)` on mount and deregisters via `use_drop` — a hook
mounted by `ProjectRoot` so a **re-root** (project-folder diff key) deregisters and re-registers
through the same mount/drop path, never a second cleanup route. This is a DI seam between
threads (the `platform::windows` shape), not reactive UI state — the AGENTS.md §4 no-registry
rule governs UI consumers and does not apply.

### The bridge (`use_agent_bridge`, mounted by `ProjectRoot`)
- One `tokio::sync::mpsc` ask channel per window; the hook `spawn`s the serial driver loop:
  `rx.recv().await` → perform the Radio write/read → reply on the ask's oneshot.
- Tab ops go through the session store's existing paths: open tab, list tabs, close tab
  (**through the same close funnel the UI uses**, so a running press is cancelled the ordinary
  way). Catalog reads project `ProjectState` rows (store, not introspection).
- A `run` ask sets the tab's `QuerySpec` request on `Chan::Request(id)` — an ordinary press —
  and **parks** its reply oneshot keyed by the press nonce.
- **Agent keepers**: beside `RequestKeepers`, one invisible component per parked ask mounting
  `use_query(spec.query(&engine))` — same cache identity, attaches to the in-flight execution —
  completing the parked oneshot on settle. The tab's own keeper still records history and the
  run log; the agent keeper only answers the ask.
- `read_page` never arrives here — it is data-plane (server → `Arc<Engine>` direct). The server
  remembers the last settled snapshot per run reply; a retired snapshot fails cleanly ("result
  was replaced; re-run").

### Event log
The driver logs what only it observes — "Agent opened tab", "Agent ran query in '<tab>'",
"Agent closed tab" — via `LogCtx` captured at the hook, per the observer-records rule. Run
*outcomes* are already logged by the tab's keeper; don't double-log settles.

### Server lifecycle
Start `AgentServer` from `main` when the `agent_access.enabled` setting is on (AA-04 builds the
control; until then the setting exists with default **off** — dark launch). React to the setting
on `ConfigChan::Settings` (start/stop live), the `use_engine_config` shape. Port + token come
from settings (constants for the default port and token minting live here or in AA-02 — one
place).

## Acceptance
- With the setting enabled (hand-edited config until AA-04), `claude mcp add --transport http
  strata http://127.0.0.1:<port>/mcp --header "Authorization: Bearer <token>"` connects, and a
  Claude Code session can: list projects/tables, describe a table, open a tab, run a query
  **that visibly appears and settles in the window's tab strip**, page it, and close the tab.
- A user re-run over the agent's in-flight press → the tool gets the non-fault "replaced"
  outcome; the app behaves as for any re-run.
- Closing the window (and: re-rooting via "open in this window") mid-ask answers "the project
  window closed"; no panic, no leaked oneshot, registry entry gone.
- Agent runs appear in history and the event log; a running agent press triggers the T2 close
  confirm (`watch_inflight` — should be automatic; verify, don't assume).
- Unit tests where a store is drivable without a renderer (ask-op → store mutation mapping);
  the end-to-end path is verified on a Mac build by hand per the environment note.

## Notes
- Model the ask enum so an op that needs no reply has no oneshot to forget (impossible states).
- The bridge must never hold a Radio borrow across an await (the known GenerationalBox trap).
- Keep the driver serial (one ask at a time) — the diagnostics driver's reasoning applies: the
  engine has two workers and the user's own press comes first.
