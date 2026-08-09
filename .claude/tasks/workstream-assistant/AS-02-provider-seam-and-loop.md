# AS-02 · Provider seam + the loop

**Workstream:** Assistant · **Status:** ⬜ · **Depends on:** AS-01

## Goal

The agentic loop, in `strata-agent` (new module, e.g. `strata_agent::assistant`), Freya-free:
take a transcript + pinned context + the AS-01 manifest, stream one assistant turn from the
configured provider via **`genai`**, execute any tool calls through the AS-01 facade, and
repeat until the model answers in prose. Streaming out, cancel in, errors verbatim.

## Why `genai`, in one line each (full record: this workstream's README)

Provider abstraction, not agent framework — the loop, transcript and vocabulary are ours;
tools + streaming + tool-call chunks are in its one `ChatStreamEvent` surface; resolvers make
config-owned keys and custom endpoints (Ollama, OpenAI-compat) first-class. **Pin the version;
verify the API from its source before building** — the shapes below are from docs and must be
confirmed: `ChatRequest` (messages + tools), `MessageContent` parts (`ToolCall`,
`ToolResponse`), `ChatStreamEvent` (text chunks, `ToolChunk`, `StreamEnd`), `AuthResolver`,
`ServiceTargetResolver`, `ChatOptions`.

## What to build

1. **The selection, as plain data, per send.** A struct this module owns, handed in with
   every send — the app resolves it from the conversation's pick (AS-04) over the roster
   entry (AS-03) at send time; the loop holds no global config and reads no Settings.
   Fields: provider kind (Anthropic · OpenAI · Gemini · Ollama · OpenAI-compatible), model
   name, optional base URL (required for the last two), optional API key **as a resolved
   string** — the *caller* resolves the AS-05 reference to a key before the call, so this
   crate stays keystore-free exactly as it stays Freya-free — and an optional **effort**.
   Effort is not a portable knob (Anthropic spells it as a thinking budget, OpenAI as
   `reasoning_effort`, Ollama not at all): the provider table (AS-03's one table, homed in
   this module) declares per kind whether it exists and how it maps, and **verify `genai`'s
   `ChatOptions` coverage of it at the pinned version** before promising it anywhere. A kind
   without one simply has no field, end to end. Construction of the `genai` client happens in
   **one** place from this struct — `AuthResolver` answers keys from the struct with the
   provider's env var as fallback (genai's own default), `ServiceTargetResolver` answers the
   custom-endpoint cases. A selection that cannot make a client (compat with no URL, keyed
   provider with no key anywhere) is a typed error naming the missing field and where it is
   set — this is what the pane's honest degradation (AS-04) renders.
2. **The turn loop.** Input: system prompt + pinned context blocks (@-mentions arrive as
   `describe_table` results the pane already fetched) + transcript. One iteration: send with
   the manifest's tools → consume the stream, forwarding text deltas and tool-call starts to
   the caller as they arrive → on settled tool calls, execute each through the AS-01 facade,
   append the `ToolCall`/`ToolResponse` pair to the message list, iterate. Stop on a prose
   settle, on error, on cancel, or on a bounded number of tool rounds per send (a guard
   against a runaway loop — refusing with a plain "stopped after N tool rounds" beats spinning;
   pick N generously).

   **The name→method binding is this task's, and it is deliberately here.** AS-01 ships ten
   typed methods plus `StrataTools::manifest()`; a model answers with a *name* and a JSON
   object, and turning one into the other needs both the provider's tool-call type and a
   message for bad arguments that reads well to a model — neither of which belongs in a crate
   with no provider in it. Keep it one match over `manifest()`'s names with a test that every
   manifest entry dispatches, so a tool added to the router cannot reach the model with no arm
   behind it. Do **not** grow a second tool trait or registry to avoid the match: rmcp's
   `ToolRouter` already is that registry (which is what `manifest()` reads), its dispatch path
   needs a live `Peer` we do not have, and it answers in content blocks rather than typed
   values — the AS-01 file records the survey.
3. **The outward stream.** The loop reports events on a channel the pane consumes: turn
   started, text delta, tool call started (name + args), tool call settled (the result the
   *pane* needs for a step card: SQL · row count · elapsed · query session — not the full
   JSON), turn settled / failed / cancelled / stopped-at-cap. This event vocabulary is the
   pane's data source; keep it small and let the transcript state own its accumulation.
4. **Cancel.** A `CancellationToken` per turn. Cancelling drops the genai stream (the HTTP
   request dies with it) and must also settle an in-flight engine run through the same abort
   the connection-drop path already has (AA-03c's tombstone — verify against it, don't grow a
   second abort). A cancelled turn settles as *cancelled*, never as failed —
   `stopped_on_purpose` stays the only judge of stopped-vs-failed underneath.
5. **Errors pass through verbatim.** A tool error (§7 taxonomy) goes back to the **model** as
   the tool's result — that is the design working, not a failure: the model reads "CREATE
   TABLE is not supported…" and recovers, exactly as an MCP client would. A *transport/provider*
   error (bad key, dead endpoint, over quota) fails the turn and surfaces to the pane with the
   provider's own message.
6. **Runtime.** genai needs Tokio; the loop runs on `strata-agent`'s own runtime — but note
   `AgentServer`'s runtime exists only while agent access is enabled, and the assistant must
   not require that setting. Give the assistant module its own small runtime on the Engine
   pattern (private runtime, callers await `JoinHandle`s) or hoist a shared one — decide
   against the code, not this file, and name the reason where it lands.

## System prompt

Authored here, once: what Strata is, the IDE register for user-facing prose (AGENTS.md §3 —
the assistant's words render in the transcript), the tool guidance (validate before run when
unsure; read_page for more rows; sessions are yours; the user may promote), and honesty rules
(cite runs, never invent columns). Keep it in a `.md` include, not a string literal.

## What is NOT this task

- No Freya, no Settings UI (AS-03), no transcript rendering (AS-04).
- No conversation persistence — the transcript is the pane's ephemeral state (like
  `state::agents`); if chat history is ever wanted it is its own task with its own file rules.
- No RAG, no embeddings, no genai `chain`/`agent`-adjacent features: chat + tools + streaming
  only.

## Acceptance

- A loop test drives a full multi-turn tool exchange — question → tool call (`run`) → tool
  result → prose answer — against `MockHost` and a **local stub endpoint** behind genai's
  OpenAI-compat adapter (`ServiceTargetResolver` pointed at a test axum server speaking the
  compat wire shape). No production signature bent for the test (the bar).
- Streaming order is observable: text deltas arrive before the turn settles; a tool round's
  events arrive between them in the right order.
- Cancel mid-stream and cancel mid-run both settle the turn as cancelled and leave the engine
  clean (no run left in flight — assert via the session's state).
- A policy refusal round-trips: the stub model sends blocked DDL, receives the editor's own
  message as the tool result, and the turn still settles in prose.
- A config that cannot make a client yields the typed error naming field + Settings, without
  panicking and without a network attempt.
