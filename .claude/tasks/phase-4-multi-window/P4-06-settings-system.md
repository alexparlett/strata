# P4-06 · Settings ▸ System (+ history limit)

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** W3 / U12 · **Depends on:** P4-03

## Goal
The System category, including the query-history limit.

## Current state
Not built. `Settings.max_history` caps `project.history` (P3-14).

## Build
- System prefs as a uniform divider-separated list (no ALL-CAPS labels — U12 alignment).
- **History limit** = `Settings.max_history` (default 100) as a numeric input, like the data-display
  fields; the history list is truncated to the cap after each insert (P3-14).

## Acceptance
- [ ] System fields edit the draft; history-limit changes cap the History drawer.

## Freya / references
- Design: `Settings.dc.html` System. DEV_TASKS W3/U12. `Settings.max_history`.
