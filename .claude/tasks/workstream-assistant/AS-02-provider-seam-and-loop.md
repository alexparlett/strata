# AS-02 · Provider seam + the loop

**Workstream:** Assistant · **Status:** ✅ · **Depends on:** AS-01

## Owed to AS-04 (2026-08-11)

**A reasoning event.** The design canvas draws a collapsible "Thought for 4s" above a reasoning
model's answer, and the pane cannot build it: the stream loop matches `ChatStreamEvent::Chunk` and
`End` and lets reasoning + thought-signature chunks ride the captured content into the next
request (`capture_reasoning_content`), so nothing about the model's thinking reaches
`TurnEvent`. Building the control from here would mean inventing the fact.

The shape is one variant and one arm — a `TurnEvent::Reasoning(String)` emitted from the arm that
currently falls through, deltas appended the way `Delta` already is. What it must **not** do is
change what rides into the next request: the capture is what keeps a thought signature valid, and
emitting a copy is additive to it. `state::chat` grows a `Block::Thought` beside `Prose` when the
event exists, and the transcript renders it collapsed.


## As built

`crates/strata-agent/src/assistant/`, Freya-free like the rest of the crate — four modules and
a prompt:

| File | What it owns |
|---|---|
| `provider.rs` | The five provider kinds in **one table**, [`Selection`], `SelectionError`, and the single site a `genai` client is built |
| `turn.rs` | The loop, the `TurnEvent` stream, `Conversation`, cancel |
| `dispatch.rs` | Name → method for the ten tools, plus `Scope` and the step card's `Facts` |
| `offer.rs` | `offer_sql`: the assistant's own eleventh tool, how it hands the user a statement to run |
| `system.md` | The system prompt, `include_str!`'d |
| `mod.rs` | `Assistant` (the runtime) and `Running` (a turn in flight) |

`genai` is pinned `=0.7.0-beta.18` and every shape below was read off that source before it was
built on. The pin moved from `0.6.5` deliberately: 0.6.5 does not know the Claude 5 family
(`claude-sonnet-5`, Fable, Mythos and opus 4.7+ all fall to its legacy thinking-budget path) and
it **ignores request-level cache control for Anthropic outright**, so the prompt cache the rest
of this design is arranged around could not be turned on at all. A beta is a deliberate pin, not
a drift — the exact `=` version is the whole point. Tests: 20 unit tests in the module plus `tests/assistant.rs`, which drives whole turns
against `MockHost` and a **local stub endpoint** reached through the roster's own
OpenAI-compatible kind — so the path the test exercises is one a user can configure, and no
production signature is shaped for it.

### The selection model (the part AS-03 and AS-04 build on)

**`PROVIDERS` is the one table.** Per kind: display label, `BaseUrl` policy
(`Provider` · `Editable(default)` · `Required`), `KeyUse` policy (`Env(var)` · `Anonymous` ·
`Unused`), the **effort rule** (`Efforts`), an example model for the placeholder, and the private
`genai` adapter. Settings' form and the composer footer both read it; neither restates it. A
kind added without a row fails to compile (`ProviderKind::info` is a match).

**Effort is offered per model *and* per rung, and the split is the whole design.** The kind's
`Efforts` rule answers against the model in hand (`Never` for Ollama, `Always` for the
compatible endpoint, `Only(&[Rungs])` elsewhere) — because reasoning is a model capability, not
a provider one, and a per-kind answer is wrong in both directions: it hides
`claude-opus-4-5`'s working control and offers `claude-sonnet-4-5` one that breaks the turn. A
`Selection` carrying a rung the model does not offer is refused, naming the model.

The answer is a **set of rungs** rather than a yes/no, because `genai` resolves the vendors'
disagreement about the top of the ladder silently: it clamps `Max` to `"high"` for Anthropic and
Gemini, and passes it through verbatim for OpenAI, which has no `max` at all. Offering the two
top rungs where they do not land would put a label on the footer that was not what got sent —
the same "a field silently ignored is a lie on screen" the base URL and the key are refused for.
So Anthropic's newest models get all five, and everything else gets `Low · Medium · High`.

