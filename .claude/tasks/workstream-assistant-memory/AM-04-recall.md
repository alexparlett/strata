# AM-04 · Recall

**Workstream:** Assistant memory · **Status:** ⬜ · **Depends on:** AM-01, AM-03

## Goal

What was learned reaches the model: top-k relevant memories injected as a budgeted
`Project memory` context block on every send, and a `search_memory` tool the model can call
mid-conversation — assistant-only, `offer_sql`'s shape, never on the MCP router. Plus the
static `system.md` section that teaches the model both halves.

## Current state (verified 2026-08-13)

- `Ask::message()` (`turn.rs:172`) fences each `ContextBlock` as
  `<attached-context label="…">` prepended to the user message — deliberately not the system
  prompt, so provider prompt caches hold; injection-hardened at :181 (a block cannot close
  its own fence). The recall block is one more `ContextBlock` — no new channel.
- `chat_send.rs` builds `Ask.context` in the send task (`split_anchors` :462; the pinned
  `describe_table` blocks at :206-222). The recall block is appended there, **after** the
  visible chips are recorded by `chats.write().ask(…)` — the transcript shows what the user
  pinned; the model additionally sees what the project remembered.
- `offer_sql` is the assistant-only-tool precedent: spec appended in `turn::offered()`
  (`turn.rs:685`), handled in the loop via `offer::is_offer` (`offer.rs:44`) — never on
  rmcp's router, so `tools/list` stays byte-identical. `dispatch.rs:402`'s
  every-advertised-tool-has-an-arm test walks `manifest()`, which does not include offered
  extras; a same-shaped test sits beside `offer.rs`'s.
- `Assistant::send(tools, selection, scope, conversation, ask)` (`mod.rs:100`) — grows
  `recall: Option<Arc<Memories>>` (strata-agent already depends on strata-core). `None` for
  memoryless callers; the existing tests pass unchanged — an honest absent, not a shaped
  signature (AGENTS §1). The parameter is named `recall` and its bindings `memories`, never
  `memory` — that name is taken in this signature's own neighbourhood by the
  `Arc<Mutex<Conversation>>` turn history (`Chat::memory`, `state/chat.rs:266`, bound as a
  local `memory` in `chat_send.rs:171-179`), which is an unrelated concept.
- `system.md` (`turn.rs:54`) is byte-identical every request by design — the new section is
  **constant text**, present whether or not recall returned anything this send.
- AA-07's rule for list answers: bounded, **totals stated** — an answer with no totals reads
  as complete.
- `Memories::search` embeds the query inside the facade (AM-02); with no model it is the FTS
  floor. Local embedding is ~ms — no timeout machinery needed (the reason the provider-path
  design carried one is gone).

## Build

1. **`recall_block`** in `assistant::memory`: `recall_block(memories, question,
   context_tables) -> Option<ContextBlock>` — `search(question, tables, …)`, take hits
   best-first under **k ≤ 6 and a 4 KB byte budget** (a recipe whose SQL would blow the
   budget is skipped in favor of the next hit — the budget binds, not the count); render
   each as its kind, text, tables, and a recipe's SQL fenced; label `Project memory`; end
   with one constant sentence pointing at `search_memory` for anything not shown. Empty
   search → `None` (no empty block).
   `context_tables`: the folded table names off the send's pinned anchors — the same
   entity signal capture uses.
2. **Wire into the send**: in `chat_send.rs`'s send task, when the window's `Memories` is
   open, append `recall_block`'s answer to `ask.context` (after the chips are recorded).
   Any error is a tracing line and no block — recall never fails a send.
3. **`search_memory`**: params `{ query: String }` (no `project` — the Scope rule,
   offer.rs:53); spec via `schema_for_input`, appended in `turn::offered()` when `recall`
   is `Some`; handled in the loop beside `offer::is_offer`. The answer is bounded JSON —
   up to 10 hits of `{kind, text, sql, tables, updated_ms}` — **with the store's total and
   the shown count stated** (AA-07). Feeds through `bounded()` like every tool result.
4. **`Assistant::send`'s `recall` parameter** threaded from `chat_send.rs` (the window's
   `Arc<Memories>`); `None` everywhere else (headless host untouched — the tool simply
   isn't offered).
5. **`system.md` "Project memory" section** (static): what the `Project memory` block is —
   facts and SQL recipes learned in earlier conversations in this project, possibly stale,
   verify against the live schema when it matters; when to call `search_memory` — the user
   references prior work ("like we did last time", "that query from the other day"), or
   before re-deriving how to query a table whose quirks may already be recorded; results
   state their total.
6. **Tests** (the `tests/assistant.rs` stub rig, temp-dir store): a seeded store's fact
   arrives in the request body's user message inside the fence; the budget drops an
   oversized recipe; a scripted `search_memory` call gets hits with totals; `recall: None`
   offers no tool and injects nothing; the MCP manifest is byte-identical with recall on.

## Acceptance

- A new conversation about a table the store knows receives the block (verified in the
  stub's captured request), and the model can pull more via `search_memory`.
- `tools/list` unchanged; headless host unchanged.
- With an empty store: no block, tool answers "0 of 0", nothing else differs.
- Full check green.

## Files

`crates/strata-agent/src/assistant/memory.rs` (recall + tool) ·
`crates/strata-agent/src/assistant/turn.rs` (`offered`, the loop arm) ·
`crates/strata-agent/src/assistant/mod.rs` (`send` param) ·
`crates/strata-agent/src/assistant/system.md` (the section) ·
`crates/strata-freya/src/apps/project/state/chat_send.rs` (wiring) ·
`docs/AGENT_ACCESS_SPEC.md` (the assistant-only tool count in "What is not built") ·
tests in `crates/strata-agent/tests/assistant.rs`.

## What is NOT this task

Any transcript affordance showing which memories were injected — deliberately not built
(the management panel is the transparency surface); a future "used N memories" line would
need a `TurnEvent`-adjacent record. Validating a recipe's SQL at recall — recalled SQL is
context, not an executable card; the assistant validates anything before offering it
(`offer_sql`), which is the existing gate.
