# P3-04 · Catalog validity indicators

**Phase:** 3 · **Status:** ✅ · **DEV_TASKS:** D11 · **Depends on:** P3-02

## Goal
Flag invalid tables/views with a warning triangle (hover = reason).

## Current state
Built. Validity is **derived on read** off the `ProjectState` store — never stored, so there is
nothing to invalidate and it self-heals.

## Build
1. **Tables:** `ProjectState::table_problem` surfaces `Reg::Failed`'s error (missing file, bad
   path). `Loading` is *not* a problem — a re-scan resets every row, and flagging then would put a
   triangle over the whole catalog while it retried.
2. **Views:** `ProjectState::view_problem` surfaces `Reg::Failed` (SQL error / missing base at
   creation) **plus** a derived check — invalid if any of its `ViewInfo::deps` (base tables) is
   absent from the catalog or itself `Failed`. Deps are transitive base tables, so it reaches
   through a view-of-a-view. Dep names fold case (`same_name`), because they come back from the
   planner while def names come from the user.
3. **The row — one trailing status slot, one glyph, words only on hover.** A settled row is clean;
   an unsettled one is either waiting on the engine (a 12px `CircularLoader`) or broken (a 12px
   `IconName::Warning` in the new `catalog.warn_color` → the `warning` token). Never both: an
   unanswered row is never flagged. The status **text** P3-02 had ("loading…" / "failed") is gone —
   "failed" said strictly less than the reason the triangle already carries, and it cost the name
   half the row. Each glyph wears its message as a `Tooltip` *and* an **`a11y_alt`**, so the
   explanation isn't mouse-only; the a11y label is also what the interaction tests read, which
   makes a whole-list `assert_eq` on the pane's unsettled rows meaningful. The spinner's message is
   *named* ("Loading…") because P3-09 puts a profiling spinner within reach of the same row.
4. **The slot holds still.** One rule: *a settled answer applies immediately; while unanswered the
   slot keeps whatever it last showed, for 400ms; past that the spinner takes it.* Registration is
   metadata-only (`register_external` infers the schema and lists files — no data scan), so a local
   pass lands far inside the window and nothing in the pane moves at all; a partitioned tree of
   thousands of files, or an object store (W7), is what the spinner is for. This is what stops ↻ on
   a broken row **blinking its triangle** off and back on — the store honestly has no verdict
   mid-re-scan, and an empty slot reads as "fine", which is a claim the row can't make. Two
   `use_side_effect_with_deps` do it: one arms a one-shot `Timer` on entry into the wait (the
   cancel-and-rearm shape of `query/validate.rs`'s `SURFACE_DELAY`), one remembers the last settled
   verdict. A row that has been waiting all along keeps its spinner across a re-scan rather than
   blinking — its wait never stopped.
4. **Reactivity:** a view row subscribes to `ProjChan::Tables` as well as `Views` — a table failing
   or being dropped never touches the views channel, and it is exactly what turns a view invalid.

## Acceptance
- [x] A failed table and a view over a missing base both show a triangle with the right reason.

## Freya / references
- `ProjectState::{table_problem, view_problem}` + unit tests (`state/project.rs`); the rendered
  cases in `catalog/interaction.rs`. Note DF-54 truth (dropping a table doesn't break a view until
  reload) — copy per DEV_TASKS D10/D11, spelled out on `view_problem`. Design: the Dioxus
  `.cat-warn` treatment (`--orange`); the canvas doesn't model a broken catalog.

## Left for its own task
- **Fork: make `freya-sdk`'s `use_timeout` fit this job.** It is the natural home for the
  hold-back, but as written it doesn't fit: its task is a fixed-period `loop { Timer::after(d) }`
  poll, so `elapsed()` flips somewhere between 1× and 2× the duration after `reset()` (a 400ms
  hold-back would fire anywhere in 400–800ms), and the loop runs forever per instance — one
  permanent ticker per catalog row, settled or not. Making it precise *and* quiescent needs a wake
  signal on `reset` (the `ReactiveContext::new_for_task` + `rx.notified()` pattern `Effect` already
  uses) — a small but real redesign of a public SDK type, with its own tests. Worth doing; not
  worth widening this task for. When it lands, this call site collapses to
  `use_timeout(|| SPINNER_DELAY)` + `reset()` in the same effect.
- **Column-level flags.** Only entries (tables / views) carry a triangle; a column can't be
  individually invalid in this model.
- **What a drop would break** (P3-05): `view_deps` and the "which views read this table" direction
  are untouched here — this task only answers "is *this* row invalid".
