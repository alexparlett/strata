# Chart 10 · Tier B marks — heatmap, error bands, box plot

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 01–04 · Before 11 only for
strip layout (11 adds a toggle; this task adds tiles). Each mark is independently shippable
— land heatmap first.

## Goal
Three analytical marks as **column-role mappings over SQL the user owns** (spec §10): a
heatmap (two group columns + a measure), error bands (`y`/`y_lo`/`y_hi`), a box plot
(median/q1/q3/whiskers). Each is a role mapping + a renderer + the palette template that
writes its SQL — never an engine computation. Settled in planning (2026-08-07, Alex):
approved; new `ChartMark` variants, not a separate preset enum.

## Current state
- `ChartMark { Bar, Line, Area, Scatter, Histogram, Pie }` + `ALL` in
  `strata-model/src/chart.rs`; the strip's tile grid iterates `ChartMark::ALL` three per
  row (`strip.rs`), icons in `components/icon.rs`.
- `encode` (`chart/config.rs`) is the one `ChartQuery` construction site; per-mark option
  sets make invalid encodings unreachable. `resolve` merges defaults under choices with
  read-time fallback, never a write-back.
- The `Rows` read + long→wide pivot (`strata-core/src/engine/chart.rs`) refuses two rows in
  one `(x, series)` cell — for a heatmap matrix that refusal is correct by construction.
- Every Tier B mark is rects/lines/circles/polygons — nothing new from
  `freya-plotters-backend` (`fill_polygon` via pie is already the most exotic path).
- 07 establishes `chart/templates.rs` and the palette command pattern; if 07 has not
  landed, build the templates module here with the same shape (whichever lands second
  merges).

## Build

**Vocabulary** — `Heatmap`, `Band`, `Box` appended to `ChartMark::ALL` (9 = 3 clean tile
rows), `label()`s, icons + glyphs. **Serde-compat check first**: confirm a `TabSnapshot`
carrying an unknown mark degrades **per-tab**, not whole-session, in
`QueryTab::restored`/`from_snapshot` — record the finding in this file.

**Heatmap** (first — zero new config fields):
- Roles: X = first group column, `series` = second group column, `ys[0]` = the measure.
  The existing pivot **is** the matrix: axis labels = X categories, one `ChartSeries` per
  second-group value, `None` = empty cell.
- `config.rs`: `x_options = categories`, `allows_row_index = false`,
  `takes_many_ys = false`, series **required** = categories minus X; `encode` →
  `Rows { x, ys: [y], series: Some(s), cap: ROWS_CAP }`, errors "Pick two category columns"
  otherwise. `sortable = true` (`ByX` is meaningful on a matrix).
- `marks.rs`: filled rects on a grid, both axes the existing `Categories` `Ranged`, series
  names on Y; cell color = value normalized over the finite min/max through a sequential
  ramp.
- Theme: the ramp is new `chart` theme fields (`heat_lo`/`heat_hi` or a stop list) — new
  roles in the closed vocabulary, then `UPDATE_SCHEMA=1 cargo test -p strata-freya
  schema_in_sync`. Worth a quick dress check against the design bundle before naming them.
- `mod.rs`: `legend()` arm (min/mid/max swatches), `notice()` arm for an all-empty matrix.
- Palette command `chart_heatmap` + template (`SELECT a, b, count(*) FROM … GROUP BY 1, 2`
  with an `ORDER BY`).

**Error bands**:
- `ChartConfig` gains `#[serde(default)] y_lo: Option<String>`, `y_hi: Option<String>` —
  intent, read-time fallback like every reference; **no schema-derived default** (a role
  from a column name is ruled out) — unset is an `encode` error naming the template
  ("Pick the band's bounds. The Error band template writes them.").
- `Encoding` carries them; `encode(Band)` → `Rows { x, ys: [y, lo, hi], series: None }` —
  fixed order, the renderer reads by position. Option sets: measures, excluding columns
  already on another band role (duplicates unreachable, not reported).
- `strip.rs`: LOWER/UPPER `Encoder` rows, shown only when the mark's option set is
  non-empty (the X/series conditional pattern).
- `marks.rs`: translucent polygon (upper path + reversed lower) under the centre line;
  NULL in any of the three at a category cuts the run (reuse `runs`). `legend()`: one
  entry — the centre series.
- Command `chart_error_band` + template (`avg(y)`, `avg(y) - stddev(y)`,
  `avg(y) + stddev(y)` — or percentile bounds — over a grouped X).

**Box plot**:
- Reuses `y_lo`/`y_hi` as whiskers; adds `#[serde(default)] q1/q3: Option<String>`.
  Median = `ys[0]`. `encode(Box)` → five fixed-order ys, all required, one message naming
  the template. `x_options = categories`, no row index.
- `marks.rs`: per category — whisker line lo→hi, rect q1→q3, median tick. All rects/lines.
- `sort.rs` note: `ByYDesc` sorts by the first series = the median — correct by the ys
  ordering; say so in a comment where the order is fixed.
- Command `chart_box_plot` + template (`percentile_cont(0.25/0.5/0.75) WITHIN GROUP
  (ORDER BY y)` + `min`/`max` per category).

**Deliberately deferred** (recorded in 05): candlestick (OHLC fields — the box pattern
again), ECDF/Pareto as template-only over Line until proven to need marks, indexed
comparison, period delta.

## Acceptance
- [ ] Each mark renders from columns its template produced; each template opens unrun and
      runs against a fixture; duplicate/over-cap refusals behave unchanged.
- [ ] Roles fall back at read time (a vanished `y_lo` disables the band with the encode
      message; it returns when the column does); flipping marks never loses config.
- [ ] `schema_in_sync` green after the ramp fields; preview harness gains
      heatmap/band/box fixtures; `config.rs` tests cover the new option sets and errors.

## References
`docs/CHART_SPEC.md` §4, §10. `docs/CHART_FUNCTIONS.md` §2–3 (Tier B).
`05-analytical-presets.md`. `07-tier-a-templates.md` (templates module + command pattern).
