# P2-18 · SQL validation + inline squiggles

**Phase:** 2 — Workbench · **Status:** ✅ · **DEV_TASKS:** E1 · **Depends on:** — · **Related:** P2-04 (autocomplete), P3-11 (Problems drawer)

## Goal
Diagnostics from the shared service over the editor text, with the full coverage set, plus inline
squiggles. One task — no later coverage pass.

## Shipped (clean-slate rebuild — no Dioxus carry-over)
1. **Core `sql::validate` rebuilt as the engine dry-plan** (`strata-core::engine::sql::validate`,
   facade `Engine::validate(sql)`): per-statement (top-level `;` split, token-level) —
   lexical lints (unterminated string, unbalanced parens, keyword-typo warning) → **managed-DDL
   policy** classified on the *parsed* `DFParser` statement (only queries / EXPLAIN / SHOW /
   DESCRIBE pass; CREATE EXTERNAL TABLE / CTAS / INSERT / COPY / SET / CREATE DATABASE **and
   CREATE/DROP VIEW** get a guidance diagnostic naming the owning surface — views are Save's
   artifact: ⌘S wraps the *plain query* in `CREATE OR REPLACE VIEW` itself, so typed view DDL
   would nest DDL inside the wrapper) → **dry-plan** (`sql_to_statement` → `statement_to_plan` → `optimize`,
   never executed): unknown table/view/column/function, bad casts, un-coercible expressions — the
   exact errors a Run would hit. DF 54 planner `Diagnostic` **source spans** (engine enables
   `datafusion.sql_parser.collect_spans`, now an owned key) map to byte spans; parser errors fall
   back to the `Line:/Column:` in the message; else the statement's leading keywords. Statements
   accumulate independently. 14 unit tests.
2. **Squiggles natively in Skia**: the fork gained `TextDecorationStyle` (incl. **Wavy**) +
   `text_decoration_color` on `Span`/paragraph text styles; `strata-code-editor` gained a
   `Decoration` layer on `CodeEditorData` (`set_decorations`, byte→char clamped) and
   `EditorLineUI` splits syntax runs at decoration boundaries (severity-colored wavy underline,
   themed via `code_editor.diagnostic_{error,warning,info}` — both stock themes reference the
   sheet's `error`/`warning`/`info` tokens). Decorations carry the **message**, and the editor
   shows a **hover popup** (severity dot + message, `code_editor.panel_background/panel_border`)
   while the pointer sits on a decorated span — every diagnostic covering that char; clears on
   move-off / editor leave / a new pass / drag.
3. **Per-tab driver** (`query::use_validation`, mounted in `EditorTab`): buffer-revision-gated
   (caret traffic never re-validates), 300 ms cancel-and-rearm debounce (`spawn` + `Timer`;
   cancelling a mid-await pass *is* the supersede) with an **asymmetric surface delay** — a pass
   that only clears/keeps shown diagnostics applies at 300 ms, one introducing new ones holds
   another 700 ms of quiet (trailing valid-prefix statements are also never flagged: `found: EOF`
   on the last statement stays silent). Applies decorations into the buffer and
   `QueryTab::diagnostics` on its own `Chan::Diagnostics(id)` — ready for the P3-11 Problems
   drawer (`validation(tab) ∪ query_error(tab)`, state-arch §8/§9).

## Acceptance
- [x] Each case produces a byte-spanned diagnostic; multiple errors all report (accumulated).
- [x] Squiggles render under the offending spans; the Problems drawer (P3-11) reads the same
      diagnostics off `Chan::Diagnostics(id)` when it lands.

## Freya / references
- Core `strata-core::engine::sql::validate` (engine dry-plan — the authority; SQL_LANGUAGE_SPEC §3
  "preferred" path). Fork: `TextDecorationStyle` in `freya-core/src/style/text_decoration.rs`.
  state-arch §8/§9 (Problems = validation ∪ query_error).
