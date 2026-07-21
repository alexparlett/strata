# P3-12 · Drawer — Problems tab

**Phase:** 3 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** U10 · **Depends on:** P3-11, P2-18, P2-01

## Goal
Live per-tab diagnostics in the Problems tab.

## Current state
Not built. Diagnostics = `sql::validate` output (P2-18) ∪ the tab's query error (P2-01).

## Build
- Render `validation(editor.text) ∪ query_error(tab)` for the **active tab** (state-arch §8) — **not**
  a log. They **self-clear** when the SQL is fixed or the query re-runs.
- Row = **icon · message · line** (no code chip — dropped in the Dioxus app, DEV_TASKS U10).
- **No Clear button** on Problems (the scaffold hides it — deliberate, do not "fix").
- Empty state: "No problems — queries are clean".

## Acceptance
- [ ] Problems reflects the active tab's diagnostics live and updates as the SQL changes / re-runs.
- [ ] No Clear button; empty state shows the clean message.

## Freya / references
- state-arch §8 (Problems = validation ∪ query_error). Core `sql::validate` + query error. Design:
  `DrawerProblems.dc.html`. DEV_TASKS U10 (row shape + the deliberate no-Clear divergence).
