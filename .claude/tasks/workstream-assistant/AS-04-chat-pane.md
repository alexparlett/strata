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
  Placement was re-examined against a tab-strip alternative and confirmed 2026-08-09 — the
  README records the survey and the reasoning; the tab shape is banked as a *future* delegation
  surface, not a relocation of this one.
- **Entry at the point of friction.** The rail toggle is the general door; the gestures that
  make the pane feel native open it with context already pinned: a press on a failed run's
  error (opens with `@tab` pinned — SQL + the error), on the results toolbar ("explain this
  result" — opens with `@result` pinned), and on a catalog item's context menu ("ask about
  this table" — opens with `@table` pinned). Error-anchored help is the most convergent
  gesture in the field (DataGrip, DBeaver, Databricks, Snowflake Cortex Code, MotherDuck and
  Hex all hang it on the failure site). Each entry is one press into this same pane with a
  chip pre-filled, never a second surface.
- **Transcript state.** A per-window satellite in the image of `state::agents` /
  `state::log` — ephemeral, capped, **never** `SessionState` (nothing reaches
  `session.json`), under its own granular channels so a streaming delta wakes the transcript
  and nothing else. The AS-02 event channel is the sole producer; the pane's driver drains it
  and folds into this state (a log is recorded by its observer).
- **The conversation.** User input at the bottom (an `Input`-based composer — remember a
  focused `Input` owns the keyboard: chords in `on_pre_key_down`); send becomes **stop** while
  a turn streams (cancel = AS-02's token; a cancelled turn stays in the transcript marked as
  such — truthful, not erased). Assistant prose streams in as deltas.
- **The selector.** The composer's footer holds the conversation's pick: **provider · model ·
  effort** (the IntelliJ AI-chat footer shape — "Junie · GPT-5.5 · High effort"). Providers are
  the **enabled** ones in `Ai::providers` (`Ai::enabled()`, keyed by `ProviderKind` — AS-03 has
  no roster of named entries and no per-entry model; that shape was built and withdrawn, see its
  task file). Model and effort seed from `Ai::default_model` / `Ai::default_effort` and are
  overridable here. Effort renders **only** when `efforts(kind, model)` is non-empty — a
  question about the **model**, not the provider, so changing model within one provider can add
  or remove the control (absent, not disabled). The pick is per-conversation runtime state on the
  transcript satellite — never config, never `SessionState` — seeded from `Ai::default_provider`,
  read at send time into AS-02's per-send selection. Changing it mid-conversation affects the
  next send and nothing already settled. A pick whose provider has since been **disabled**
  degrades honestly: the footer says so and offers the default, never a silent re-point. (In
  Settings a disabled provider also loses its key, so "disabled" and "no longer usable" are one
  state rather than two the pane has to tell apart.)
- **The model is picked from what the provider reports, not typed.** A `Select` over
  `provider::list_models(kind, base_url, key)` — the same call Settings' Test makes, and the only
  thing in the system that knows what a provider actually serves. genai prescribes nothing here:
  a model name is an opaque string in the request payload, and the list is a live `GET` against
  the provider's own endpoint (`Client::all_model_names`). So the offer is the provider's answer,
  and typing a name that does not exist stops being a way to spend a turn on a 404.

  Three things fall out, all of which the build has to carry:

  - **It is network I/O, so it is not on the render thread.** `task::offload`, fetched per
    `(kind, base_url, key)` when a provider is picked (or the dropdown first opens), with the
    four states named rather than collapsed: not asked · asking · listed · failed. Cancelling is
    dropping the answer. Cache it on the transcript satellite for the window's life; a provider
    the user has already opened must not re-dial every time the footer is touched.
  - **The configured model is always in the list, even when the fetch is not.** The list endpoint
    is not the chat endpoint — a proxy or a private deployment can serve `/chat/completions` and
    no `/models` at all (genai carries hardcoded lists for Cohere and Baidu for exactly this
    reason), and an offline laptop serves neither. A strict picker over an empty answer would
    strand a setup that works, so the offered set is *reported ∪ {the current pick}*, and a
    failed fetch says which provider would not answer while leaving the seeded model selectable.
    That is the honest-degradation rule, not an escape hatch back to free text.
  - **The list is unfiltered, and stays that way in v1.** genai returns every `id` the provider
    names, so OpenAI's carries `text-embedding-3-large`, `whisper-1` and `dall-e-3` beside the
    chat models. Do **not** invent a static name filter to tidy it — that is precisely the
    prescribed-model table this design avoids, and it would hide a new chat model the day it
    ships. Picking a non-chat model fails on the first send in the provider's own words, which
    is how the rest of this surface already behaves. If it becomes a real irritation the fix is
    a capability the adapter reports, in the fork of genai or upstream, never a list here.
- **Rendering.** Evaluate the fork's `freya-markdown` for the transcript **first**
  (standard-components-first, one level up); build bespoke only for what it will not carry,
  and then prefer extending it in the fork (§6) over app-side workarounds.
- **Step cards.** A tool round renders as a compact card, built from AS-02's
  `TurnEvent::ToolCall` / `ToolSettled` pair and the `Facts` the second carries — **SQL ·
  query session · exact rows · `elapsed_ms` · a `stopped` reason** — none of which the pane
  re-derives or re-measures (`elapsed_ms` is the engine's own, and `stopped` is the engine's
  own word). `run` → SQL collapsed to `util::collapse_sql`'s one-liner, row count, elapsed,
  and the promote presses. Promotion is **two presses, Snowflake's Run/Add shape**: *Open*
  (new tab holding the SQL through `actions::open_sql` — the same funnel the Agents pane uses
  — not run) and *Open and run* (same funnel, Run pressed on arrival) — because in a data tool
  the check on the assistant's SQL is the grid updating, not a diff read (MotherDuck's Instant
  SQL lesson), and a promoted-and-run query records into history like any user press, which is
  exactly the **history = adoption** rule (README). A `stopped` run is dressed as a status,
  never as a failure (`failed` on the event is the fault flag, and a stop does not set it).
  Small results may render inline as a mini-table from the run's own page (never a second
  results pipeline); anything bigger is the promote press. Non-run tools (describe, validate…)
  get a one-line card, expandable to the JSON. Cards are **citations**: AS-02's prompt says no
  number in prose without a run behind it, and the card under the paragraph is what makes that
  auditable. The card shows the arguments the call **ran with**, which AS-02 already scopes —
  the model's own `project` is overwritten, so quoting its request could name a project the
  run never touched.
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
  capability. This is the surface the prompt's *drafted, never executed* rule lands on.
