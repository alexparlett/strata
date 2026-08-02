# Chart 01 · `Engine::chart` + chart vocabulary `[core]`

**Workstream:** Chart (Rz2) · **Status:** ✅ · **Depends on:** nothing open (P2-01 snapshots ✅)

## Goal
The data half of the chart, engine-side: a `ChartQuery` → `ChartData` vocabulary in `strata-model`
and an `Engine::chart(snapshot, query)` that runs the grouped / binned / raw read **in DataFusion
over the snapshot table** and returns a chart-ready model. Spec: `docs/CHART_SPEC.md` §5.

## As built

- **`strata-model/src/chart.rs`** — `AggFn` (7 fns) and `Measure { y, agg_fn }` with `label()`;
  `Stride` (`parts()` → the `(months, days, nanos)` interval, `wider()` → the ladder, `Display` →
  the SQL interval literal); `Width` (an `f64`'s bits behind a validating constructor); `Bucket`
  (`Time` | `Width`); `ChartQuery`; `ChartData`.
- **`strata-core/src/engine/chart.rs`** — `run_chart`: one group slot, a measure list, one pivot.
  DataFrame API throughout.
- **`Engine::chart`** — spawned on the engine runtime like `fetch_page`; no lifecycle bookkeeping
  and no confirm, because a `GROUP BY` over a local snapshot is `fetch_page`-tier work.
- Tests: 27 unit cases in `engine::chart` over in-memory fixtures (the answer matrix), 7 in
  `tests/engine_chart.rs` through the facade over a **real spooled snapshot** (the IPC round trip,
  every shape, a retired snapshot), 6 in `strata-model` (the stride table, the width gate).

## Corrections this task settled — do not re-litigate

- **`min(row_number() OVER ())` does not give snapshot order, and the spec's proposal is
  withdrawn.** Measured, not reasoned: an Arrow *File* scan range-splits across `target_partitions`
  once the file passes `datafusion.optimizer.repartition_file_min_size` (10 MB), and a window with
  no `PARTITION BY` then sits above a `CoalescePartitionsExec` ("no guarantees are made about the
  order of the resulting partition"). On stock config a 200k-row snapshot came back in perfect file
  order and a 3M-row one put 2 975 424 of 3 000 000 rows out of it — i.e. the property holds at
  every test size and reverses on exactly the results the chart exists for. **Categorical order is
  the measure descending, ties by label ascending**, the fallback the spec named; temporal and
  numeric axes order by value and never face the question. Full reasoning is in the `engine::chart`
  module header, which is where anyone tempted to "fix" it will be standing.
- **A binned numeric X groups on the bin *index*, not on `floor(x / w) * w`.** Identical buckets —
  the start is `index × w`, which is what the scaffold writes — but the group key is an integer, so
  filling the empty bins is exact. Matching returned bucket starts against a generated sequence
  compares floats that agree mathematically and differ in their last bit.
- **A bin width is a `Width`, not an `f64`.** `ChartQuery` is cache identity and identity may not be
  approximate, so the width is the float's own bits behind a constructor that refuses zero,
  negative, NaN and infinity — the impossible widths have no representation rather than a check.
- **The auto stride is widened until the axis fits the cap.** The spec's ladder alone hands back an
  hourly axis for a 60-day span — 1 440 buckets against a cap of 1 000 — so the default would refuse
  by construction. A stride the *request* names is never widened. A numeric X is **not**
  auto-binned: grouping by value is the honest default and a width is something the user turns on.
- **The bucket is resolved engine-side and reported back** (`ChartData::Grouped.bucket`) rather than
  resolved in the UI before the request is built. The request stays concrete and cacheable either
  way (`bucket: None` is a value, and the snapshot is immutable), and the alternative needed a
  second engine entry point purely to ask a column's span.
- **A bucket of the wrong kind for the column is refused**, per the spec — a stale width on a
  temporal X errors rather than being ignored.
- **`group_cap` bounds aggregate rows — categories × series** — not categories alone. One budget,
  one `LIMIT cap + 1`, and no second cap on series cardinality to invent a default for. A bucketed
  axis is capped a second time on the buckets it would *span*, since two rows a decade apart are
  two aggregate rows and 87 600 hourly buckets.
- **A refusal is `ChartData::OverCap`, not a `capped` flag beside a payload** — a truncated chart
  is not a state that can exist. `group_count` is likewise gone: `categories.len()` is the group
  count, and the banner (§7) reads it.
- **Groups are keyed by the value, labelled separately.** A NULL group reads `(null)` and so does a
  column that genuinely holds that string — keying on the rendering would silently merge them.
- **A non-numeric measure is refused, not cast.** Arrow's default cast is lenient, so `min` over a
  text column (the one measure DataFusion plans happily for a non-numeric Y) would otherwise come
  back as a chart of empty cells rather than as the encoding error it is.
- **`date_bin` takes neither `Date32` nor `Date64`**; both are cast to `Timestamp(Millisecond)`
  first. (`Date32` happens to coerce and `Date64` does not, so relying on coercion would make a
  date column work or fail by its width.) `Time32`/`Time64` are deliberately not bucketed.
- **X with no column is one category, `all`.** Whatever splits the chart — a series column, the
  measure list, both — names itself in the legend, so the single axis tick says only what it covers.

## What the adversarial review changed (all fixed, all pinned by a test)

Ten isolated lenses over the finished change; the ones that landed are worth keeping because
several are invisible until production scale or production data.

- **A NaN measure broke the sort outright.** `partial_cmp(..).unwrap_or(Equal)` makes NaN compare
  equal to every real weight while those weights still order among themselves — intransitive, so
  `sort_by` returns garbage below 20 elements and **panics** above it (measured: 21 categories with
  one NaN abort the read). Both comparators now use `total_cmp` with NaN placed last.
- **The auto stride widened on the bucket count while the cap counts categories × series**, so a
  default temporal chart with a series column refused by construction. The bucket now widens on the
  *answer*, retrying one rung up, and the budget is checked three times — returned rows, filled
  axis, and the cell product — because each can overrun without the others noticing.
- **`date_bin` overflows outside the nanosecond window**, and `9999-12-31` is the ordinary
  "still current" sentinel: a release build returned an opaque Arrow message and a debug build
  **panicked**. The span pass now runs for every temporal X and refuses out-of-range dates by name.
- **One NaN killed the histogram.** Arrow's `max` reports NaN as the largest value, so the width
  became NaN and the strict cast failed the read. Non-finite values are filtered from both passes.
- **`count_all()` is aliased to the literal `count(*)`**, which collides with the column
  `SELECT region, count(*) FROM t GROUP BY 1` produces — and two Y-less measures collided with each
  other. Measures are aliased positionally under a prefix derived from the result's own names.
- **A binned numeric axis printed float noise** (`0.30000000000000004` for a width of 0.1) — the
  drift the bin-index keying avoids, moved from the key to the label. Bin starts are rounded onto
  the grid the width defines.
- **`Time32`/`Time64` ranked by measure**, so a time-of-day line ran out of clock order. They now
  take the ordered-by-value axis, which is also where numeric X lives (`Grouping::Ordered`).
- **Labels bypassed `CellFormat`**, so a chart axis and the grid rendered the same value
  differently on stock config, and were unclipped where every other display text clips at
  `DISPLAY_CHARS`. Both fixed; a NULL stays `(null)` per spec rather than the grid's NULL text.
- **`raw`/`histogram` had no type guard**, so a text column failed with an Arrow parse message
  where the aggregate path names the type. One answer now.
- **The read was not pinned** across its two passes, so a re-run between them could yield a
  histogram of real edges and zero counts. `Engine::chart` takes an in-call pin like `export`.
- **`OverCap` now carries the bucket in effect**, so §7's "nudge to a wider bucket" is buildable.
- Doc corrections: `Points` is explicitly **unordered** (the change's own measurement disproves the
  guarantee it claimed), `AggFn` does **not** come from the live registry, and `CHART_SPEC.md` §11's
  acceptance no longer demands the snapshot order §5 withdrew.

**Deliberately not fixed, and why** — both are judgement calls rather than defects:

- **`LIMIT cap + 1` bounds the answer, not the work.** DataFusion's hash aggregate materializes
  every distinct group before the limit sees a row (`LimitedDistinctAggregation` is disabled
  whenever an aggregate expr is present, which is always here), and the default memory pool is
  unbounded — so `x = id` over a 10M-row snapshot builds a 10M-entry table and then discards it.
  A distinct pre-pass would bound it at O(cap) and early-terminate, at the cost of a second scan on
  **every** chart. Open question for 02/04, alongside whether the chart should have a confirm tier.
- **`AggFn::Median` is exact** where `profile.rs` deliberately uses `approx_percentile_cont`. Same
  memory shape, and the chart has no confirm in front of it. Changing it changes what the number
  means, so it wants a decision rather than a patch.

## Out of scope
Trendline/overlays (05), and the algebra's window / derived slots with them. Any UI. Any confirm
gating — this is `fetch_page`-tier, not profile-tier.

## Acceptance
- [x] `Engine::chart` answers all three query shapes over a real snapshot with correct values,
      order, gaps, `(null)` groups, and honest cap reporting; core tests cover the list above.

## Wiring notes for 02–04
- The freya-query capability is keyed `(SnapshotId, ChartQuery)` and built in **one** place.
- 02's renderer must treat `ChartData::OverCap` as "draw nothing" and `None` cells as gaps.
- 03's bucket control reads `ChartData::Grouped.bucket` for what is currently in effect and sets
  `ChartQuery::Aggregate::bucket` to override; `Stride::wider()` from `Stride::Minute` enumerates
  the temporal options, and a numeric width goes through `Width::new` (which is also the control's
  validation). The core charts send exactly **one** `Measure`.
- 04's scaffold writes `Stride`'s `Display` as its `interval '…'` literal and `floor(x / w) * w`
  for a numeric width, so the scaffolded query buckets exactly as the chart did.
- 05 adds measures to the same request (box plot, candlestick) and extends `ChartQuery` with the
  window / derived slots; series come back measure-major in request order, which is how a preset
  reads its parts back.

## References
`docs/CHART_SPEC.md` §3–§5, §7 (cap defaults). `docs/CHART_FUNCTIONS.md` §2 (the query algebra
this vocabulary is the chassis for). `docs/reference/ENGINE.md`.
`engine/query.rs` (`read_page`, `snapshot_name`), `engine/export.rs` (`select_sql`).
