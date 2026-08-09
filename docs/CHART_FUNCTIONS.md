# Shaping a result for the chart in SQL

The chart maps result columns onto marks and computes nothing SQL can say
(`docs/CHART_SPEC.md` §1.2) — so the shape of the chart is the shape of the query. This file is
the practical reference for writing that query: which function families the engine offers, and
what chart shape each one buys, with an example where it helps.

The enumeration was read from the **pinned DataFusion 54.0.0 sources** (`all_default_*_functions`
in each functions crate), not from the upstream docs pages, which track a newer version. The live
registry is always the truth (`docs/reference/ENGINE.md`).

Two facts about the chart worth keeping in mind throughout:

- **Result order is the axis order.** Rows draw in the order the query produced them, and a
  `GROUP BY` has no output order of its own — so a grouped query that should read left-to-right
  ends with an `ORDER BY`.
- **Columns are the encoding.** Several aggregate columns are several series with no
  configuration; a category column picked in the Series encoder pivots long→wide. A preset shape
  (candlestick, box plot, error band) is just columns in named roles.

---

## 1. One value per category — `GROUP BY` + aggregates

The bread and butter behind bars, lines, areas and pies — and the fix the chart's own refusals
name: over the row cap, or with two rows in one pivot cell, the answer is always a `GROUP BY`.

Reducers: `sum`, `avg`, `min`, `max`, `count` (and `count(*)` when there is no measure);
`median` / `approx_median` as the skew-honest centre.

```sql
SELECT country, sum(revenue) AS revenue
FROM sales
GROUP BY country
ORDER BY revenue DESC;
```

Two aggregates are two series (`sum(revenue), sum(cost)`); keeping a second grouping column in
the SELECT list and choosing it in the Series encoder splits one measure into a series per value:

```sql
SELECT month, region, sum(revenue) AS revenue
FROM sales
GROUP BY month, region
ORDER BY month;          -- X: month, Y: revenue, Series: region
```

`FILTER` splits a measure without a second grouping column:

```sql
SELECT month,
       sum(amount) FILTER (WHERE status = 'paid')    AS paid,
       sum(amount) FILTER (WHERE status = 'refunded') AS refunded
FROM invoices GROUP BY month ORDER BY month;
```

Subtotals in the same pass: `ROLLUP` / `CUBE` / `GROUPING SETS`, with `grouping()` telling the
result which rows are the subtotals.

## 2. Time series — `date_bin`, `date_trunc`, `date_part`

`date_bin(stride, ts, origin)` is the temporal bucketing mechanism — an even stride from an
origin, which keeps buckets comparable across the whole range; `date_trunc` snaps to calendar
units instead.

```sql
SELECT date_bin(INTERVAL '1 day', created_at, TIMESTAMP '2024-01-01') AS day,
       count(*) AS n
FROM events
GROUP BY day
ORDER BY day;
```

A temporal X defaults the mark to a line, and the axis places buckets at their true positions —
an irregular series draws with its real gaps.

`date_part` / `extract` pull cycle components for seasonality: `extract(dow FROM ts)` as X and
`extract(hour FROM ts)` as a series is a weekly cycle in one `GROUP BY` over two parts.

Rescuing columns that are temporal in meaning but not in type (a Utf8 timestamp, an epoch
number): `to_timestamp*`, `from_unixtime`, `to_date`. The chart derives a column's role from its
Arrow type, never its name — the cast in SQL is what makes a string chartable as time. `to_char`
formats a bucket into a label engine-side when the display format isn't what's wanted.

Note a stride constraint the role system mirrors: `date_bin` with a day-or-wider stride is
refused over a `Time` column (a time of day has no calendar under it) — which is why the chart
distinguishes instants from clock times (`docs/CHART_SPEC.md` §3).

## 3. Distributions

The **histogram mark** bins engine-side already (the one computed mark) — reach for SQL when its
equal-width bins are the wrong reading:

- **Percentile summaries** — `percentile_cont(p) WITHIN GROUP (ORDER BY x)` computes exact
  percentiles (p25/p50/p75 in one pass makes the columns of a box-plot-shaped result);
  `approx_percentile_cont` (and `…_with_weight`) for very high row counts.

  ```sql
  SELECT region,
         percentile_cont(0.25) WITHIN GROUP (ORDER BY latency) AS p25,
         percentile_cont(0.50) WITHIN GROUP (ORDER BY latency) AS p50,
         percentile_cont(0.75) WITHIN GROUP (ORDER BY latency) AS p75
  FROM requests GROUP BY region;
  ```

- **Equal-count bins** — `ntile(n) OVER (ORDER BY x)` for decile/quartile summaries, the
  complement of equal-width bins for skewed data.
- **ECDF** — `cume_dist() OVER (ORDER BY x)` charted as a line is a cumulative distribution,
  often the honest replacement for a histogram; `percent_rank` is its rank-based sibling.
- **Numeric bins by hand** — `floor(x / w) * w` groups a numeric X into width-`w` bins (there is
  no `width_bucket` in DataFusion 54); `floor(log10(x))` buys decade buckets for heavy-tailed
  data (`log`, `log2`, `log10`, `ln`, `power` are all present).

