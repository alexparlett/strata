# P4-14 · Session persistence + autosave

**Phase:** 4 · **Status:** ⬜ `[core ✓ IO]` · **DEV_TASKS:** project lifecycle · **Depends on:** P4-13

## Goal
Keep `.strata/session.json` (and the `project.json` defs) in sync as the user works.

## Current state
Not built (`session.rs`: "Persistence — a serde snapshot — is a later slice"). `SessionState` holds
live `QueryTab`s whose `CodeEditorData` **isn't serde**, so persistence goes through a snapshot.

## Build (state-arch §4/§5)
1. **`SessionSnapshot`** — a serde view of `SessionState`: each tab's **text + origin + language**,
   the order / active / closed stack, layout, inspector selection, per-tab view intent, and history.
2. **Autosave** — a debounced `use_side_effect` writes `session.json` on change (tabs, layout,
   history, window). Local-only (gitignored).
3. **project.json** — written on catalog/def changes (view create/drop, saved-query, register/
   deregister): the durable, shareable **defs**, separate from the ephemeral session.
4. **Dirty tracking** — a tab is dirty via `Origin` + content hash (`is_dirty = editor.is_edited()`).
5. ⚠️ **Known bug:** editing a view's SQL + ⌘S must **update the view** (route by `Origin`), not save
   a new saved-query — pairs with P2-16.

## Acceptance
- [ ] Edits / tabs / layout / history persist to `session.json` (debounced) and restore on reopen.
- [ ] Catalog def changes persist to `project.json`; dirty state tracks per tab.

## Freya / references
- state-arch §4 (durable client model), §5 (persistence). Core `.strata/` IO. Memory
  `project-persistence`. DEV_TASKS Known bugs (the ⌘S-on-a-view bug).
