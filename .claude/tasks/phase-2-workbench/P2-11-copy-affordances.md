# P2-11 · Copy affordances (TSV / CSV / JSON / Markdown) + grid focus

**Phase:** 2 — Workbench · **Status:** ✅ · **DEV_TASKS:** Rz4 · **Depends on:** P2-03

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
6. Wire the **record view's** header buttons (P2-10 — rendered, currently no-ops) into this
   same path: Copy row as CSV = `write_selection` over one page-batch row, all columns, with
   header; Copy row as JSON = a **bare row object** per the canvas (`buildRowJSON`: single
   `{col: value}`, nulls explicit — not `write_selection`'s array-of-objects), as a new core
   serializer beside `cell_pretty_json`. Map a find-filtered display row back through
   `cell_view::page_batch_row` first, as the record view's nested blocks already do.

## Acceptance
- [x] Right-click selection → four copy formats produce correct, header-carrying output.
- [x] Nested cells serialize as JSON in every format.
- [x] With the grid focused: ⌘A selects all cells, ⌘C copies TSV, and Edit ▸ Select All /
      Copy do the same; with the editor focused they keep acting on the editor.

## As built
The clipboard side effect moved UI-side: core's `ClipboardWriter` (arboard) was deleted —
`serialize` produces text into any `io::Write` sink, and the Freya app commits via
`freya::clipboard::Clipboard::set` (the per-window copypasta provider freya-winit already
creates — the same stack the text inputs use). The shared path is `results/copy.rs`:
selection → sorted in-page rows/cols (find-filtered display rows map back through
`page_batch_row`) → `write_selection` → clipboard, plus the right-click `copy_menu`
(reusing the tab-bar's `menu_row` for the ⌘C hint) and the record view's two buttons
(`copy_record_csv`; `copy_record_json` via the new core `serialize::row_pretty_json`, a
bare explicit-null `{col: value}` object — noted on F6 as a 4th arrow-json round-trip
consumer). Focus routing is pure a11y: cells' `SelCtl` mutators `request_focus()` the grid
surface (`a11y_focusable`), and a **focused** `on_key_down` resolves ⌘A/⌘C against the live
keymap — location-less key events dispatch only to the a11y-focused node, so text surfaces
keep the chords whenever they hold focus, with zero menu-side coordination. Gotcha for the
record: `on_secondary_down` is sugar over `on_pointer_down` (one handler per event name), so
the cells' right-click branch lives *inside* their selection pointer-down handler.

## Freya / references
- Freya `ContextMenu`/`Menu`. Core `crate::serialize` (the shared Arrow serializer) + arboard.
