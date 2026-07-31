# AA-03 · In-app host: service directory · bridge · agent keepers · server lifecycle

**Workstream:** Agent access · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** AA-02

## Goal
The centerpiece: the running app implements AA-02's `Host`, so an MCP client (Claude Code)
investigates a live project and **its queries land as real tabs** in the window. Everything here
is the verified bridge design of `docs/AGENT_ACCESS_SPEC.md` §4 + the dataflow diagram —
re-read both before starting; the seam was checked hop by hop against the fork
(`FuturesWaker`/`EventLoopProxy` wake, runtime-agnostic `tokio::sync` channels, the `spawn`
driver shape, keeper-style settle observation) and the design is not to be re-derived.

## What was built

```
src/agent/                       the app-wide half — what outlives any one window
  mod.rs                         AgentCtx (directory + server slot) + create_global_agent
  ask.rs                         AgentAsk: one variant per Host method that touches UI state
  directory.rs                   the cross-thread service registry AND the app's `Host` impl
  server.rs                      use_agent_server: start/stop off the setting; Running
  status.rs                      the header's dot (AgentStatusDot)
src/apps/project/state/agent.rs  the window's half: use_agent_bridge (driver) + the projections
src/apps/project/views/agent_keeper.rs   AgentKeepers — the parked replies' settle observers
```

The split is deliberate and was corrected mid-build: the bridge and the keepers started life
under `src/agent/` and moved, because they *are* a window driver and a window pin — the moment
they needed `SessionState`, `QuerySpec` and `Chan`, keeping them outside `apps::project` meant
widening that module's public surface for one consumer. `src/agent/` now holds only the three
things that outlive a window, and neither half of the seam reaches into the other's internals.

**Settings.** `Settings::agent_access` (`AgentAccess { enabled, port, token }`), through
`settings_merge!`. Off by default. Default port `47821`. The **token is minted on first use and
persisted** (`strata_agent::mint_token`, beside the empty-token refusal it exists to satisfy),
never by `Default` — a serde default would mint a fresh one on every load of a file that lacks
the field and nothing would write it back, invalidating the client config on every launch.

## What it settled

- **A registry between threads is not the registry AGENTS.md §4 rules out.** That rule governs
  reactive UI state threaded through one value; the directory is a DI seam between the server's
  runtime and the render thread, where context cannot reach. It is `platform::windows`'s shape,
  one thread further out. Registration is per **mount** of the project subtree, keyed by a
  minted `RegId` rather than by the project root — an engine restart remounts at the *same*
  root, and keying on it would make Freya's drop-before-mount ordering load-bearing for
  correctness rather than tidiness, with a silent failure if it ever changed.
- **The driver is serial and never waits for a query.** A `run` sets the tab's request and
  **parks** its reply against the press's nonce; the agent keeper completes it on settle. So the
  diagnostics driver's "one at a time, the user's press comes first" costs nothing here.
- **A run loads the buffer before it presses it.** The first cut set the request and nothing
  else, which left the tab showing results over an *empty editor* — the premise of landing agent
  queries in real tabs, undone. `actions::load_sql` then `set_request` makes an agent run the
  History drawer's double-press exactly, and keeps one implementation of "replace a tab's text".
- **`close_tab` takes the funnel, not the gate in front of it.** `close_one` plus the root's
  tab-diff effect *is* what cancels a running press the ordinary way; the T2 confirm is a
  question for the *user*, and neither answer works for a tool call — replying Ok while a dialog
  decides reports a tab closed that is still open, and waiting on the dialog is the
  blocks-on-a-modal shape §6 already rules out for profiling. Tabs are last-writer-wins (§1), so
  an agent closing one is a write like any other.
- **An `AGENT` badge was built and then removed, and the removal is the finding.** It was a
  retracting condition done properly (`QueryTab::agent`, set by the bridge, cleared by
  `press_query`, absent from `TabSnapshot`) and it closed the spec's §11 open question. Using it
  showed the question was a *symptom*: needing a badge to say "this tab isn't really yours" is
  the tell that the tab should never have been the user's. That, the focus stealing, the tab
  pollution and the per-tab diagnostics cost are one problem, and **AA-03b** is the answer —
  agent runs move to an Agents pane. The badge, `QueryTab::agent` and the toolbar wiring are
  gone; `open_background` stays until 03b removes the tab landing entirely.
