# P2-16 · Editor toolbar actions (Format · Clear · Preview · Save-as-view)

**Phase:** 2 — Workbench · **Status:** ✅ `[core ✓]` · **Depends on:** P2-01

## Goal
Wire the remaining editor-toolbar buttons and fix the view-save routing bug.

## Current state
`editor/toolbar.rs` renders Format · Clear (Trash) · Preview (Eye) · Save buttons — all stubbed.

## Build
1. **Format** → SQL format on the buffer. **Clear** → clear the editor buffer.
2. **Save-as-view / ⌘S** → managed DDL: capture `CREATE`/`DROP VIEW`; a plain query saves a
   saved-query.
3. **Fix the known bug:** a tab opened from an existing **view** must remember its origin and route
   ⌘S to `CREATE OR REPLACE VIEW` on that view — not create a new saved-query.

## Acceptance
- [ ] Format/Clear act on the buffer; Save creates/updates the right catalog object.
- [ ] Editing a view's SQL + ⌘S updates that view.

## Freya / references
- `editor/toolbar.rs`. Managed DDL policy in `strata-core` (see CLAUDE.md engine model). DEV_TASKS "Known bugs".