## 4. Running, cumulative and comparative lines — window functions

Any aggregate is also a window function via `OVER`, which is where most of the line-chart
vocabulary comes from:

- **Moving average** — a frame: `avg(y) OVER (ORDER BY x ROWS BETWEEN 6 PRECEDING AND CURRENT
  ROW)`.
- **Running total** — `sum(y) OVER (ORDER BY x ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT
  ROW)`.
- **Period-over-period** — `lag(y)` / `lead(y)`: delta and growth-rate series over a bucketed
  axis.

  ```sql
  SELECT day, n,
         (n - lag(n) OVER (ORDER BY day)) * 100.0 / lag(n) OVER (ORDER BY day) AS growth_pct
  FROM daily GROUP BY day, n ORDER BY day;
  ```

- **Indexed comparison** — normalize each series to its first bucket so different-scale series
  compare on one axis: `y / first_value(y) OVER (PARTITION BY series ORDER BY x) * 100`
  (`first_value` / `last_value` / `nth_value` all have window forms).
- **Share of total** — `sum(y) OVER (PARTITION BY x)` or `OVER ()` for percent-of-whole:
  100%-stacked readings and pie percentages, computed where the data is.

`row_number() OVER (ORDER BY …)` numbers rows explicitly. It is not needed for order itself —
result order is already the chart's axis order, carried by the snapshot ordinal
(`docs/SNAPSHOT_SPEC.md` §9) — and `row_number() OVER ()` with no `ORDER BY` follows scan order,
which is nondeterministic on large results.

## 5. Spread and relationships

- **Error bands** — `stddev` / `stddev_pop` / `var_samp` / `var_pop` beside an `avg` in the same
  `GROUP BY` gives `y`, and `avg ± stddev` gives `y_lo` / `y_hi` — three series on one axis.
- **Trendline numbers** — the regression family computes an honest fit in-engine:
  `regr_slope`, `regr_intercept`, `regr_r2` (plus `regr_avgx/avgy/count/sxx/syy/sxy`);
  `corr` and `covar_samp` / `covar_pop` quantify the relationship without drawing it. The chart
  draws no fitted line itself (`docs/CHART_SPEC.md` §8), but the fit's endpoints are one query
  away.
- **OHLC-shaped results** — the order-sensitive aggregates `first_value(y ORDER BY x)` and
  `last_value(y ORDER BY x)` beside `min` / `max`, grouped by a `date_bin` bucket, make
  open/high/low/close columns in a single `GROUP BY`.

## 6. Taming cardinality

The chart refuses over its caps and banners a crowded axis rather than sampling or truncating —
the constructive answers live in SQL:

- **Top-N + Other** — rank categories by measure and fold the tail:

  ```sql
  WITH ranked AS (
    SELECT country, sum(revenue) AS revenue,
           rank() OVER (ORDER BY sum(revenue) DESC) AS r
    FROM sales GROUP BY country
  )
  SELECT CASE WHEN r <= 10 THEN country ELSE 'Other' END AS country,
         sum(revenue) AS revenue
  FROM ranked GROUP BY 1 ORDER BY revenue DESC;
  ```

- **Explicit sampling** for scatter over huge results — `WHERE random() < 0.01`. There is no
  `TABLESAMPLE` in DataFusion 54, and the chart never samples on its own, so a sample is always
  visible in the query that made it.
- **Cardinality preflight** — `approx_distinct(col)` (HyperLogLog) answers "how many series
  would this split make" cheaply; `count(DISTINCT col)` is the exact form.
- `array_agg` / `string_agg` collect bounded per-group examples when a label needs to carry a
  sample of its members.

## 7. Gaps, zeroes and missing buckets

A NULL Y cell draws as a **gap** — a line is cut, never interpolated. That is the honest default
for a bucket with no rows, but "no rows" and "zero" are different claims, and the choice belongs
in the query:

```sql
WITH calendar AS (
  SELECT unnest(generate_series(TIMESTAMP '2024-01-01', TIMESTAMP '2024-03-31',
                                INTERVAL '1 day')) AS day
)
SELECT c.day, coalesce(sum(e.amount), 0) AS amount   -- 0 = "measured, nothing happened"
FROM calendar c
LEFT JOIN events e ON date_bin(INTERVAL '1 day', e.at, TIMESTAMP '2024-01-01') = c.day
GROUP BY c.day ORDER BY c.day;
```

`generate_series` produces the full bucket calendar; the LEFT JOIN makes an empty bucket an
explicit row; `coalesce` (or its absence) states whether that row is a zero or a gap. `nullif`
goes the other way when a sentinel value should become a gap.

## 8. Nested and JSON columns

A nested column (struct, list, map, union) has no axis to sit on — the encoders never offer it.
Flattening it is SQL:

- **`unnest`** turns a list column into rows (one mark per element) or a struct column into its
  fields.
- **JSON accessors** — the engine registers `json_get` and the `->` / `->>` operators over Utf8
  columns holding JSON text; `->> 'price'` extracts a field, and a cast makes it a measure.
- `concat` builds a composite category key (two columns as one X, or one series label)
  engine-side, so the chart's pivot stays a reshape.
