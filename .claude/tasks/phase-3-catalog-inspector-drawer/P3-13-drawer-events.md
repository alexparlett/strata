# P3-13 · Drawer — Events tab

**Phase:** 3 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** U10 · **Depends on:** P3-11, P2-01

## Goal
The engine event log in the Events tab.

## Current state
Not built. The event router (P2-01) drains engine events; every engine/window/query event appends to
the log (state-arch §8).

## Build
- List the appended log entries (engine start/restart, registration, query errors, progress, …),
  newest-first, with the sticky group headers + row style from the scaffold.
- **Clear** is supported here (shown by the scaffold header).

## Acceptance
- [ ] Engine/window/query events appear in the log; Clear empties it.

## Freya / references
- state-arch §8 (the log). The P2-01 event router feeds it. Design: `DrawerEvents.dc.html`.
