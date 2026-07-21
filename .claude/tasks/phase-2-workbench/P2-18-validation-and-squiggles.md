# P2-18 · SQL validation + inline squiggles

**Phase:** 2 — Workbench · **Status:** ⬜ · **DEV_TASKS:** E1 · **Depends on:** — · **Related:** P2-04 (autocomplete), P3-11 (Problems drawer)

## Goal
Diagnostics from the shared service over the editor text, with the full coverage set, plus inline
squiggles. One task — no later coverage pass.

## Current state
Nothing in `strata-freya` calls `sql::validate` / `sql::analyze`. The core service exists and shipped
in the Dioxus app (byte-spanned `Diagnostic`s). The SQL language is already wired (see P2-04).

## Build
1. **Diagnostics:** compute `sql::validate`/`analyze` over the editor text (a `use_memo` /
   `use_side_effect` on the tab's buffer) into the tab's diagnostics. **Full coverage now** (E1):
   unknown table/view (reuse the context resolver), bad leading keyword, unterminated string —
   **accumulate all**, don't stop at the first. Per state-arch §8, Problems =
   `validation(editor.text) ∪ query_error(tab)` → surfaced in the Phase-3 Problems drawer (P3-11).
2. **Inline squiggles:** `CodeEditor` has **no decoration hook**, so underline diagnostic spans via
   the lower-level `use_editable` + `paragraph` `.highlights()` path (plan §8), keyed on the
   `Diagnostic` byte spans.

## Acceptance
- [ ] Each case produces a byte-spanned diagnostic; multiple errors all report (accumulated).
- [ ] Squiggles render under the offending spans; the Problems drawer lists the same diagnostics.

## Freya / references
- Core `strata-core::sql::{validate, analyze}` — do not re-implement. Plan §8 (no decoration hook →
  `use_editable`/`paragraph` `.highlights()`). state-arch §8 (Problems = validation ∪ query_error).
