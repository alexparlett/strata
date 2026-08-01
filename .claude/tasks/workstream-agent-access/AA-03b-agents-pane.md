# AA-03b · The Agents pane: an agent's work is its own surface, not the user's tabs

**Workstream:** Agent access · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** AA-03

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
card (SQL preview · figures · age) for a run. A run row is **pressable**: press opens its SQL in
a **new** tab through `actions::open_sql`, so a promoted agent query is an ordinary scratch tab
the user can read, edit and run.

> Written before the canvas landed, this section said "loads into the active tab, double-press
> loads and runs" — the History drawer's model verbatim. Both halves were overruled; see
> **What the canvas overruled** below.

The header's status dot stays as it is; the pane is where "what is it *doing*" is answered, and
the dot stays "is anything connected". The **rail button** grows a live-agent count.

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

## What the design settled (the three open questions, answered)

**A canvas arrived mid-task** (design bundle 42, `Strata.dc.html` `data-pane="agents"`, plus a
CHANGELOG entry that argues the placement). It confirmed the pane's structure and the vocabulary
it is built from, and overruled two guesses — both changes are in.

- **It is a pane, not a drawer tab**, and the canvas states the rule better than the task did: a
  drawer is an ephemeral log you consult, while this is a live, navigable tree of connected
  things you press *into*, which is the catalog's job description. Rail order is Catalog ·
  **Agents** · Connections, and the Agents button carries a **live-agent count** — accent, not
  the Problems badge's error tone, because it counts things happening rather than things wrong.
- **It groups by agent, then by query session**, built from vocabulary the app already has (the
  catalog's `SidebarRow` for both levels, the History drawer's card for a run). MCP's
  `clientInfo` is reachable without per-session bookkeeping — an rmcp 3 `#[tool]` method takes a
  `Peer<RoleServer>` and reads `peer_info()` — so a row reads `claude-code`, with the version in
  its tooltip and a plain `Agent` stand-in for a client that does not introduce itself. A session
  row is `Query session N` from a **per-agent monotonic ordinal**: a query session genuinely has
  no name, but a list of them needs rows a person can tell apart, and every alternative is worse
  (the handle is a uuid, the newest query repeats the card under it, a *position* renumbers the
  rest when one closes — which is the one place this diverges from the canvas's `qi + 1`, and it
  is only visible after a close). Sessions list oldest-first so those ordinals read 1, 2, 3 down
  the pane; the runs inside one list newest-first, because that is where "what is it doing now"
  is read.
- **A query session does not outlive its connection.** RAII on the per-connection service value,
  which is the only signal a transport gives: `StrataTools` carries a `Connection` whose `Drop`
  calls `Host::agent_gone`, and the window drops the agent and retires each session's engine
  workspace. A retraction for an agent nothing ever heard of removes nothing, so the schema-probe
  instance the transport builds costs a no-op rather than needing a flag to suppress it.

## What the canvas overruled

- **A promoted run opens a *new* tab** — not the active one, which the first build took from the
  History drawer. The canvas's own comment names the reason and it is the task's own argument:
  overwriting the buffer the user is working in is the precise harm this pane exists to prevent,
  and loading into the active tab puts it straight back. The History drawer is the *other* case —
  being in that tab is itself the ask. `actions::open_sql` is the funnel, composed from
  `open_blank` + `load_sql` so a promoted query is an ordinary scratch tab in every respect.
- **No double-press to run** (Alex, on top of the canvas, which still had one). Promoting is
  putting a query where the user can read it; pressing Run is their next decision, and the tab it
  lands in has a Run button an inch away. It also removes the only place this surface would have
  needed to tell one press from two.
- **No status dot on the agent row** (Alex). The canvas had one, always `--c-ok`; since only
  connected agents appear it has exactly one value, which is decoration implying a distinction
  the data does not carry — the same reasoning that left the History drawer's cards without one.
  A session row still shows `RUNNING`, which *is* a distinction.

## What it settled beyond that

- **The scoping is a type, not a check.** `StrataTools` *is* one agent: the transport's service
  factory runs once per MCP session and the value it returns is owned by that session's worker
  for its whole life, so one connection is one `AgentId`. Every session-scoped tool is scoped to
  it, and the run cache is keyed `(agent, root, session)` — the agent is *in* the key rather than
  compared against it, because a key is a check that cannot be forgotten. A handle an agent does
  not own answers exactly as one that never existed: a distinct "that is not yours" would confirm
  the session exists.
