# P2-09 · Find in results

**Phase:** 2 — Workbench · **Status:** ⬜ · **DEV_TASKS:** U6c · **Depends on:** P2-03

## Goal
A collapsible find popover over the results, opened from the toolbar Search button and ⌘F.

## Current state
The toolbar Search button exists but is inert — it already wears the keymap-derived
"Find in results (⌘F)" tooltip (`keymap::use_hint_title`). No find state, no popover.

## Build
1. Build a **search popover** on Freya `Popup`/`Backdrop`: the trigger measures its own rect and
   anchors the panel (BOTTOM_END); the backdrop dismisses and clears the filter.
2. Filter matching rows. A quick find can be page-bounded, but a true **filter** reads the run's
   **snapshot** (P2-01) so it spans the whole result, not just the visible page; show an active/`on`
   state on the toggle and a ✕ to clear.
3. ⌘F opens it — the keymap landed with P2-20: attach `keymap::on_command(settings,
   Command::Find, …)` on the results scope. The popover's ✕ takes the canvas tooltip
   `Close (Esc)` via `keymap::use_hint_title("Close", Command::Cancel)`.

## Acceptance
- [ ] Search button toggles a popover; typing filters the visible rows; dismiss clears the filter.

## Freya / references
- Freya `Popup` / `Backdrop` (S29 family). Design: `Results.dc.html` find panel.