- **The status dot is the app's one polled fact, and the reason is named.** The count lives in
  rmcp's `LocalSessionManager` (whose `sessions` map is `pub`), and a session is created inside
  `service.handle(req)` — below our `serve`, with nothing on our side to observe. Wrapping
  `SessionManager` is ten pass-throughs for a number already public; a channel needs a receiver,
  and a receiver can be taken once, which a status shown in *every* window must not depend on.
  So: `try_read` (never blocks the render thread), a 2s tick, and the timer only exists while
  the setting is on, because `AgentStatusDot` mounts the hook-owning child conditionally.
- **`Size::flex` needs `Content::Flex` on the parent.** The badge's spacer was added without it
  and pushed the badge off the toolbar's right edge — the header bar's cluster had the same two
  lines all along. The badge is gone but the rule went to AGENTS.md §3, where the header bar is
  now the worked example.
- **A tool's `outputSchema` must say `"type": "object"`.** `RunResult` is the vocabulary's one
  sum type, and schemars emits an internally-tagged enum as a bare `oneOf`. A client that
  validates the `tools/list` response rejects that tool **and every other tool with it** — so the
  server connected, reported healthy, answered `tools/list` with all ten, and a fresh Claude Code
  session showed none, with nothing anywhere naming the cause. `#[schemars(extend("type" =
  "object"))]`, plus a test over all nine result shapes so the next sum type cannot repeat it.

## Acceptance — verified live

Driven over real MCP/JSON-RPC against a running app (a small Streamable-HTTP client in the
session scratchpad, since an MCP server can only be attached at a client's session start):

- `initialize` → `tools/list` (all ten) → `list_projects` → `list_tables` (failed defs listed
  with their errors) → `describe_table` → `open_tab` → `run` → `read_page` (paged **and**
  sorted) → `run(mode: explain)` → `close_tab`. ✅
- The run **appears in the window's tab strip**, with its SQL in the editor. ✅ (It appeared with
  an `AGENT` badge too, until that and the whole tab landing were superseded — see above.)
- Policy gate: `CREATE TABLE …` and `DROP VIEW …` refused with the editor's own message. ✅
- `validate` reports a missing column without executing. ✅
- Unknown tab handle → the plain not-found `list_tabs` recovers from. ✅
- Unit tests: the store projections (`state::agent`, 6) and the directory contract
  (`agent::directory`, 7 — an ask round trip, a window's own refusal, all three ways to lose a
  window, a dropped reply, root-vs-name listing, the page-size mirror).

**Not exercised end to end**, and worth doing by hand: a *user* re-run over an agent's in-flight
press (the `stopped` outcome — the mock covers the mapping, and `stopped_on_purpose` is asked in
one place), and window-close-mid-ask (`WindowGone` — covered by the directory test, not by a real
close).

## Notes for what follows

- **AA-04** builds the Settings ▸ Agent access pane, and the README already promises it: the
  Agent access section says the switch, the port and the token live there, and **deliberately
  says nothing about the config file** — encouraging a user to hand-edit `config.prefs.json` is
  encouraging them to fight the one writer of it. Everything the pane needs exists: the setting,
  a live start/stop reconcile keyed on the whole `AgentAccess`, `mint_token` for Regenerate, and
  `AgentServer::addr` / `clients` for a status readout. The one thing to fix there: a failed
  start is currently only a `tracing::error!` (deliberately not remembered, so the next settings
  write retries rather than latching) — the pane is where it gets a status to show.
- **Testing before that pane exists** means setting `settings.agent_access.enabled` to `true` in
  `~/Library/Application Support/Strata/config.prefs.json` with the app **closed** (it reads the
  file once at startup and owns it from then on), leaving `token` empty for the app to mint. That
  is a note for this task file, not for anyone else.
- The user-facing setup guide is the **README's Agent access section** — Claude Code, Claude
  Desktop via `mcp-remote`, VS Code, Cursor, Gemini CLI, Codex CLI, each verified against that
  client's current docs rather than written from memory.