**Anthropic's list is narrowed by a second rule: a rung must not turn thinking *on*.** genai
maps an effort to `output_config.effort`, which for a model supporting *adaptive* thinking with
it off by default is what enables thinking — after which the model answers with a thinking block
genai's Anthropic streamer captures but never puts back, and Anthropic rejects the next tool
round. So `claude-opus-4-6`, `-4-7`, `-4-8` and `claude-sonnet-4-6` are excluded: the control
would work once and then fail every turn that calls a tool. What is left is safe for opposite
reasons — `claude-opus-4-5` takes an effort and has no adaptive thinking to enable; Sonnet 5,
Opus 5, Fable and Mythos think already, so a rung changes depth and not kind.

`Only` is **default-closed**, which is the safety argument for keeping name lists at all: they
will fall behind what the providers ship, and falling behind has to cost a knob the user cannot
reach yet — an omission they can report — rather than a menu whose settings the provider
refuses. It is not the *complete* argument, because `contains` also over-matches, which is what
`Rungs::except` is for: `gpt-5-chat-latest` contains `gpt-5` and is the non-reasoning chat
model, and `o1-mini`/`o1-preview` predate `reasoning_effort` entirely.

The rungs are `Low · Medium · High · XHigh · Max`, matching `genai`'s keyword variants;
`Budget(u32)`, `Minimal` and `None` are deliberately not offered (a token budget means nothing
across providers, `Minimal` is one vendor's spelling of `Low`, and `None` is what an unset
effort already says).

**Four refusals that are `Selection`'s alone**, all answered before a socket opens: a base URL
on a kind that owns its endpoint, a key on a kind that sends none, an effort the **model** does
not offer, and a model whose own name ends in a reasoning keyword. That last one is
`ModelReadsAsEffort`: with no explicit effort set, the Anthropic and OpenAI adapters parse a
trailing `-<keyword>` off the name and send the *prefix* as the model, so `qwen3-max` on a
compatible endpoint quietly queries `qwen3`. The keyword list is `genai`'s own
(`ReasoningEffort::from_model_name`, asked rather than copied) so the guard cannot fall out of
step with the parse it guards. Each of the four is a field silently ignored otherwise, which is
a lie on screen.

**The request-level cache breakpoint is Anthropic's alone.** genai reads the same
`with_cache_control` for the OpenAI family as `prompt_cache_retention: "in_memory"` on the
request body — a field an arbitrary compatible endpoint may well 400 on, for a caching mode
nobody asked for — and drops it on Gemini and Ollama. `with_capture_reasoning_content` is
likewise conditional, on the model having rungs at all: on Gemini it also turns on
`includeThoughts`, which a model with no thinking config refuses. "Does this model reason" is a
question the table already answers, so both read it rather than growing a second list.

**`Provider::check_base_url` is the one copy of the URL rule** (`Provider::check_address`'s
precedent), called by client construction *and* available to AS-03's form. Its load-bearing
half is the **trailing slash**: every adapter joins its path onto the base — Ollama by
`format!("{base}api/chat")`, the OpenAI family through `Url::join` — so `http://host/v1` reaches
`http://host/chat/completions` and `http://localhost:11434` reaches `…11434api/chat`, both as a
connection error naming a URL the user never typed.

**The OpenAI-compatible kind has no environment fallback, and that is a safety property.**
`genai`'s default auth for its OpenAI adapter is `OPENAI_API_KEY`, and the compatible kind's
endpoint is whatever host the user typed — so falling back would post their OpenAI key to it.
`KeyUse::Anonymous` sends an empty bearer instead, which local endpoints ignore and real ones
answer 401 to, in their own words.

**One kind is two adapters.** OpenAI's newer models speak the Responses API and the rest chat
completions; which is which is `genai`'s knowledge, so it is asked (`AdapterKind::from_model`)
and its answer taken **only if it stayed in the family** — `from_model` falls back to Ollama for
an unrecognized name, and a key-bearing provider silently rerouted to localhost is the worst
kind of wrong. Everything else is the table's adapter, passed as an explicit `ModelSpec::Iden`
so nothing is ever inferred from a model name's spelling.

The key rides as a `strata_core::secret::Secret`, not a `String`: the caller resolves the AS-05
reference before the call (this crate stays keystore-free), and a `Debug` of a `Selection`
cannot print it.

### The loop

