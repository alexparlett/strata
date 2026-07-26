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

**From P3-11:** the drawer header already has the title, expand/restore and collapse ×. **Clear**
needs a `clear_history` in `strata-core::project` that truncates `history.jsonl` as well as
emptying the satellite — it does not exist yet. The Clear button's Events/History-only rule lands
with P3-12.

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
