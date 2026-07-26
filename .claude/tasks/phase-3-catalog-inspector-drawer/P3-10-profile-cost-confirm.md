# P3-10 · Profile-cost confirm

**Phase:** 3 · **Status:** ✅ · **DEV_TASKS:** U15 · **Depends on:** P3-09

## Goal
Confirm before a first profile scan; re-scans skip it.

## As built

`views/dialogs/profile_confirm.rs`, mounted at the window root after the close and drop confirms
(that order is the order their questions outrank each other: a running query, then a destructive
catalog change, then work the user is about to start). Warning-toned header chip, the action over
its subject (`Profile table` over a mono `events`), the body copy, then Cancel + an accent
**Run scan**. Esc cancels, Enter runs — the shared `Dialog`'s barrier.

**One entry point.** `ProfileActions::ask` is what every trigger calls — the inspector's scan card
and both row menus' items. It raises the dialog when the entry has no request and calls
`ProfileActions::start` directly when it has one, so a ↻ re-scan (and a retry after a failure)
never asks twice. The decision itself is `needs_confirm(&ProjectState, kind, name)`, pure over the
store so it is tested without a window. Confirming calls the same `start` — the dialog is a gate in
front of one path, not a second copy of it.

**No cost figures, and no `>50 files` gate** (DEV_TASKS D4/U15). The canvas quotes "248 files ·
~186 MB" off a file-count threshold; file count is a backwards proxy (one 10GB Parquet file trips
nothing, sixty small ones trip it) and we measure no bytes at all, so any figure would be a guess
wearing a decimal point. The copy states the shape of the work, which is true at every size: it
reads everything once, distinct counts cannot be merged so there is no cheaper form, and the result
is cached until the entry changes. A **view** says the same three things in its own terms — its
whole query rather than a file read, and invalidated by a table it reads changing too (D10).

`start` also **reveals** the entry: a scan asked for from a catalog row would otherwise run out of
sight, so the inspector opens on the entry's first column (one scan covers every column, so any of
them shows that it ran). Asked for from the inspector's own card, nothing moves. An entry whose
schema hasn't landed keeps the selection it had rather than pointing the panel at nothing.

## Acceptance
- [x] First profile shows the confirm; re-scan does not; the copy describes the work, not a file count.

Pinned by `a_re_scan_needs_no_confirm` (including that invalidation puts the question *back*, since
the numbers are gone) and by `the_confirm_describes_the_work_and_quotes_no_figures`, which asserts
the body carries no ASCII digit at all.

## Freya / references
- The shared `components::dialog::Dialog`. DEV_TASKS U15 / D4 (the "no cost figures" decision).
  Design: the `profile` tile on the dialogs canvas.
