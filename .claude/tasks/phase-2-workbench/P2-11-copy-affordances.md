# P2-11 · Copy affordances (TSV / CSV / JSON / Markdown) + grid focus

**Phase:** 2 — Workbench · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** Rz4 · **Depends on:** P2-03

## Goal
Right-click a selection → Copy as TSV / CSV / JSON / Markdown; ⌘C = TSV; ⌘A selects all
cells — both routed to the grid *when the grid is focused*.

## Current state
Not built. Selection exists (`SelCtl`); no context menu, no clipboard wiring. The grid is
**not a11y-focusable**, so every edit chord (typed or Edit-menu-synthesized — see
`menu.rs`) routes to the last-focused text element (usually the SQL editor), including ⌘A.

## Build
1. Make the grid surface **a11y-focusable** (click focuses it) — the prerequisite for all
   keyboard routing here. Keyboard dispatch (including the Edit menu's synthesized chords)
   routes by a11y focus, so nothing menu-side changes.
2. A grid key handler resolving the *edit commands* against the live keymap
   (`Command::SelectAll` → `SelCtl` select-all; `Command::Copy` → TSV copy). Mirrors the
   Dioxus behaviour: grid claims ⌘A/⌘C while focused, text surfaces keep them otherwise.
3. A **context menu** (Freya `ContextMenu`/`Menu`) on the grid selection with the four formats.
4. Project + `take` the selection into a `RecordBatch`, then write it with the shared
   `crate::serialize` writers (arrow-csv, `PrettyJsonWriter`, `MarkdownWriter`); nested cells stay
   real JSON, flattened to compact JSON for flat formats; all carry headers.
5. Copy to the clipboard (arboard, in core). Clipboard is **page-bounded** (no export-to-clipboard).

## Acceptance
- [ ] Right-click selection → four copy formats produce correct, header-carrying output.
- [ ] Nested cells serialize as JSON in every format.
- [ ] With the grid focused: ⌘A selects all cells, ⌘C copies TSV, and Edit ▸ Select All /
      Copy do the same; with the editor focused they keep acting on the editor.

## Freya / references
- Freya `ContextMenu`/`Menu`. Core `crate::serialize` (the shared Arrow serializer) + arboard.
