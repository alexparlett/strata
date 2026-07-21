# P2-11 · Copy affordances (TSV / CSV / JSON / Markdown)

**Phase:** 2 — Workbench · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** Rz4 · **Depends on:** P2-03

## Goal
Right-click a selection → Copy as TSV / CSV / JSON / Markdown; ⌘C = TSV.

## Current state
Not built. Selection exists (`SelCtl`); no context menu, no clipboard wiring.

## Build
1. A **context menu** (Freya `ContextMenu`/`Menu`) on the grid selection with the four formats.
2. Project + `take` the selection into a `RecordBatch`, then write it with the shared
   `crate::serialize` writers (arrow-csv, `PrettyJsonWriter`, `MarkdownWriter`); nested cells stay
   real JSON, flattened to compact JSON for flat formats; all carry headers.
3. Copy to the clipboard (arboard, in core). Clipboard is **page-bounded** (no export-to-clipboard).
4. ⌘C = TSV (wire with the keymap, Phase 6; menu path first).

## Acceptance
- [ ] Right-click selection → four copy formats produce correct, header-carrying output.
- [ ] Nested cells serialize as JSON in every format.

## Freya / references
- Freya `ContextMenu`/`Menu`. Core `crate::serialize` (the shared Arrow serializer) + arboard.
