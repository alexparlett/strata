# P2-09 · Find in results

**Phase:** 2 — Workbench · **Status:** ✅ **built** · **DEV_TASKS:** U6c · **Depends on:** P2-03

## Goal
A collapsible find popover over the results, opened from the toolbar Search button and ⌘F.

## Built
- `results/find.rs` — `FindState` (open + query, owned per-press by `ResultsBody`, threaded as
  props) and `filter_page`, the **page-bounded** filter (Dioxus parity — the snapshot-spanning
  filter was considered and rejected: paging keeps walking the unfiltered snapshot). Any cell's
  display text, case-insensitive; survivors keep their absolute gutter numbers (gaps, not
  renumbering); unit-tested.
- The popover: `Attached` (bottom-end) off the toolbar Search trigger, on the `Menu` base whose
  chrome *is* the panel — a chrome-less mono `Input` (auto-focus) filling it, magnifier leading,
  ✕ beside it (not in the input's `trailing` — the input's focus-press `prevent_default` would
  swallow the press). No match-count label (dropped by request). Every dismissal path (backdrop,
  Esc, ✕, trigger toggle-off) funnels through `FindState::dismiss`, which clears the filter. The
  trigger wears the comp's accent `on` dress while open.
- ⌘F toggles via `keymap::on_commands` on the grid root (the results scope), where Esc
  arbitrates: dismiss the popover first, then fall through to clearing the selection. The ✕ is
  the flat tab-close-style icon button, no tooltip. A query change clears the page-local
  selection (same invariant as a pager jump).

## Acceptance
- [x] Search button toggles a popover; typing filters the visible rows; dismiss clears the filter.

## Freya / references
- Freya `Popup` / `Backdrop` (S29 family). Design: `Results.dc.html` find panel.
