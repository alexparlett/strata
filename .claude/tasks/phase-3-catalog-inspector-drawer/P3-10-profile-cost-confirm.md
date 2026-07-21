# P3-10 · Profile-cost confirm

**Phase:** 3 · **Status:** ⬜ · **DEV_TASKS:** U15 · **Depends on:** P3-09

## Goal
Confirm before a first profile scan; re-scans skip it.

## Current state
Not built.

## Build
A confirm dialog before a table's **first** profile (the ↻ re-scan skips it). Reached from the sidebar
table context menu (P3-06). **No cost figures, no `>50 files` gate** — it names the *shape* of the
work (full scan; distinct can't merge across files; cached until the table changes), not arithmetic.

## Acceptance
- [ ] First profile shows the confirm; re-scan does not; the copy describes the work, not a file count.

## Freya / references
- Freya `Popup`/dialog. DEV_TASKS U15 / D4 (the "no cost figures" decision). Design: dialogs canvas.
