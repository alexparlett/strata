# AS-04 · The chat pane

**Workstream:** Assistant · **Status:** ⬜ · **Depends on:** AS-02, AS-03

## Goal

The Freya surface: a right-side pane in the project window (the Snowflake Copilot / DataGrip
position), toggled from the activity rail, where the conversation streams and the assistant's
runs show as step cards the user can promote. Everything below it exists by now; this task is
UI + wiring, on the state rules the app already lives by.

## What to build

- **Placement + layout.** Right-side pane; toggle rides the activity rail alongside Catalog ·
  Agents · Connections. Structure on `Chan::Layout`, sizes on `Chan::LayoutSize`
  (unsubscribed), panels keyed with fixed `.order()` — exactly the drawer/sidebar pattern.
- **Transcript state.** A per-window satellite in the image of `state::agents` /
  `state::log` — ephemeral, capped, **never** `SessionState` (nothing reaches
  `session.json`), under its own granular channels so a streaming delta wakes the transcript
  and nothing else. The AS-02 event channel is the sole producer; the pane's driver drains it
  and folds into this state (a log is recorded by its observer).
- **The conversation.** User input at the bottom (an `Input`-based composer — remember a
  focused `Input` owns the keyboard: chords in `on_pre_key_down`); send becomes **stop** while
  a turn streams (cancel = AS-02's token; a cancelled turn stays in the transcript marked as
  such — truthful, not erased). Assistant prose streams in as deltas.
- **The selector.** The composer's footer holds the conversation's pick: **entry · model ·
  effort** (the IntelliJ AI-chat footer shape — "Junie · GPT-5.5 · High effort"). Entries
  come from the AS-03 roster in config; model defaults to the entry's own and is overridable
  free-form; effort renders **only** for kinds the provider table says have one (absent, not
  disabled). The pick is per-conversation runtime state on the transcript satellite — never
  config, never `SessionState` — seeded from the roster's default entry, read at send time
  into AS-02's per-send selection. Changing it mid-conversation affects the next send and
  nothing already settled. A pick whose entry has since been deleted degrades honestly:
  the footer says so and offers the default, never a silent re-point.
- **Rendering.** Evaluate the fork's `freya-markdown` for the transcript **first**
  (standard-components-first, one level up); build bespoke only for what it will not carry,
  and then prefer extending it in the fork (§6) over app-side workarounds.
