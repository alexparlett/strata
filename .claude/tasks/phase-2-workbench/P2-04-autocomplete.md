# P2-04 · SQL autocomplete (completions + follow-ups)

**Phase:** 2 — Workbench · **Status:** ✅ · **DEV_TASKS:** E2 · **Depends on:** — · **Related:** P2-18 (validation/squiggles), P2-22 (docs/signatures), P2-23 (validator fitness)

## Goal
Editor completions from the shared `strata-core::sql` service, follow-ups included — no later pass.

## Outcome (see `docs/COMPLETION_SPEC.md` — the as-built design)
- **Core** rebuilt as a clause×role grammar model (`engine/sql/complete/` module:
  vocabulary tables, composed ranking forces, guards) — context-aware pools, fuzzy
  match tiers, CTE/derived-table resolution, join-key + type affinity, written
  demotion, projection-coverage ranking; first-ever `complete()` test suite incl. a
  torture corpus with an every-caret sweep.
- **Editor** (`strata-code-editor`) owns a generic synchronous popup: word-start
  anchor, overlay flip-up, single-undo accept, accept-chaining, ⌃/⌘Space.
- **App** wires the provider with a memoized `Catalog`; validation stays quiet on
  FROM-less drafts.

## Acceptance
- [x] Completions appear (tables/columns/keywords) and insert correctly.
- [x] Manual trigger (⌃Space, plus ⌘Space where Spotlight is remapped); the list
      flips up near the edge; the caret lands correctly after accept.

## Freya / references
- `docs/COMPLETION_SPEC.md` · `strata-core::engine::sql::complete` ·
  `strata-code-editor::{completion, editor_ui}` · `editor/tab.rs` (provider mount).