- **@-mentions.** `@` in the composer opens a picker over the catalog **from the store**
  (tables · views · saved queries — the catalog is the `ProjectState` store, never a query).
  A mention pins that object's `describe_table` result into the turn's context via the AS-01
  facade, and renders as a chip in the sent message.
- **Anchors beyond the catalog — same mechanism, no new tools.** Two more mention targets,
  both *pinned context per send* exactly like `@table`, never additions to `StrataTools` (the
  read-only gate and the vocabulary are untouched): **`@tab`** — the active tab's SQL, plus
  its last error if the run failed — and **`@result`** — the active tab's settled run
  (schema · row count · first page of rows), read under `Engine::pin_snapshot` exactly as any
  reader that outlives a run. These are the two anchors a query tool lives on and a code IDE
  cannot have; result-anchored follow-up ("why the dupes in *this* grid?") is the pane's
  highest-value interaction.
- **Honest degradation.** Unconfigured (AS-02's typed error): the pane states exactly what is
  missing and links Settings ▸ Assistant (`form::reveal` addressing if a row anchor exists).
  Never a dead send button.
- **Keep the assistant *out* of the Agents pane** (settled 2026-08-09, overturning this task's
  earlier note that its sessions belonging there was correct). That pane answers "which external
  clients are connected to my project right now" — headless MCP clients. The assistant is part
  of the app, not connected to it, and its runs already have a richer home in the transcript;
  listing it there would put a permanent row in a pane whose whole premise is that a row means
  somebody dialled in.

  **Done, in the core — nothing to wire here.** `StrataTools::in_app` marks the pane's own
  value, the mark rides `Agent::in_app` to every `Host` on the call that opens a session, and
  `Agents::agents` (the pane's listing, and `len` behind the rail badge) filters on it.
  `Agents::held` is the unfiltered iterator `list_query_sessions` answers from and the event log
  attributes from; `Agents::sessions_of` is the same line drawn for the close confirm, which
  says "the assistant is running a query" rather than "an agent" (`Whose::Assistant`). The
  ownership check and the session cap live inside the satellite and read the field directly.
  The assistant is *held* like any other agent and only *listed* differently. **Not** a name
  comparison against `AgentIdentity::assistant()`: an identity is a claim a client makes at
  `initialize`, so any MCP client could have hidden itself with it. Build the pane against
  `StrataTools::in_app(host)` and the rule holds by construction.

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
- **No in-place edits to the user's buffer, ever in v1.** A fix or rewrite arrives as a
  promoted tab; the buffer is often the user's only record of how a number was reached, and
  text moving under the cursor destroys provenance. Field practice does allow in-place edits
  — always behind a diff gate (DataGrip apply-with-diff, Databricks cell diff, Hex pending
  changes) — so if the gesture ever comes it is diff-gated and a task of its own, never a
  silent write.

## Acceptance

- A conversation can: answer a schema question from an @-mentioned table without running SQL;
  run a query that appears as a step card, promote it into a focused tab holding the SQL;
  recover in prose from a policy refusal (the transcript shows the editor-register message).
- An `offer_sql` card runs its statement in a new tab and never appears as a step card; SQL the
  assistant only explains renders as an ordinary code block with no Run press.
- The assistant has **no row in the Agents pane**, in a window where an MCP client does.
- The three friction entries (failed run · results toolbar · catalog context menu) each open
  the pane with the right anchor pinned, visible as a chip before the user types.
- *Open* and *Open and run* both land in a focused tab; the run variant's query records into
  history (adoption), while the assistant's own runs never do.
- Streaming renders incrementally; stop mid-stream leaves a truthful cancelled turn; stop
  mid-run leaves no run in flight and no session left showing `Running`.
- Unconfigured state names the missing field and reaches Settings; configuring and returning
  makes the same composer live without a restart.
- The selector round-trips: switching provider or model mid-conversation changes the next send
  (observable in AS-02's selection); effort appears only for models that have rungs; disabling
  the picked provider in Settings leaves the footer in its honest degraded state, not a crash
  and not a silent fallback.
- The model list is the provider's own: picking a provider offers what it reports, the pane
  does not freeze while it is asked, and a provider whose `/models` cannot be reached still
  leaves the configured model selectable and says why the rest is missing.
- Pane open/collapse and width survive restart via the layout channels like every panel.