- **Only `open_query_session` needs the MCP peer.** Identity is wanted exactly once — when a host
  first has something of this agent's to show — so every other tool is addressed by `AgentId`
  alone, and `StrataTools::open_session` is the semantic call with no peer in it. That is what
  keeps the whole vocabulary drivable in-process, which AA-06 needs and which a peer parameter on
  five tools would have quietly cost.
- **The run is dispatched by the *directory*, on the engine, and only bracketed by the window.**
  `RunStarting` before (ownership check + record, replying with the run's sequence number),
  `RunSettled` after. A settle **names its run** rather than taking the newest: an agent that
  presses on before a slow query finishes would otherwise have the older outcome stamped on the
  newer row. And it is what deletes AA-03's keepers outright — a run no longer rides a
  `QuerySpec`, so there is nothing to observe through freya-query.
- **Two channels, because one producer cannot wait.** Asks stay bounded and awaited (honest
  backpressure for a tool call); notices are unbounded and one-way, because the most important of
  them is sent from a `Drop` with nothing to await on. One serial loop drains both, **asks
  first**, so a settle can never overtake the dispatch that minted its sequence number. That
  needed `tokio`'s `macros` feature declared in `strata-freya`'s manifest — it would have
  compiled either way through `strata-core`'s `full`, and a build that only works because of what
  another crate happens to ask for is one manifest edit away from not.
- **The T2 confirm keeps its gate and changes its sentence.** Which work is in flight is derived
  by asking the engine about the tab `WsId`s and the query-session `WsId`s separately — never
  from the satellite's own record, which would be a second answer to a question the engine owns.
  *Not* confirming for agent-only work was rejected: it reads well and costs the one property
  that makes the confirm trustworthy.

## Deleted, as planned

`views::agent_keeper` (and `AgentRuns`, the parked replies it drained), `SessionState::open_background`,
and the `load_sql`-then-`set_request` pairing in the bridge's Run arm. `QueryTab::agent` and the
`AGENT` badge were already gone (AA-03 removed them once the premise underneath was condemned).

## Acceptance

- An agent's `run` executes, settles and appears in the Agents pane, and **no tab is opened,
  focused, or validated**. ✅ (structural — nothing in the run path touches `SessionState`)
- Pressing a run row opens its SQL in a **new** tab, leaving the user's own buffer untouched and
  running nothing. ✅ (`views/sidebar/agents/run.rs` tests, driven through the real renderer)
- `read_page` still pages the run's snapshot, and a newer run in that query session still
  answers "the result was replaced". ✅ (`tools.rs` tests, over a real engine)
- The user's tab strip is untouched by anything an agent does. ✅
- Unit tests on the satellite (append · cap · the projection the pane renders) with no renderer,
  the way `state::log` and `state::history` are tested. ✅ (`state/agents.rs`, 7)
- Driven live against the running app over real MCP/JSON-RPC, two clients at once: the whole
  vocabulary, cross-agent scoping (a second client sees `[]` and a handle it should not have gets
  the same not-found a made-up one does), the policy refusal, a failed run landing as a red row,
  and a disconnect. ✅
- The whole vocabulary still round-trips over the real transport. ✅
  (`tests/mcp_over_http.rs`, renamed tools and all)

## Left for a later pass

- **Not verified by hand**: that a running agent press trips the window-close confirm at all
  (`Engine::watch_inflight` publishes per dispatch regardless of workspace, so it should — AA-03
  said "verify, don't assume" and it was not verified, and this task did not either).
- **No control reaches into an agent's work from the pane** — no close-session, no cancel-run.
  Deliberate: those are the agent's own, and a control that reached into somebody else's work
  would be this task's argument pointed backwards. If it is wanted, it is a design call first.

## Note on the design bundle

The canvas is bundle **42** (`.claude/design-handoff/parquet-viewer-design-concept 42/`), which
also carries the flattened Export/Configure option groups — unrelated to this task and **not
built**. Bundle 39 is still in the worktree and is stale for this surface.

## Note for AA-06 (chat pane)

This is what makes the chat pane's story *better*, not worse. Chat lives in the window, the user
is looking at it, and "open this in a tab" is a wanted gesture there — so the chat pane can
promote into a real tab deliberately, using the same `actions::load_sql` funnel this pane uses.
The distinction that was missing in AA-03 is exactly the one between an agent that is in the
window and an agent that is not. Concretely: it holds a `StrataTools` of its own
(`StrataTools::new`), introduces itself through `open_session` with an `AgentIdentity` of its
own making, and needs no MCP peer anywhere.
