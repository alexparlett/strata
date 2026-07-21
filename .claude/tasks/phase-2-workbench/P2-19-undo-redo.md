# P2-19 · Undo / redo per tab

**Phase:** 2 — Workbench · **Status:** ⬜ · **Depends on:** —

## Goal
⌘Z / ⇧⌘Z undo/redo scoped to each tab's editor.

## Current state
Not implemented. Each tab owns its `CodeEditorData` in the session store.

## Build
Decide between the `CodeEditorData` built-in history (if sufficient) and an explicit per-tab
undo/redo stack; wire ⌘Z / ⇧⌘Z (keymap in Phase 6; a direct handler is fine first). Ensure history is
per-tab, not global.

## Acceptance
- [ ] ⌘Z/⇧⌘Z undo/redo edits in the active tab only; switching tabs preserves each history.

## Freya / references
- `strata-code-editor` `CodeEditorData`. Verify its history API in the crate before choosing.
