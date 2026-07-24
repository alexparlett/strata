# P2-22 · Completion docs + function signature help

**Phase:** 2 — Workbench · **Status:** ⬜ · **DEV_TASKS:** E2 (polish) · **Depends on:** P2-04 · **Related:** P2-18

## Goal
Two follow-ons to P2-04's completion popup, both about **telling the user more than a name**:

1. **Docs panel on completion items** — a side panel beside the popup (VS Code's
   `expandSuggestionDocs` / JetBrains' auto-docs) showing, for the selected row:
   functions → signature(s) + return type; columns → dtype/nullability (+ stats when known);
   tables/views → column count / source. Toggle with ⌃Space while open (VS Code convention),
   or auto-show after a short dwell (JetBrains).
2. **Signature help while typing arguments** — after `fn(` and on `,`, a small popup above
   the caret showing `round(value [, decimals])` with the **active parameter highlighted**
   (LSP `signatureHelp` UX). Dismiss on `)` / Esc; re-anchor per argument.

## Current state
- P2-04 accept-chaining already re-triggers completion after `fn(` — the *argument value*
  suggestions (columns) pop, but nothing tells you the function's arity/types.
- `FunctionCatalog` (`strata-core::sql`) carries **names only** (`scalar/aggregate/window:
  Vec<String>`, pushed from the engine registry at startup).

## Build
1. **Core: enrich the function catalog.** DataFusion's `ScalarUDF`/`AggregateUDF`/`WindowUDF`
   expose `signature()` (`TypeSignature` — arg counts/types, variadic forms) and
   `return_type(args)`. Render each to a display string at registry-snapshot time
   (`FunctionSym { name, signatures: Vec<String>, ret: Option<String> }`); keep it engine-side
   so the UI never touches DataFusion. `Completion.detail` for functions can then show the
   arity form (`round(x [, d])`) instead of the flat "function".
2. **Core: signature-at-caret.** `sql::signature_help(sql, caret, catalog) ->
   Option<SignatureHelp { label, active_param }>` — token-scan back from the caret to the
   innermost unclosed call (`ident (` at depth), count top-level commas for the active
   parameter. Pure + unit-tested like `complete`.
3. **Editor: docs side panel.** A second overlay rect beside the popup (right, flip to left
   near the edge — reuse `flip_and_clamp`), fed by a `docs_for(item) -> Option<String>`
   callback prop (generic, like `on_completions`). ⌃Space toggles while open.
4. **Editor: signature popup.** Small single-line overlay above the caret line (same anchor
   math), driven by an `on_signature: Callback<CompletionRequest, Option<SignatureHelp>>`
   prop; recompute on `(`/`,`/`)`/caret-move; suppressed while the completion popup is open.
5. Theme: `completion_docs_*` / reuse `panel_*`; schema regen.

## Acceptance
- [ ] Selecting a function row shows its signature(s) + return type in a docs panel; columns
      show dtype; ⌃Space toggles the panel.
- [ ] Typing inside `round(|` shows `round(value [, decimals])` with the first param
      highlighted; the highlight advances on `,`; closes on `)`.
- [ ] Signature + docs sources are engine-authoritative (DataFusion registry), never
      hand-listed; both features fully off when the props aren't wired.

## Freya / references
- P2-04's popup (`strata-code-editor::completion`, `editor_ui.rs` overlay + flip math).
- DataFusion `ScalarUDF::signature()/return_type()`, `TypeSignature` rendering.
- VS Code `toggleSuggestionDetails` / LSP `textDocument/signatureHelp` for UX shape.
