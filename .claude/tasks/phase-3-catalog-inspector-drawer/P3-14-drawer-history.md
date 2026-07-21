# P3-14 · Drawer — History tab

**Phase:** 3 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** U10 · **Depends on:** P3-11, P2-01

## Goal
Query history in the History tab.

## Current state
Not built. History is **durable client model** — Radio, persisted to `session.json`, capped by
`Settings.max_history` (truncated after each insert).

## Build
- List past queries newest-first (meta · line-count badge · timestamp, per the canvas).
- **Click to load** into the editor (`onLoadHistory`); **double-click to load & run** (`onRunHistory`)
  — matches the Strata canvas history rows.
- **Clear** is supported (scaffold header). The cap is `Settings.max_history`.

## Acceptance
- [ ] Past queries list, capped at `max_history`; click loads into the editor; double-click loads + runs.
- [ ] Clear empties the history.

## Freya / references
- Durable client model (Radio → `session.json`). Canvas `onLoadHistory` / `onRunHistory`. Design:
  `DrawerHistory.dc.html`. `Settings.max_history` cap.
