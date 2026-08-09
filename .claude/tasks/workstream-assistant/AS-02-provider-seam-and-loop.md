# AS-02 · Provider seam + the loop

**Workstream:** Assistant · **Status:** ✅ · **Depends on:** AS-01

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

`genai` is pinned `=0.6.5` and every shape below was read off that source before it was built
on. Tests: 20 unit tests in the module plus `tests/assistant.rs`, which drives whole turns
against `MockHost` and a **local stub endpoint** reached through the roster's own
OpenAI-compatible kind — so the path the test exercises is one a user can configure, and no
production signature is shaped for it.

### The selection model (the part AS-03 and AS-04 build on)

**`PROVIDERS` is the one table.** Per kind: display label, `BaseUrl` policy
(`Provider` · `Editable(default)` · `Required`), `KeyUse` policy (`Env(var)` · `Anonymous` ·
`Unused`), the **effort rungs offered**, an example model for the placeholder, and the private
`genai` adapter. Settings' form and the composer footer both read it; neither restates it. A
kind added without a row fails to compile (`ProviderKind::info` is a match).

**Effort splits in two, and the split is the whole design.** *Whether the control exists* is a
property of the kind and is declared here — Ollama's list is empty, so no surface offers it and
a `Selection` carrying one is refused rather than ignored. *What a rung means for a given model*
is `genai`'s, verified at the pin: its Anthropic adapter already knows `xhigh` needs Opus 4.7+
and downgrades otherwise, its Gemini adapter already knows `gemini-3` takes a thinking *level*
where 2.5 takes a *budget*. A per-model capability table here would be a second copy of that,
stale within a release — the same argument that makes the model name free-form text. The rungs
are `Low · Medium · High · XHigh · Max`, matching `genai`'s keyword variants; `Budget(u32)`,
`Minimal` and `None` are deliberately not offered (a token budget means nothing across
providers, `Minimal` is one vendor's spelling of `Low`, and `None` is what an unset effort
already says).

**Three refusals that are `Selection`'s alone**, all answered before a socket opens: a base URL
on a kind that owns its endpoint, a key on a kind that sends none, and an effort on a kind with
no ladder. Each is a field silently ignored otherwise, which is a lie on screen.

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

`turn::run` → `Settle` (`Answered` · `Failed(String)` · `Cancelled` · `StoppedAtCap`). The
outcome is delivered twice and identically — as the last `TurnEvent` and as the return value —
never one derived from the other. `MAX_TOOL_ROUNDS` is 32.

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
**`StrataTools::agent_id()`** — the id the app itself minted for the pane — not the identity's
name: an identity is a claim any MCP client makes at `initialize`, so a name-keyed rule would let
a client make itself invisible by calling itself `strata-assistant`. Excluding it is AS-04's
wiring; the accessor and the reasoning are here. Two model-facing strings lost their "in the
Agents pane" clause so they stay true on all three transports (`open_query_session`'s doc, the
handler `instructions`).

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
- No conversation persistence — the transcript is the pane's ephemeral state.
- No RAG, no embeddings, no genai `chain`/`agent` features.

## Known, and owed elsewhere

**A cancelled run leaves a stale `Running` row in the app's own agents satellite.**
`strata_freya::agent::directory::run` sends its `AgentNotice::RunSettled` *after* awaiting the
engine, so a dropped run future sends none and `Agents::run_settled` never fires. Pre-existing —
an MCP client hanging up mid-run does the same, and AA-03c's note says such a row is reaped when
the connection ends. For the assistant the "connection" is the pane's whole mount, so it would
sit there. The fix is a drop guard around that await in `agent/directory.rs`, which is Freya-side
and out of this task's scope; noted in AS-04.
