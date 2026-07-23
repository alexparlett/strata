# P2-19 · Undo / redo per tab

**Phase:** 2 — Workbench · **Status:** ✅ · **Depends on:** —

## Goal
⌘Z / ⇧⌘Z undo/redo scoped to each tab's editor.

## Current state
Done — and generalized. Each tab's `CodeEditorData` built-in history (freya-edit
`EditorHistory`) provides the per-tab stacks; no explicit stack was needed.

The whole text-editing chord set is now **configurable in the fork** rather than hardcoded:
freya-edit's `process_key` resolves an `EditBindings` (select all / copy / cut / paste /
undo / redo → `Vec<EditChord>`, exposed per instance via `TextEditor::edit_bindings`)
instead of its old hardcoded ⌘A/⌘C/⌘X/⌘V/⌘Z/⌘Y arms. Defaults preserve the platform
conventions (and fix ⇧⌘Z-as-redo, which the old hardcoding got wrong).

Strata-side, all six are rebindable keymap commands (`Command::{Undo, Redo, Cut, Copy,
Paste, SelectAll}`, classified by `Command::is_edit`); `RESERVED_KEYS` and
`BindError::Reserved` are gone. `keymap::edit_bindings(settings)` converts the effective
chords for the text layer, and the editor tab keeps the buffer's `EditBindings` synced via
a side effect — a rebind in Settings retargets the editor live. The tab's pre-key gate lets
exactly the chords that resolve to an edit command through to `process_key`; other
primary-held chords stay app shortcuts.

`CodeEditorData::undo_edit` / `redo_edit` remain as the programmatic entry (full edit path:
history revert + selection restore + re-parse + re-measure) for dispatch that arrives
outside the keyboard — the future muda Edit menu (whose accelerators claim the keys at
menubar level) and the command palette route through these.

## Acceptance
- [x] ⌘Z/⇧⌘Z undo/redo edits in the active tab only; switching tabs preserves each history.
- [x] All text-editing chords are settings-driven (`Settings::keybinds` via
      `keymap::effective_chord`), not hardcoded in freya-edit.

## Freya / references
- Fork: `freya-edit` `EditBindings` / `EditChord` / `EditAction` (`config.rs`),
  `TextEditor::{edit_bindings, process_key}`, `RopeEditor::set_edit_bindings`.
- App: `strata-core::keymap` (`COMMANDS`, `Command::is_edit`), `strata-code-editor`
  `CodeEditorData::{set_edit_bindings, undo_edit, redo_edit}`, editor gate + sync in
  `strata-freya` `views/workbench/editor/tab.rs`, conversion in `strata-freya::keymap`.
