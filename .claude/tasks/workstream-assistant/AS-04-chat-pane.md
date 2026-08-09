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
- **Step cards.** A tool round renders as a compact card: `run` → SQL (collapsed to
  `util::collapse_sql`'s one-liner) · row count · elapsed · a press that **promotes** through
  `actions::open_sql` (the same funnel the Agents pane uses — new tab, focused, ordinary
  editable text). Small results may render inline as a mini-table from the run's own page
  (never a second results pipeline); anything bigger is the promote press. Non-run tools
  (describe, validate…) get a one-line card, expandable to the JSON.
- **@-mentions.** `@` in the composer opens a picker over the catalog **from the store**
  (tables · views · saved queries — the catalog is the `ProjectState` store, never a query).
  A mention pins that object's `describe_table` result into the turn's context via the AS-01
  facade, and renders as a chip in the sent message.
- **Honest degradation.** Unconfigured (AS-02's typed error): the pane states exactly what is
  missing and links Settings ▸ Assistant (`form::reveal` addressing if a row anchor exists).
  Never a dead send button.
- **The assistant in the Agents pane.** Its sessions already appear there (it is one more
  agent to the machinery below) — that is correct, costs nothing, and needs no special-casing;
  the transcript is simply the richer view of the same runs.
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
- Streaming renders incrementally; stop mid-stream leaves a truthful cancelled turn; stop
  mid-run leaves no run in flight (the Agents pane agrees).
- Unconfigured state names the missing field and reaches Settings; configuring and returning
  makes the same composer live without a restart.
- The selector round-trips: switching entry or model mid-conversation changes the next send
  (observable in AS-02's selection); effort appears only for kinds that have one; deleting
  the picked entry in Settings leaves the footer in its honest degraded state, not a crash
  and not a silent fallback.
- Pane open/collapse and width survive restart via the layout channels like every panel.