- **Step cards.** A tool round renders as a compact card, built from AS-02's
  `TurnEvent::ToolCall` / `ToolSettled` pair and the `Facts` the second carries — **SQL ·
  query session · exact rows · `elapsed_ms` · a `stopped` reason** — none of which the pane
  re-derives or re-measures (`elapsed_ms` is the engine's own). `run` → SQL collapsed to
  `util::collapse_sql`'s one-liner, row count, elapsed, and a press that **promotes** through
  `actions::open_sql` (the same funnel the Agents pane uses — new tab, focused, ordinary
  editable text). A `stopped` run is dressed as a status, never as a failure (`failed` on the
  event is the fault flag, and a stop does not set it). Small results may render inline as a
  mini-table from the run's own page (never a second results pipeline); anything bigger is the
  promote press. Non-run tools (describe, validate…) get a one-line card, expandable to the
  JSON.
- **Executable cards are a different thing from step cards.** `TurnEvent::Runnable(String)`
  arrives when the assistant calls **`offer_sql`** — its own eleventh tool (AS-02, `offer.rs`),
  not on the MCP router — to hand the user a statement to execute. It renders as a card with a
  **Run** press and an **Open in editor** press (`actions::open_sql` again), and it deliberately
  produces **no step card**: an offer is not a step, and a card describing the tool call beside
  the executable card would be the same thing said twice. SQL the assistant is merely
  *explaining* never arrives this way — it stays in `Delta` as an ordinary markdown code block,
  which is the distinction the tool exists to make. The statement has already been validated
  against the catalog and the **editor's** policy before the event is sent, so a card's Run
  press cannot be offering something that will not parse — and note it may legitimately carry a
  write statement the assistant itself is refused, because the user runs it under their own
  capability.
- **@-mentions.** `@` in the composer opens a picker over the catalog **from the store**
  (tables · views · saved queries — the catalog is the `ProjectState` store, never a query).
  A mention pins that object's `describe_table` result into the turn's context via the AS-01
  facade, and renders as a chip in the sent message.
- **Honest degradation.** Unconfigured (AS-02's typed error): the pane states exactly what is
  missing and links Settings ▸ Assistant (`form::reveal` addressing if a row anchor exists).
  Never a dead send button.
- **Keep the assistant *out* of the Agents pane** (settled 2026-08-09, overturning this task's
  earlier note that its sessions belonging there was correct). That pane answers "which external
  clients are connected to my project right now" — headless MCP clients. The assistant is part
  of the app, not connected to it, and its runs already have a richer home in the transcript;
  listing it there would put a permanent row in a pane whose whole premise is that a row means
  somebody dialled in.

  **Filter on `StrataTools::agent_id()`**, the id the app minted when it built the pane's own
  `StrataTools` — the app therefore holds it and can hand it to the window. **Not** a name
  comparison against `AgentIdentity::assistant()`: an identity is a claim a client makes at
  `initialize`, so a name-keyed rule would let any MCP client hide itself from the pane by
  calling itself `strata-assistant`. The exclusion belongs wherever the window records an
  agent (`state::agents` via `agent::directory`), not in the pane's render — a satellite that
  holds a row nothing draws is a row the eviction cap still counts.

  Everything *below* the pane is unchanged: the assistant is one more agent to `Host`, the
  policy gate and the query-session machinery, with its own `AgentId` and its own sessions.
- **A cancelled run leaves a stale `Running` row** in `state::agents` today: `agent::directory`'s
  `run` sends `AgentNotice::RunSettled` after awaiting the engine, so a dropped run future (which
  is how AS-02 cancels, and how an MCP client hanging up mid-run already behaves) sends none.
  AA-03c reaps such a row when the connection ends; the assistant's "connection" is the pane's
  whole mount, so it would sit there instead. Fix is a drop guard around that await sending the
  cancelled settle — Freya-side, so it was left out of AS-02. Needed for this task's own
  acceptance below.
- **Dress.** Existence and placement are settled; the chrome follows a canvas when the
  designer draws one (`Strata.dc.html` has none for this pane yet). Build with standard
  components + roles/tones; user-facing text in the IDE register.

## What is NOT this task

- No loop logic, no provider handling, no prompt text — all AS-02.
- No conversation persistence; closing the window is the end of the transcript (v1).
- No loosening of read-only; the assistant is the same `Host` path as every agent.

## Acceptance

- A conversation can: answer a schema question from an @-mentioned table without running SQL;
  run a query that appears as a step card, promote it into a focused tab holding the SQL;
  recover in prose from a policy refusal (the transcript shows the editor-register message).
- An `offer_sql` card runs its statement in a new tab and never appears as a step card; SQL the
  assistant only explains renders as an ordinary code block with no Run press.
- The assistant has **no row in the Agents pane**, in a window where an MCP client does.
- Streaming renders incrementally; stop mid-stream leaves a truthful cancelled turn; stop
  mid-run leaves no run in flight and no session left showing `Running`.
- Unconfigured state names the missing field and reaches Settings; configuring and returning
  makes the same composer live without a restart.
- The selector round-trips: switching entry or model mid-conversation changes the next send
  (observable in AS-02's selection); effort appears only for kinds that have one; deleting
  the picked entry in Settings leaves the footer in its honest degraded state, not a crash
  and not a silent fallback.
- Pane open/collapse and width survive restart via the layout channels like every panel.
