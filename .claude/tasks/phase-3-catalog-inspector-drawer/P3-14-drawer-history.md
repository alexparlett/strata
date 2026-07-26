# P3-14 · Drawer — History tab

**Phase:** 3 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** U10 · **Depends on:** P3-11, P3-12, P4-14

## Goal
Query history in the History tab.

## Current state
The **view** is not built; the **store is** (P4-14 ✅ — `state/history.rs`). Correcting this file's
earlier description: history is a **satellite**, not Radio and not on `SessionState` — a
`State<History>` in context (`HistoryCtx`), persisted append-only to `.strata/history.jsonl`
(IO in `strata-core::project`), newest-first in memory, capped by `Settings::max_history` and
deduped by `RunId`. Only successful data runs are recorded (a failed/cancelled run or an Explain
never reaches it).

**From P3-11 via P3-12:** the drawer header already has the title, the count, expand/restore and
collapse ×. The **Clear** button and its Events/History-only rule are in `drawer/mod.rs`, shipped
**parked** (`enabled(false)`) — it needs a `clear_history` in `strata-core::project` that truncates
`history.jsonl` as well as emptying the satellite, which does not exist yet. Give the button an
`on_press` and an `enabled(..)`; nothing else at the call site changes. The header's **count** is a
`DrawerCount` (`State<usize>`) the shell owns and the mounted body writes (see P3-12) — write the
history length into it and reset it on unmount, as `Problems` does. The shared **frame** is
`drawer/frame.rs`: `DrawerBody` (scroll container) and `DrawerEmpty` (centred glyph + copy);
colours come from the `drawer` component theme, which this task extends rather than duplicating.

**Known defect to fix here:** the recorder lives *inside* `ResultsBody`
(`state/history.rs::use_history_recording`), which only the active tab mounts. A run that settles
while the user is on another tab is not recorded then — it is recorded when they come back, and
if they never do, the successful run never enters History at all. P3-12 fixed the *timestamp*
half (`ts_ms` now comes from `settlement_instant`, not from the clock at record time); moving the
recorder out of the view needs a per-tab observer and belongs with this task, which owns History.

## Build
- List past queries newest-first (meta · line-count badge · timestamp, per the canvas).
- **Click to load** into the editor (`onLoadHistory`); **double-click to load & run** (`onRunHistory`)
  — matches the Strata canvas history rows.
- Reuse P3-12's scroll container + empty state (the canvas's clock icon over "No queries run yet").
- **Clear** empties the satellite *and* truncates `history.jsonl`. P3-12 owns the button's
  show/hide rule.

## Acceptance
- [ ] Past queries list, capped at `max_history`; click loads into the editor; double-click loads + runs.
- [ ] Clear empties the history.

## Freya / references
- The `History` satellite (`state/history.rs` → `.strata/history.jsonl`). Canvas `onLoadHistory` /
  `onRunHistory`. Design: `DrawerHistory.dc.html`. `Settings::max_history` cap.
