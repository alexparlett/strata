# AA-03b · The Agents pane: an agent's work is its own surface, not the user's tabs

**Workstream:** Agent access · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** AA-03

## Why this exists

AA-03 shipped the founding decision of `AGENT_ACCESS_SPEC.md` §1 — *agent queries are ordinary
runs, and the investigation trail **is** the tab strip* — and using it revealed the decision was
wrong for the **MCP** frontend. Not the mechanism, which works: the reasoning.

The premise was that a person watching the tab strip sees the agent work. An MCP client is not
in the window. Claude Code is in a terminal, possibly on another desktop, and the conversation
driving it is happening somewhere the app cannot see. So an investigation of twenty queries
mutates a workspace nobody is looking at, and each of the three costs falls on the user rather
than the agent:

- **Focus.** Opening a tab moved the editor out from under whoever was typing. (AA-03 patched
  this with `SessionState::open_background`; the patch is the tell, not the fix.)
- **Pollution.** Twenty tabs the user did not open and has to close.
- **Contention.** Every open tab is validated by the window's one diagnostics driver, and each
  validation is a dry plan on the **same engine** the user's own press needs. Agent tabs make
  the user's queries slower.
- **The user's own buffer is reachable**, which is the sharpest of the four and the one with no
  patch. `list_tabs` hands an agent *every* open tab, the user's included, and a `run` on one
  calls `actions::load_sql` — so an agent can replace SQL the user is in the middle of typing.
  It is undoable and it is what "shared, last-writer-wins" literally licenses, but that rule was
  written about a tab the agent opened, not about the one the user is working in. Fixing it
  inside AA-03 means either re-introducing the ownership state that was just deleted, or making
  `list_tabs` lie about what is open. Under AA-03b it cannot arise: the agent has no handle on a
  user's tab to begin with. **This is the reason the redesign is a redesign and not a guard.**

What was right about §1 is the half about *results*: an agent step is a real snapshot the user
can page, sort, export and take over, which is what makes this better than a generic
read-only-SQL MCP server (§2). Going headless would throw that away to fix the three costs
above.

**So: keep the runs real, give them their own surface.** An `Agents` pane in the sidebar shows
what each connected agent is doing; a press promotes its SQL into the user's own tab, through
the funnel the History drawer already uses. The user's tabs stay the user's.

## Terminology (settled)

