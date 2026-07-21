# P2-04 · SQL autocomplete (completions + follow-ups)

**Phase:** 2 — Workbench · **Status:** ⬜ · **DEV_TASKS:** E2 · **Depends on:** — · **Related:** P2-18 (validation/squiggles)

## Goal
Editor completions from the shared `strata-core::sql` service, follow-ups included — no later pass.
(Syntax **highlighting is already wired**; this task is autocomplete only.)

## Current state
- **Done:** `state/session.rs::sql_language()` builds the `tree_sitter_sequel` `EditorLanguage`, and
  each `QueryTab`'s editor is `CodeEditorData::new(text, Some(sql_language()))` — so SQL is
  highlighted. (The `None::<EditorLanguage>` in `editor/tab.rs` is only the scratch fallback.)
- **Missing:** nothing in `strata-freya` calls `sql::complete`.

## Build
1. Bind `sql::complete` (candidates from catalog symbols + keywords) to the editor as an overlay.
2. **Follow-ups now** (E2, not later): **⌘Space** manual trigger, **flip-up** near the viewport bottom
   edge, **caret-after-accept** placed correctly after inserting a candidate.

## Acceptance
- [ ] Completions appear (tables/columns/keywords) and insert correctly.
- [ ] ⌘Space triggers them; the list flips up near the edge; the caret lands correctly after accept.

## Freya / references
- Core `strata-core::sql::complete` — do not re-implement. Freya `CodeEditor` completion flow.
- `state/session.rs` (language), `editor/tab.rs` (editor mount).
