# AM-03 · Extraction + consolidation

**Workstream:** Assistant memory · **Status:** ⬜ · **Depends on:** AM-01 (AM-02 optional —
capture works FTS-only until it lands)

## Goal

The assistant learns: after a turn settles, a background call distills durable **facts** and
**recipes** from the exchange and consolidates them against what is already stored — mem0's
shape, where the call is shown the top-k related existing memories and answers with
ADD / UPDATE / DELETE / NOOP operations, so dedup and correction happen inside the call. Plus
the switch (`Ai::memory_enabled`, default on) and the window's `Memories` handle. This is the
earliest end-to-end demonstration of the workstream: chat about a table, open the store, see
the facts.

## Current state (verified 2026-08-13)

- The settle points are `Chats::settle` (`state/chat.rs:689`) and `Chats::finish` (:713); the
  send task's epilogue (`chat_send.rs:203-252`) drains events → `fold` → `store` (:262,
  offloaded) → `finish`. **The hook belongs after `finish`** — `Chat::running` is released, so
  extraction never blocks the next send.
- The turn already read the key: `chat_send.rs:189-238` — `ai.setup(kind)` → `SecretRef` →
  `offload(get)` → `Selection.api_key`. The epilogue still holds the `Selection`; extraction
  reuses it — **no second keystore read**.
- `Brain::resolve(selection, pool)` (`provider.rs:700`) is the one genai-client construction
  site; a single non-streamed round is `exec_chat` on the same seam. `offer.rs` is the
  precedent for a schema-taught tool (`spec()` via `rmcp::…::schema_for_input`, offer.rs:64)
  — prompt-taught formats fail on small local models, a schema does not.
- `Assistant` (`assistant/mod.rs:59`) owns a 2-worker runtime; `send` (:100-135) returns
  `Running` whose drop is the cancel. `distill` is different on purpose: fire-and-forget, no
  cancel handle — a settled exchange's learnings should land even if the pane moves on. The
  root-scoped-task rule (AGENTS §3) applies to the *state fold only*: disk writes finish,
  UI folds check alive-ness.
- A task spawned from a Freya handler dies with its scope (AGENTS §3) — which is why the
  extraction future must ride the assistant's runtime, not a Freya `spawn`.
- `Ai` (`ai.rs:121`) is flat fields riding `settings_merge!`; Settings ▸ AI ▸ Chat
  (`apps/settings/views/ai/chat.rs`) holds the chat rows (`max_chats`'s `NumberField` at :63
  is the row shape to copy); `AssistantCtx` is provided at `apps/project/app.rs:478-490` —
  the `Arc<Memories>` slot mounts beside it.
- `Facts` (`dispatch.rs:71`) carries each step's `{sql, rows, elapsed_ms, stopped}` — the
  exchange rendering reads the turn's blocks, not re-measured data.

## Build

1. **`assistant/distill.md`** (include_str!'d): the distiller's system prompt — what
   qualifies (schema shape, column meaning/units, project conventions, working SQL for a
   non-obvious query with a retrieval-oriented description), terseness rules, the
   tables-tagging requirement (folded names), the ops semantics against the shown `m1`…`mN`
   handles, and the never-store list: credentials/secrets, one-off result values, anything
   about the **user** rather than the project. NOOP when nothing durable was learned.
2. **`assistant::memory::distill`** (new module beside `offer.rs`): render the settled
   exchange bounded (~8 KB — the question, the reply's prose, each step's SQL from its
   `Facts`, each `offer_sql`); `memories.search(exchange_text, tables, 10)` for the related
   set, presented under handles with the handle→`Uuid` map kept caller-side; one `exec_chat`
   with the `memory_ops` tool spec (offer.rs's `spec()` shape); parse the tool call's
   arguments — or one attempt at the content as that JSON — into `Vec<MemoryOp>`; **give up
   silently otherwise** (tracing line, store untouched; retrying a model that cannot produce
   the shape spends tokens to learn nothing). Map handles back to ids; unknown handle → op
   dropped, logged.
3. **`Assistant::distill(selection, memories: Arc<Memories>, exchange) -> ()`** — spawns on
   the assistant runtime, calls the module fn, `memories.apply(ops)` on success. No handle
   returned; errors are tracing only. Nothing here can fail a turn — the turn is already
   over.
4. **The hook**: in `chat_send.rs`'s send task after `finish`, gated on
   `config.settings.ai.memory_enabled` and on the window's `Memories` being open; assemble
   the exchange rendering from the just-folded turn, call `Assistant::distill`. The UI-side
   fold (if any listing surface is open — AM-05) is driven by a `State` bump guarded by
   `is_alive`.
5. **The handle**: open `Memories` per project window at mount (background — `open` does IO;
   park the result in a `State<Option<Arc<Memories>>>` beside `AssistantCtx`; an open error
   logs once and leaves `None` = memory off for this window, AM-06 hardens the wording).
6. **`Ai::memory_enabled: bool` (default true)** + the `settings_merge!` arm + one Settings
   ▸ AI ▸ Chat toggle row, subtext naming the spend ("Distills project facts after each
   assistant turn. One small model call per turn.").
7. **Tests**: distill through the scripted OpenAI-compatible stub (`tests/assistant.rs`'s
   rig) — a scripted `memory_ops` reply lands in a temp store; handle mapping; unknown
   handle dropped; unparseable reply leaves the store untouched; the never-store list is
   prompt text (not testable) but the bounded rendering is — an over-long exchange clips.

## Acceptance

- Chat about a table through the stub; the store then contains the scripted fact, and a
  second distill whose reply UPDATEs it by handle edits the same row (no duplicate).
- Extraction runs after `finish` (the next send is never blocked), only when the toggle is
  on, and a provider error during it changes nothing user-visible.
- The toggle row applies through the config funnel like every `Ai` field.
- Full check green.

## Files

`crates/strata-agent/src/assistant/distill.md` (new) ·
`crates/strata-agent/src/assistant/memory.rs` (new) ·
`crates/strata-agent/src/assistant/mod.rs` (`distill`) ·
`crates/strata-freya/src/apps/project/state/chat_send.rs` (hook) ·
`crates/strata-freya/src/apps/project/app.rs` (the `Memories` slot) ·
`crates/strata-core/src/ai.rs` + `crates/strata-freya/src/apps/settings/views/ai/chat.rs`
(toggle) · tests in `crates/strata-agent/tests/assistant.rs`.

## What is NOT this task

Recall of any kind — no injection, no tool (AM-04). The management panel (AM-05 — this task
may land a placeholder-free store nobody can browse yet; that is fine). Embedding (inside
`Memories`, AM-02's concern). Store lifecycle hardening beyond logging an open failure
(AM-06).