An agent's container for a sequence of runs is a **query session**, not a tab and not a
workspace. Bare "session" is taken twice — MCP's own `Mcp-Session-Id` (an agent's *connection*)
and ours (`SessionState` / `session.json`, the window's tabs and layout) — and a tool called
`open_session` reads as opening a connection. `query_session` collides with neither, says what
it is, and maps exactly onto the engine's `WsId`.

## What to build

### The vocabulary (rename + re-point)

`open_tab` → `open_query_session`, `list_tabs` → `list_query_sessions`, `close_tab` →
`close_query_session`; the `tab` parameter on `run` and `read_page` becomes `query_session`. The
tool descriptions stop promising a tab in the user's window and say what a query session is: a
place the agent's queries run in sequence, each replacing the last, visible to the user in the
Agents pane.

`TabInfo` → `QuerySessionInfo`, and its `title` goes: a query session has no name to show,
only what has run in it.

### The store — a satellite, not the session

`state::agents`, ephemeral and capped, exactly like `state::log` (P3-13) and `state::history`
(P3-14) — **nothing goes on `SessionState`**. It holds, per connected agent, its query sessions
and per session the run trail: SQL, outcome (rows · total · elapsed, or the engine's error, or a
stop), and when. Recorded by its observer, per AGENTS.md §2: the bridge driver watched the run,
so the bridge driver appends.

Runs execute on the engine directly against the query session's `WsId`. That means **no request
keeper, no diagnostics pass, no `QuerySpec` on a tab** — the three costs above disappear by
construction rather than by mitigation.

### Persistence — nothing an agent does reaches disk

Two stores, two reasons, and only one of them is a decision:

- **`session.json` — by construction.** `SessionSnapshot` is tabs, layout and geometry; an agent
  owns no tabs, so there is nothing to exclude. (Under AA-03 this was only half true:
  `QueryTab::agent` was kept out of `TabSnapshot` deliberately, but the *tab* persisted, so
  reopening a project restored tabs an agent had made and the user never asked for.)
- **`.strata/history.jsonl` — a decision, and the answer is no.** Under AA-03 an agent run rode a
  tab, so that tab's request keeper recorded it through `use_history_recording`; with no keeper
  here it happens only if chosen. It must not be: history is capped at `max_history` and
  **deduped before the cap** (P3-14), so twenty exploratory agent queries take twenty slots of
  the user's hundred and evict runs they actually made. That is this task's own pollution
  argument, moved into a different store.

So the split is the one the app already draws: **the Agents pane is ephemeral**, like
`state::log`; **history is persisted, and it is the user's**. Promotion is what reconciles them —
a promoted query the user runs is an ordinary press, so it enters history the ordinary way, which
is the honest rule: history records what *the user* ran.

### The pane

`SidebarPane::Agents` beside the catalog, on the rail's top group. Rows from the vocabulary the
app already has — the catalog's `SidebarRow` for the agent/session level, the History drawer's
card (SQL preview · figures · age) for a run. A run row is **pressable**: press loads the SQL
into the active tab, double-press loads and runs, both through `actions::load_sql` /
`actions::press_query`, so a promoted agent query is an ordinary press with its own nonce and
its own cache entry. That is the History drawer's model verbatim, which is the point — it is
already proven and already the gesture the user knows.

The header's status dot stays as it is; the pane is where "what is it *doing*" is answered, and
the dot stays "is anything connected".

### What AA-03 built that this deletes

- `views::agent_keeper` (the parked-reply settle observers) — a run no longer rides a tab's
  `QuerySpec`, so there is nothing to observe through freya-query.
- `QueryTab::agent` and the editor toolbar's `AGENT` badge — no agent tabs, nothing to badge.
- `SessionState::open_background` — added to stop an agent stealing focus; unreachable once an
  agent opens no tabs. (`open_blank` keeps focusing: that is ⌘T's own deliberate act.)
- The `load_sql`-then-`set_request` pairing in the bridge's `Run` arm.

**What survives, and would have to be rebuilt otherwise:** the service directory and the `Host`
impl, the catalog/describe projections (which must come from `ProjectState`, never DataFusion
introspection), the server and its lifecycle, the token, the policy gate, the error taxonomy.
Those are the seam; only the surface changes.

### The close confirm (T2) — settled here

Moving agent runs off tabs changes this for the better, and the two halves differ:

- **Per tab (⌘W and friends) is fixed by construction.** `TabCloser` asks
  `engine.is_running(tab.into())` for *that tab's* `WsId`, and a query session is a different
  one — so an agent's run can no longer gate closing a user's tab, which under AA-03 it could.
- **Window close / re-root / engine restart still asks, and must.** `guard.running` is the
  engine's own engine-wide flag, and AGENTS.md §2 settles that the predicate is always the
  engine's answer and never derived from mounted UI. Excluding agent work would mean a second,
  weaker predicate — and destroying a long investigation with no notice is worse than a dialog.

**What has to change is the wording, not the gate.** "Queries are running. Are you sure you want
to stop them and exit?" shown to a user who pressed Run on nothing sends them looking for a query
they never started. Which it is, is derivable — `Engine::is_running` over the tab `WsId`s versus
over the agents satellite's query-session `WsId`s — so the dialog picks its sentence:

- only agent work in flight → "An agent is running a query. Stop it and exit?"
- the user's own, or both → the existing wording

The rejected option is *don't confirm at all for agent-only work*: it reads well ("it isn't the
user's query") and costs the one property that makes the confirm trustworthy — that the app never
destroys work in flight without saying so.

**Not verified in AA-03**, and inherited here: that a running agent press trips the confirm at all
(`Engine::watch_inflight` publishes per dispatch regardless of workspace, so it should — the
acceptance said "verify, don't assume" and it was not verified).

## Open questions for the design

- **The pane wants a canvas.** CLAUDE.md treats `.dc.html` as the pixel-perfect source of truth
  and AGENTS.md §5 makes a design call the designer's. Building it from the catalog-row and
  History-card vocabulary is the honest default, but a new sidebar surface is a design surface.
- **What identifies an agent in the pane.** MCP's `clientInfo` (name + version) arrives at
  `initialize` and is the only thing a client tells us about itself. Whether the pane groups by
  agent or shows a flat run list is a design call that follows the canvas.
- **Does a query session outlive its MCP session?** The natural rule is no — a client that
  disconnects takes its query sessions with it, and the pane keeps the *trail* the way the event
  log keeps events. Worth stating explicitly, because "the agent disconnected but its results are
  still readable" is a defensible alternative.

## Acceptance

- An agent's `run` executes, settles and appears in the Agents pane, and **no tab is opened,
  focused, or validated**.
- Pressing a run row loads its SQL into the active tab; double-press loads and runs it, as an
  ordinary press.
- `read_page` still pages the run's snapshot, and a newer run in that query session still
  answers "the result was replaced".
- The user's tab strip is untouched by anything an agent does.
- Unit tests on the satellite (append · cap · the projection the pane renders) with no renderer,
  the way `state::log` and `state::history` are tested.

## Note for AA-06 (chat pane)

This is what makes the chat pane's story *better*, not worse. Chat lives in the window, the user
is looking at it, and "open this in a tab" is a wanted gesture there — so the chat pane can
promote into a real tab deliberately, using the same `actions::load_sql` funnel this pane uses.
The distinction that was missing in AA-03 is exactly the one between an agent that is in the
window and an agent that is not.
