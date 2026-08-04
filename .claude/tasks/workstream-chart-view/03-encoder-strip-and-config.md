# Chart 03 · Encoder strip + `ChartConfig` state

**Workstream:** Chart (Rz2) · **Status:** ✅ · **Depends on:** 02

## Goal
The left control strip (X / Ys / Series / Sort) and the persisted per-tab `ChartConfig` that
drives the chart. Spec: `docs/CHART_SPEC.md` §3, §4, §6.

## What was built

**State.** `ChartConfig { mark, x, ys, series, sort }` (serde, `strata-model::chart`) on
`QueryTab` under **`Chan::Chart(tab)`** (derives `Persist`, like `View`), persisted via
`TabSnapshot::chart`. It holds **intent**, never a resolved read — every channel can say "not
chosen", and X is a three-state `ChartX { Auto, RowIndex, Column }` because "not chosen" and
"chosen to be the row index" are different answers: an `Option<String>` would let the next
result's date column overrule a deliberate row-index axis. `QueryTab::restored` now takes the
whole `TabSnapshot` rather than a positional list that grows with each persisted facet.

**Resolution** (`results/chart/config.rs`, new). `Roles` keeps the result's chartable columns in
result order with their `ChartRole`; the per-mark **option sets** (`x_options`, `y_options`,
`series_options`, `allows_row_index`, `takes_many_ys`, `sortable`) are spec §4's table as
functions; `resolve` merges the schema's defaults **under** the user's choices and drops any
reference this result cannot answer; `encode` stays the one `ChartQuery` construction site.

The fallback is **read-time, never a write back**: a column that disappears from one result and
returns in the next brings the user's choice back with it. Same rule narrows rather than spends —
a pie draws one of four Ys and the config still holds all four, so switching back is free.

**Strip** (`results/chart/strip.rs`). Mark tiles, then X / Y / Series as app-standard `Select`s
and the sort as a `SegmentedToggle`, each appearing only where its channel means something for
the mark (a histogram has no X and no series; neither scatter nor histogram has a sort). One
write funnel (`commit`) on `Chan::Chart(tab)`; each control carries the whole `ChartConfig` it
commits, resolved in the strip's own render where the rules are.

**The Y multi-pick is a plain `Select`.** A row that takes several picks calls
`e.prevent_default()`, which cancels the queued `GlobalPointerPress` the `Select` closes on —
non-capture globals are emitted last (`EventName::priority`) and `PointerPress`'s cancellable
set names `GlobalPointerPress`, so the close is removed before it is handled. The select's
other closer, a focus-within test, holds because focus never leaves the trigger: `MenuItem`
requests focus only on the **unprevented** path, so the multi-pick row does not take it. (An
earlier version of this note had that backwards — the behaviour was right and the stated
mechanism was not, which is why both halves are pinned by tests rather than by reading:
`the_y_list_stays_open_across_picks_and_keeps_result_order` and
`a_single_pick_encoder_closes_on_the_pick`.) No hand-rolled dropdown.

**Sort** (`results/chart/sort.rs`, new) is a view transform over the settled `ChartData::Table` —
never in `ChartQuery`, so flipping it repaints without a re-read. Its comparator takes a
`descending` flag rather than being reversed at the call site: reversing it reverses where the
**gaps** go, which put every NULL and NaN at the head of a value-descending chart the first time
it was written (caught by the test, not by review).

**One fork change** (`crates/freya`, AGENTS.md §6 — **needs pushing**). This is the app's first
`Select` fed a *data-derived* list, and the fork's dropdown had no height bound and no scroll: a
`SELECT *` over a 30-40 column parquet opened a list ~1 000px tall, which does not fit above
either, so it stayed open downward and its tail was **unreachable**. `Select` now carries a
themed `list_max_height` (base default 320px, `Size::Inner` to opt out) and puts its items in a
`ScrollView` sized by its content in both axes, so a short list lays out exactly as before. The
cap sits on the scroll, not on the box around it — a capped box that cannot scroll hides its
tail instead of deferring it — and the items keep a `Content::Fit` parent, or `MenuItem`'s
`fill_minimum` grows them to the window instead of to the longest item. Verified visually:
`target/chart-strip-open-wide.png` (40 columns) beside `chart-strip-open.png` (two).

## Acceptance
- [x] Every channel is assignable through the strip; changes re-chart via the subscription;
      config survives restart with the tab; a schema change re-derives defaults cleanly.
- [x] Editing an encoder redraws the chart without waking any other results/editor channel.

## Notes for later tasks
- 04's scaffold should read the resolved `Encoding`, not the stored config — those are real
  column names, already checked against the result.
- The preview harness (`preview.rs`, `#[ignore]`d) now renders the strip over a real schema and
  can click before it shoots: `chart-strip-open.png` is an encoder's open list, which is the one
  part of this surface a static render cannot show.

## References
`docs/CHART_SPEC.md` §6. `state/session.rs` (`QueryTab`, `set_chart`), `state/channel.rs`
(`derive_channel`), `strata-model/session.rs` (`TabSnapshot`).
Design visuals: handoff `Strata.dc.html` control strip (its Aggregate toggle and fn menu are
deliberately absent — spec §1.2/§1.3).
