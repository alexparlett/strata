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

**From P3-11 via P3-12, and now with P3-13 built:** the drawer header already has the title, the
count, expand/restore and collapse ×. The **Clear** button and its Events/History-only rule are in
`drawer/mod.rs`; **Events' half is wired** (P3-13) and History's is still **parked**
(`enabled(false)`) — it needs a `clear_history` in `strata-core::project` that truncates
`history.jsonl` as well as emptying the satellite, which does not exist yet. Give the button an
`on_press` and widen the `enabled(..)` condition; nothing else at the call site changes, and
Events is the worked example — including the fact that `enabled` reads the mounted body's
`DrawerCount` rather than the store a second time. The header's **count** is that same
`DrawerCount` (`State<usize>`), which the shell owns and the mounted body writes (see P3-12) —
write the history length into it and reset it on unmount, as `Problems` and `Events` do. The
shared **frame** is `drawer/frame.rs`: `DrawerBody` (scroll container) and `DrawerEmpty` (centred
glyph + copy); colours come from the `drawer` component theme, which this task extends rather than
duplicating (P3-13 added `divider_fill`, the in-list hairline).

**The recorder is already out of the view** (correcting an earlier note here): it lives in the
tab's request keeper (`views::keeper`), mounted for the press's whole life at `ProjectRoot`, so a
run that settles while the user is on another tab is recorded at its real completion time. P3-13
hung the event log's own settle observer beside it. One edge remains for both: a settle landing in
the same update pass that unmounts its pin goes unrecorded.

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