`turn::run` → `Settle` (`Answered` · `Truncated` · `Failed(String)` · `Cancelled` ·
`StoppedAtCap` · `Oversized`). The outcome is delivered twice and identically — as the last
`TurnEvent` and as the return value — never one derived from the other.

**Two runaway backstops, because a loop runs away two ways.** `MAX_TOOL_ROUNDS` (32) bounds how
many rounds one send may take; `MAX_TURN_RESULTS` (five results at the per-result cap) bounds
how much those rounds may bring back. The second exists because `MAX_TOOL_RESULT` bounds one
answer and says nothing about thirty of them — a model walking a wide schema calls
`describe_table` per table, each answer under the cap and the sum past any context window. And a
`Conversation` cannot be trimmed, so the turn that overran does not just fail: every later send
is too large as well. The budget is checked *before* each call, so the answer that would overrun
is never fetched, and the calls that will not run are still answered to the model — same reason
a cancel answers them.

**A bounded tool result is still a JSON document.** Slicing an over-cap answer mid-object hands
the model a half-brace to guess at, and a model that cannot parse the answer re-runs the call,
which produces the same oversized answer. So the cut result is replaced by an object that *says*
it was cut and carries the head as a string field, with the recovery named in the vocabulary's
own terms (`read_page`).

**Two transcripts, and they are not the same list.** `Conversation` is the *model's* memory
(provider vocabulary, tool calls and results, opaque outside the crate); the pane's transcript
is built from `TurnEvent`s. Neither substitutes for the other — a person cannot read a page of
tool JSON and a model cannot read a step card — and keeping them apart is what keeps `genai` out
of the frontend.

**Pinned context rides the user's message, not the system prompt.** That was the first shape and
it invalidates the provider's prompt cache on every turn. On the message it also reads truer: the
transcript records what the user was pointing at *when they asked*. The system prompt is
byte-identical on every send.

**`TurnEvent::ToolCall` is emitted from the settled call, not from `ToolCallChunk`.** genai's
streamers emit a chunk per accumulation step with partial arguments; a card that showed half a
JSON object and rewrote it is worse than one that appears a moment later, just before the tool
runs.

**Cancel is a drop, because a drop is already the abort.** Verified against AA-03c rather than
reimplemented: `Engine`'s `DispatchGuard` is armed for exactly the await a dropped caller
abandons, and aborts the detached task and retires what it materialized. What a cancel must not
leave is a conversation the next send cannot use — an assistant message with tool calls and no
results is a request every provider rejects — so a cancel **answers the outstanding calls**
with "the user stopped this turn" and only then settles.

**A stop keeps the half-answer the user already read.** The captured content a turn normally
commits from only exists at the stream's `End`, which a cancel never reaches, so the deltas are
also accumulated as they are forwarded and pushed as the assistant's message when a stop lands
mid-stream. Without it a stopped turn committed nothing at all — `Staged::commit` drops a turn
whose only message is the user's question — and the next send carried on from before a question
whose answer is still on screen. The step card's stop reason is the engine's own `CANCELLED`,
not a sentence typed at the call site, because every other value in `Facts` came off
`RunResult::Stopped`.

**Errors pass through.** A tool error goes back to the model as that tool's result, verbatim
(the design working). Only a provider or transport fault fails the turn, with the provider's own
message.

### `offer_sql` — the executable card

The assistant's **eleventh tool**, in `offer.rs`, dispatched by the loop and **never registered
on the router**: `tools/list` is unchanged and no MCP client is offered a tool it has no
transcript to use. The loop offers `manifest() + offer::spec()`. That is one vocabulary plus one
presentation tool on the transport that has a presentation — nothing in it touches `Host`, the
engine or a query session.

