# AM-07 · Spec + end-to-end

**Workstream:** Assistant memory · **Status:** ⬜ · **Depends on:** AM-01…AM-06

## Goal

Close the workstream the way the others closed: `docs/MEMORY_SPEC.md` written **as built**
(the vocabulary, the dataflow, the fusion, the failure taxonomy, the refusals-that-aren't —
what deliberately never errors), the docs index row, one integration test that exercises the
whole loop — capture → consolidate → recall → search tool — through the scripted stub, and
the backlog index updated to a closed state.

## Current state (verified 2026-08-13)

- The spec convention: substantial workstreams land a `docs/*_SPEC.md` describing the system
  **as built**, never as planned (`CONNECTIONS_SPEC.md`, `CHART_SPEC.md`,
  `AGENT_ACCESS_SPEC.md` — the last closes with a "What is not built" section, which is where
  a follow-on's shape gets recorded). Indexed as a row in `docs/README.md`'s table.
- The rig: `crates/strata-agent/tests/assistant.rs` runs real turns against `MockHost` + a
  scripted local OpenAI-compatible HTTP stub through the roster's own `OpenAiCompatible`
  kind — no window, no vendor account. AM-02's CI step provides the local embed model.
- `.claude/tasks/README.md` carries the workstream's bullet, its Rough-order entry and the
  open-workstreams parenthetical; a completed workstream's folder is removed and its settled
  record moves to `docs/reference/SETTLED_TASKS.md` — **that closeout happens when Alex calls
  the workstream done, not automatically with this task**; this task leaves the index
  *accurate* (statuses, corrections recorded).

## Build

1. **The end-to-end test**: one scripted scenario — a turn whose reply and `memory_ops`
   distill land a fact and a recipe in a temp store; a **new** conversation (fresh
   `Conversation`) whose captured request proves the `Project memory` block arrived inside
   the fence; a scripted `search_memory` round answering with totals; the toggle off →
   none of it happens. With the embed model present (CI), assert the paraphrase case;
   without, the test **fails** (never skips).
2. **`docs/MEMORY_SPEC.md`**: the record + kinds; the store (LanceDB, why, the lockstep
   note); the embedder (bundled model, the constant, the resolution order); capture (the
   settle hook, distill's ops, the handle mapping, the one-attempt rule); recall (block +
   budget, `search_memory`, both prompts quoted by section); the four-signal fusion with
   its weights; the failure taxonomy table (AM-06's tiers verbatim); "What is not built"
   (injection-visibility affordance, recall-time recipe validation, per-user models,
   MCP-exposed memory — each with its recorded reason).
3. **Index rows**: `docs/README.md` gains the MEMORY_SPEC row;
   `docs/AGENT_ACCESS_SPEC.md`'s "What is not built" gains/updates the assistant-only-tools
   note pointing at the spec; `docs/reference/MODULE_MAP.md` and `docs/ARCHITECTURE.md`
   checked true against everything the workstream landed.
4. **Backlog truth pass**: every AM task file's status current, corrections recorded in the
   file that owns them; the workstream README's risk list resolved into what was actually
   built (the FTS-API and ort-linking verifications from AM-01/AM-02 especially);
   `.claude/tasks/README.md`'s AM bullet updated to the workstream's real state.
5. Full-check sweep: clippy + tests (container runtime + embed model in CI), a
   `bundle-macos.sh --arch arm64` build, and one manual pass — learn a fact in a real chat,
   new chat recalls it, prune it in the panel, `Clear`, relearn.

## Acceptance

- The end-to-end test fails if any link in capture → store → recall → tool breaks, and runs
  in CI.
- `docs/MEMORY_SPEC.md` describes only what exists; a reader can find every claim in code.
- The manual pass holds on a bundled build with the network cable pulled (recall + capture
  local; the turn itself needs the provider, so use Ollama for the offline half).
- Full check green.

## Files

`docs/MEMORY_SPEC.md` (new) · `docs/README.md` · `docs/AGENT_ACCESS_SPEC.md` ·
`docs/reference/MODULE_MAP.md` · `docs/ARCHITECTURE.md` (if touched surfaces moved) ·
`crates/strata-agent/tests/assistant.rs` · `.claude/tasks/README.md` + this folder's files.

## What is NOT this task

New capability of any kind — anything discovered missing here becomes a new task file, not
scope creep in this one. The SETTLED_TASKS migration and folder removal (Alex's call, after
the workstream has soaked).
