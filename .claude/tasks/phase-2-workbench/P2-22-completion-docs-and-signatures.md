# P2-22 · Function signatures in autocomplete

**Phase:** 2 — Workbench · **Status:** ✅ (narrowed scope) · **DEV_TASKS:** E2 (polish) · **Depends on:** P2-04 · **Related:** P2-18

## Outcome (as shipped)
The valuable half of the original goal landed: **the completion popup now shows a
function's signature, not just its name.**

- **Core — enriched function catalog.** `FunctionCatalog` holds
  `FunctionSym { name, kind, signatures: Vec<Vec<String>>, ret, description }` instead
  of bare names. `Engine::new` snapshots every registered built-in from the live
  DataFusion registry (`ScalarUDF`/`AggregateUDF`/`WindowUDF` `signature()` /
  `return_type()` / `documentation()`) into display strings — engine-side, so the UI
  never touches DataFusion (`strata-core::engine::functions`). A `short_type` renderer
  collapses arrow's verbose spellings (`Timestamp(Nanosecond, "+TZ")` → `Timestamp`,
  `Decimal128(38, 10)` → `Decimal`, `List(..)` → `List`) so signatures stay readable;
  return types resolve from the signature's own example arg types, guarded against the
  UDFs that panic on an empty slice.
- **Completion detail = the arity form.** `Completion.detail` for a function is
  `round(Float64[, Int32])` / `date_bin(Interval, Timestamp[, Timestamp])` instead of
  the flat `"function"` (`FunctionSym::detail()`).
- **Wider popup.** The completion popup was widened (300 → 480px) and the name/detail
  labels capped single-line, so a long signature can never collide with the name.

## Dropped / deferred (explicitly)
The two floating-popup surfaces in the original plan were prototyped and **removed** —
they fought the editor's per-line pointer model and the resizable-split overlay/clip
behaviour, and the attempt to fold docs into the diagnostics hover panel broke that
panel. Alex called the in-autocomplete signatures "good enough" and we stopped there.

- ~~Docs side-panel on completion items (⌃Space toggle)~~ — dropped.
- ~~Signature-help popup while typing arguments (active-param highlight)~~ — dropped;
  `sql::signature_help` and `sql::hover_docs` were built then removed.
- The diagnostics (squiggle) hover popup is **unchanged** from P2-18 — deliberately not
  generalised to fire on functions (that's what caused the breakage).

If richer docs/signature UX is wanted later, extend the autocomplete surface rather
than the diagnostics hover, and treat any change to the editor's
`hover`/`update_hover`/pointer handlers as high-risk (verify diagnostics still work).

## Original scope (for reference)
Two follow-ons to P2-04's completion popup, both about telling the user more than a
name: (1) a docs side-panel showing signatures/return/dtype for the selected row, and
(2) signature help while typing arguments (`round(value [, decimals])` with the active
parameter highlighted). Only the underlying **signature metadata** (build step 1) and
its use in the completion detail shipped.

## Freya / references
- P2-04's popup (`strata-code-editor::completion`, `editor_ui.rs`).
- DataFusion `ScalarUDF::signature()/return_type()/documentation()`, `TypeSignature`.