A **tagged markdown fence (` ```sql run `) was built first and withdrawn.** A fence is taught
only by a paragraph of system prompt, and prompt-taught formatting is followed unevenly — least
reliably by exactly the small local models the Ollama entry exists for. A tool is taught by its
*schema*, which is the channel a model follows best and cannot get syntactically wrong. It also
buys what a fence structurally cannot: **the statement is checked before the card appears**.

That check is `validate` — lints, managed-DDL policy, dry plan — and it is the **editor's**
policy, not the agent's, which is the point of the tool: a card is executed by the user, in
their editor, under their capability. So the assistant may offer a `CREATE TABLE` it is itself
refused, which is exactly the handover the system prompt asks for. Errors only; a warning is
something the user can read on the card. A statement that does not check out produces **no
card** — the model is told why and offers a corrected one.

`OfferParams` takes `sql` and nothing else: every other tool takes a `project` because an MCP
client can be looking at any of several windows, and a chat pane belongs to exactly one. The
`Scope` supplies it.

### The assistant is not in the Agents pane

Settled 2026-08-09, overturning AS-01's note. That pane answers "which external clients are
connected to my project right now"; the assistant is part of the app, not connected to it, and
its runs show as step cards in the transcript instead. The discriminator is
**`Agent::in_app`** — a mark `StrataTools::in_app` mints and every `Host` receives on the call
that opens a session — not the identity's name: an identity is a claim any MCP client makes at
`initialize`, so a name-keyed rule would let a client make itself invisible by calling itself
`strata-assistant`. The exclusion is **enforced here, in the core**, not owed to AS-04:
`Agents::agents` filters, `Agents::held` is the unfiltered view `list_query_sessions` and the
log's attribution read, and `Agents::sessions_of` is the same line for the close confirm's
`Whose::Assistant` arm. Two model-facing strings lost their "in the
Agents pane" clause so they stay true on all three transports (`open_query_session`'s doc, the
handler `instructions`).

### The system prompt

`system.md`, `include_str!`'d — prose that will be edited by people reading it as prose, and
byte-identical on every send so a provider's prompt cache holds across a conversation (pinned
context rides the user's message instead). It authors what Strata is, the IDE register for
user-facing prose (AGENTS.md §3 — the assistant's words render in the transcript), the tool
guidance, and two rules the placement survey settled:

- **No number in prose without a run behind it.** Every claim about the data comes from a tool
  result in the conversation, and when a number matters the prompt asks which query produced
  it — because the pane renders that round as a step card the user can promote, edit and rerun.
  A wrong answer that shows its SQL is recoverable; a wrong answer in bare prose is not.
- **A write intent is drafted, never executed.** CTAS, `COPY`, view DDL: the answer is the
  statement, handed over through `offer_sql` so the user runs it under their own capability.
  The refusal the tool returns is the design working, not an obstacle to route around.

Both are prompt rules over a structural guarantee rather than instead of one: the router
refuses a write before dispatch whatever the prompt says.

### The runtime

`Assistant` owns a private two-worker Tokio runtime — the Engine pattern, for the Engine's
reason. Deliberately **not** `AgentServer`'s: that runtime exists only while agent access is
switched on in Settings, and the chat pane must not stop working because the user turned the MCP
server off.

## What AS-04 gets

`Assistant::send(tools, selection, scope, conversation, ask) -> Running`, and `Running` gives
`next()` (events), `stop()` and `settle()`. Dropping a `Running` cancels its turn — a turn
nobody is listening to spends tokens and engine time on an answer with nowhere to land.

Events: `Started`, `Delta(String)` (prose, markdown, ordinary code blocks included),
`Runnable(String)` (an offered statement — an executable card, never also a step card),
`ToolCall`, `ToolSettled { failed, facts }`, `Settled(Settle)`. `Facts` is what a step card
shows: SQL · query session · exact rows · the engine's own `elapsed_ms` · a `stopped` reason.
Nothing is measured twice.

## What is NOT this task

- No Freya, no Settings UI (AS-03), no transcript rendering (AS-04).
- No conversation persistence — the transcript is the pane's state. (**AS-07** later stored it,
  and `Conversation` grew the `to_json`/`from_json` pair for exactly that; nothing else here
  moved, and the loop still takes its memory by handle.)
- No RAG, no embeddings, no genai `chain`/`agent` features.

## Corrected by review (2026-08-09) — do not re-introduce

A max-effort adversarial pass over the first cut found fifteen correctness defects. What each
one taught is recorded here because most of them are shapes that read as obviously fine:

- **A turn stages its messages and commits them once.** Written first as a push per message,
  which was three defects at once: a cancelled turn's cleanup could land *after* a newer turn
  had written (leaving tool calls with a user message between them and their results — the
  request every provider rejects), a turn that failed before the model said anything left the
  user's question dangling with no reply, and the whole history was deep-cloned under the lock
  every round. `Staged` removes all three, and it is also what makes `shutdown_background`
  safe: a task dropped mid-flight has committed a whole block or nothing.
- **Anthropic's effort ladder was emptied, then re-enabled entire, and both were wrong.** The
  claim was that setting an effort breaks the next tool round because genai never returns the
  thinking block. The mechanism is real (its streamer hardcodes
  `captured_thought_signatures: None` and its serializer drops the parts, in 0.7 as in 0.6.5),
  but the first version applied it to every Claude model and the correction then withdrew it
  from every Claude model — each time asserting one answer for a question that is per model.
  The rule that survives verification: it bites exactly where our effort *turns thinking on*,
  which is a model that supports adaptive thinking and has it off by default —
  `claude-opus-4-6`, `-4-7`, `-4-8`, `claude-sonnet-4-6`. It does not bite where thinking is
  already on (Sonnet 5, Opus 5, Fable, Mythos: a fatal round-trip requirement would break their
  tool use with or without us) nor where there is no thinking to enable (`claude-opus-4-5`).
  Kept in full because the reusable part is the shape of *both* errors: a verified mechanism
  carrying an unverified consequence, and then an over-correction that threw out the mechanism
  along with the consequence.
- **`capture_reasoning_content` is not a display option.** It is what makes genai's OpenAI
  Responses adapter request and record the encrypted reasoning item, without which a gpt-5
  tool loop re-sends its calls with the reasoning missing in front of them. Gemini captures
  signatures unconditionally, which is exactly why the gap was invisible from that side.
- **One tool message per round, not one per call.** genai's Anthropic adapter emits a `user`
  entry per Tool-role message with no merging (its Gemini adapter merges explicitly), so N
  parallel calls answered as N messages leave the message after the assistant turn answering
  only the first. `From<Vec<ToolResponse>>` is genai's own shape for it.
- **The assistant message is assembled here, not by `into_assistant_message_for_tool_use`**,
  which keeps signatures and calls and silently drops every text part — so the model's own
  narration reached the pane and vanished from its memory.
- **An empty reply is a failure, not an empty message.** Recording `assistant("")` is refused
  by Anthropic on every later send, and `Conversation` cannot be edited, so it killed the
  conversation permanently while the turn reported success.
- **`captured_stop_reason` is read**: a reply cut off by the output limit settles `Truncated`,
  never `Answered`.
- **Tool results are bounded before they enter the model's memory** (`MAX_TOOL_RESULT`), with
  the cut naming `read_page`. A `run` answer carries up to `MAX_PAGE_SIZE` rows and is re-sent
  every round and every turn; one large call exhausted the context window with no recovery.
- **The scope is a boundary, not a default.** `scoped` overwrote nothing, so a model that named
  a *different* open project was served against it — a pane could reach every other window's
  data, and the step card carries no project to say which. It overwrites now, and
  `list_projects` answers with the pane's project alone.
- **`check_base_url` normalizes the parsed URL's path**, not the raw text: a base carrying an
  api-version query got the slash inside the query, and genai's join then ate the path segment.
- **A blank base URL reads as absent**, or a cleared box is refused with "Clear it in Settings".
- **`offer_sql` claims what `validate` delivers and no more.** It is not a parse guarantee:
  validate deliberately stays silent on an incomplete trailing statement, on unresolved columns
  where the resolver's scope is incomplete, and on a `;`-separated batch. The doc says so; the
  residual is below.
- Smaller, all fixed: `offer_sql`'s schema goes through rmcp's own normalizer (it was leaking
  `"title": "OfferParams"`), the bad-arguments sentence is shared with the ten, `tokio`'s `time`
  feature is declared rather than inherited, `Running` uses `tokio_util`'s `DropGuard`, a
  cancelled `JoinError` settles as `Cancelled`, pinned context is fenced as attached data rather
  than run together with the question, an empty ask is refused before a socket opens,
  `ProviderKind::all()` reads the table instead of being a second list, and the env key check
  happens before a socket opens rather than arriving as a 401.

The test suite grew with each: the stub now records the path and the `Authorization` header,
fails an unscripted request instead of answering an empty reply, and can serve a 4xx, and the
cancel test waits on `Engine::is_running` rather than a fixed sleep.

## Corrected by a second review (2026-08-10) — do not re-introduce

A second max-effort pass, mostly over what the two commits above had just changed:

- **An effort rule is a set of rungs, not a boolean** — the per-rung half of "offered per model"
  was never built, so every model with a control was offered all five and `genai` quietly
  clamped or forwarded whatever did not fit. See the selection section.
- **A model name that reads as an effort is refused, not silently rewritten** — the
  `ModelReadsAsEffort` arm, above.
- **An option is scoped to the adapter its reasoning is about.** `with_cache_control` was set
  for all five kinds under a comment that was true of one; `with_capture_reasoning_content` was
  set flat, which sends Gemini `includeThoughts` for models that refuse it.
- **`contains` over-matches, and default-closed does nothing about that.** `Rungs::except`.
- **A stop keeps the prose the user already read**, and the step card's stop reason is the
  engine's word rather than one typed at the call site.
- **A step card shows the arguments that will run**, not the ones the model sent: the scope
  overwrites `project`, so a card quoting the request could name a project the run never
  touched. `dispatch::scoped` is a fixed point, which is what lets the card and the call share
  one value.
- **A tool result is bounded *per turn* as well as per result**, and a bounded result is still
  parseable JSON — both above.
- **A provider's error is bounded before it reaches the transcript.** A gateway 5xx is an HTML
  page, and genai carries it into its error whole.
- **The close confirm and the event log name the assistant as itself.** "An agent is running a
  query" points at a pane that says nobody is connected, and the assistant never dialled in so
  it cannot disconnect. `Whose::Assistant` and `Agents::sessions_of`.
- **A fixture that cannot produce the state under test is not a test.** Both calls in the
  parallel-tool-call fixture carried `tool_calls` index 0, which is one call streamed in two
  parts — so the test asserted a property of a single call. The index is a parameter now and
  the test checks two calls arrived before checking how they were answered.
- Smaller, all fixed: the pool restates *all five* of genai's client knobs rather than three,
  `StrataTools::agent_id` is gone (dead since the mark replaced it) along with the three docs
  that still named it as the pane's discriminator, an env key of whitespace reads as absent,
  and `serde_json`'s items are imported rather than qualified inline (AGENTS.md §1).

## Known, and owed elsewhere

**`offer_sql` is not a parse gate, and closing that needs a seam the vocabulary does not have.**
`Engine::validate` is right for a live editor buffer and therefore silent on three things that
reach a card: an incomplete trailing statement, an unresolved column where the resolver's scope
is incomplete, and a `;`-separated batch (which Run then refuses whole). The sound check is
`sql::classify_one`, which is the engine's and sits behind a `SessionContext` no tool holds.
Adding a non-tool method to `StrataTools` for it would blunt AS-01's "the public methods *are*
the ten tools", so it is recorded rather than done. The user-visible cost today is a card whose
Run press fails in the editor's own words.

**`MAX_TOOL_RESULT`'s cut used to name a recovery three tools did not have — AA-07 closed it
(built 2026-08-11).** The three list-shaped tools now bound their own answers with stated
totals (`describe.rs`'s walk, `wire::functions_result` / `tables_result`), `turn::bounded`
names each cut tool's own recovery per tool, and the `run` ceiling landed as
`dispatch::MAX_RUN_ROWS` — a documented const over the same page-size resolution `run` uses,
**not** the `Scope` field this note originally sketched: nothing sets a per-conversation
ceiling today, and a field nobody sets is dead configuration (promote it if AS-04 ever needs
per-pane ceilings). The measurements and the settled design live in
`workstream-agent-access/AA-07-bounded-answers.md`.

**A cancelled run leaves a stale `Running` row in the app's own agents satellite.**
`strata_freya::agent::directory::run` sends its `AgentNotice::RunSettled` *after* awaiting the
engine, so a dropped run future sends none and `Agents::run_settled` never fires. Pre-existing —
an MCP client hanging up mid-run does the same, and AA-03c's note says such a row is reaped when
the connection ends. For the assistant the "connection" is the pane's whole mount, so it would
sit there. The fix is a drop guard around that await in `agent/directory.rs`, which is Freya-side
and out of this task's scope; noted in AS-04.
