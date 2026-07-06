# Strata — EXPLAIN Plan View — Design Spec (v3)

**For:** the designer, to lay out the EXPLAIN plan view. Supersedes the v1 mock in
`Strata.dc.html` (`planTree()`) and the pgjson-framed v2 of this doc.

**Read this first.** The engine does **all** the work — it walks DataFusion's own
typed plan objects and live metrics and hands the UI **one plain data structure**
(`QueryPlan`), already parsed, typed, and unit-formatted. There is **no JSON, no
pgjson, no text parsing** on the UI side. So: **design from the data model in
§§1–4.** Everything the UI can show is in there. If the design needs the data in a
different shape (extra fields, grouped metrics, a computed value), the engine can
produce it — that's the whole point of §8.

---

## 1. What the UI receives: `QueryPlan`

One object per `EXPLAIN`, delivered to the view:

```
QueryPlan {
  physical:     PlanNode[]   // the operator tree, flattened, depth-tagged
  logical:      PlanNode[]   // same, for the logical plan
  analyze:      bool         // true = EXPLAIN ANALYZE (physical nodes carry metrics)
  physicalText: string       // raw indent text, for the "Raw" toggle
  logicalText:  string       // raw indent text, for the "Raw" toggle
}
```

- Both trees are **flat arrays in pre-order**, each node tagged with a `depth`
  (0 = root). Render as an indented list; `depth` gives the indent. (Not nested —
  no recursion needed.)
- **Plain `EXPLAIN`** → `logical` + `physical` populated, `analyze=false`, **no
  per-node metrics** (the query isn't executed).
- **`EXPLAIN ANALYZE`** → `analyze=true`, `physical` nodes carry **live metrics**;
  `logical` is still populated (structure only, no metrics).
- Both trees can be shown (Physical / Logical tabs); ANALYZE defaults to physical.

---

## 2. A node: `PlanNode`

What the UI has for each operator today:

```
PlanNode {
  name:   string      // "ParquetExec", "HashJoinExec", "Projection"
  detail: string      // one-line operator config (may be long — see §5.4)
  kind:   Kind        // source | join | exchange | agg | sort | proj | limit | util
  depth:  int         // indent level
  rows:   int | null  // output rows — null when the operator doesn't emit them
  time:   { ms: number, label: string } | null   // see note below
  metrics: string     // ← CURRENTLY a flat "name=value · name=value …" string
}
```

`kind` maps to the accent colour (source `#7ee787` · join `#d2a8ff` · exchange
`#79c0ff` · agg `#ffa657` · sort `#f0a5c0` · proj `#4cc6ff` · limit `#ffcf6b` ·
util `#8b95a3`).

`time` is currently DataFusion's `elapsed_compute` (ms). **This is the wrong
headline** — it's `~0` on scans and absent on joins/exchanges (see §5.2); §7
defines the value we should send instead (per-operator "self-time").

**`metrics` is the problem.** Today it's one pre-joined string of every remaining
metric — the "wall" the design must break up. The engine can instead send a
**structured, typed list** (§8) so the design can tier it. Everything below (§3,
§4) describes the *values* that go in there, however we shape it.

### Example — one real node (the events scan), as data

```
{
  name:   "ParquetExec",
  detail: "file_groups={1 group: [[…/events/year=2024/month=01/data.parquet, …/month=02/data.parquet]]}, projection=[user_id, action, amount], predicate=amount@3 IS NOT NULL",
  kind:   "source",
  depth:  14,
  rows:   7,
  time:   { ms: 0.000001, label: "1ns" },   // elapsed_compute — misleading; real cost below
  metrics: {                                  // shown here structured (the §8 target shape)
    time_elapsed_processing:        { value: 15594334, type: "time",  label: "15.6ms" },
    time_elapsed_scanning_total:    { value: 17147249, type: "time",  label: "17.1ms" },
    metadata_load_time:             { value: 22353002, type: "time",  label: "22.4ms" },
    bytes_scanned:                  { value: 605,      type: "bytes", label: "605 B"  },
    row_groups_matched_statistics:  { value: 2,        type: "count", label: "2"      },
    file_open_errors:               { value: 0,        type: "count", label: "0", zero: true },
    // …~18 more, most zero…
  }
}
```

---

## 3. The full reference dataset (real `EXPLAIN ANALYZE`)

The worst case to design against: a join + group-by + top-N over the sample data —
**18 nodes, depth 0→14, two join branches.** The whole `physical` array as data
(zero-valued metrics elided; times shown with their real units):

| depth | name | kind | rows | time (compute) | notable metrics |
| --- | --- | --- | --- | --- | --- |
| 0 | SortPreservingMergeExec | sort | 4 | 21µs | — |
| 1 | SortExec (TopK fetch=20) | sort | 4 | 156µs | `row_replacements` 4 |
| 2 | ProjectionExec | proj | 4 | 3µs | — |
| 3 | AggregateExec (FinalPartitioned) | agg | 4 | 4.79ms | `peak_mem_used` 3.4KB |
| 4 | CoalesceBatchesExec | util | 4 | 9µs | — |
| 5 | RepartitionExec Hash([country,action]) | exchange | — | *none* | `repartition_time` 29µs · `send_time` 679µs · `fetch_time` 337ms\* |
| 6 | AggregateExec (Partial) | agg | 4 | 4.06ms | `peak_mem_used` 4.2KB |
| 7 | CoalesceBatchesExec | util | 4 | 6µs | — |
| 8 | HashJoinExec (Inner) | join | 4 | *none* | `build_time` 216µs · `join_time` 146µs · `build_mem_used` 2.1KB · `build_input_rows` 4 · `input_rows` 5 · `output_batches` 4 |
| 9 | CoalesceBatchesExec | util | 4 | 15µs | — |
| 10 | RepartitionExec Hash([user_id]) | exchange | — | *none* | `repartition_time` 4.3ms · `send_time` 19µs · `fetch_time` 256ms\* |
| 11 | CoalesceBatchesExec | util | 4 | 40µs | — |
| 12 | FilterExec (amount IS NOT NULL) | util | 4 | 134µs | — |
| 13 | RepartitionExec RoundRobin | exchange | — | *none* | `repartition_time` 1ns · `send_time` 8µs · `fetch_time` 32ms\* |
| 14 | ParquetExec (events) | source | 7 | 1ns | `time_elapsed_processing` 15.6ms · `metadata_load_time` 22ms · `bytes_scanned` 605 · `row_groups_matched_statistics` 2 · +~18 mostly-zero |
| 9 | CoalesceBatchesExec | util | 5 | 16µs | — |
| 10 | RepartitionExec Hash([user_id]) | exchange | — | *none* | `repartition_time` 10µs · `send_time` 7µs · `fetch_time` 4.4ms\* |
| 11 | ParquetExec (users) | source | 5 | 578µs\*\* | `metadata_load_time` 3.2ms · `bytes_scanned` 210 |

\* `fetch_time` = downstream-pull **wait**, not work — never surface it as cost (§7).
\*\* shown as `time_elapsed_processing`; its `elapsed_compute` is also `1ns`.

Three data facts the layout must handle (all visible above):
- **`rows` is `null` on every `RepartitionExec`.** Don't assume a row count.
- **`time`/compute is `null` on Repartition and HashJoin**, and `1ns` on scans.
  There is no reliable "the time" field per node — hence §7.
- **Every operator surfaces a different metric set.** A scan has ~24 metrics; a
  `ProjectionExec` has 2. The card must adapt, not assume fixed slots.

---

## 4. Metrics catalogue — everything the engine can hand you

Every metric the engine can emit, with the **type** it will tag it as (so the
design can format + group), which operators emit it, and whether it's usually
zero. This is the palette to design from.

| Metric | Type | Emitted by | Usually 0 | Meaning |
| --- | --- | --- | --- | --- |
| `output_rows` | count | most (not Repartition) | no | rows the operator emitted → the **rows** field |
| `elapsed_compute` | time | compute ops; `1ns` on scans; absent on join/exchange | no | CPU time in this operator |
| `time_elapsed_processing` | time | sources | no | real per-scan processing time (scan "self-time") |
| `time_elapsed_scanning_total` | time | sources | no | total scan wall incl. wait |
| `time_elapsed_scanning_until_data` | time | sources | no | time to first batch |
| `time_elapsed_opening` | time | sources | no | file-open time |
| `metadata_load_time` | time | sources | no | parquet footer/metadata load — **can dominate** |
| `*_eval_time` (bloom / page_index / row_pushdown / statistics) | time | sources | often tiny | predicate + pruning eval times |
| `bytes_scanned` | bytes | sources | no | bytes read from files |
| `row_groups_matched_statistics` / `_pruned_statistics` | count | sources | often 0 | row groups kept / skipped by min-max stats |
| `row_groups_matched_bloom_filter` / `_pruned_bloom_filter` | count | sources | often 0 | kept / skipped by bloom filter |
| `page_index_rows_matched` / `_pruned` | count | sources | often 0 | rows kept / skipped by page index |
| `pushdown_rows_matched` / `_pruned` | count | sources | often 0 | rows kept / skipped by pushdown filter |
| `file_open_errors` / `file_scan_errors` / `num_predicate_creation_errors` / `predicate_evaluation_errors` | count | sources | ~always 0 | error counters — **surface loudly iff non-zero** |
| `repartition_time` | time | exchange | no | actual repartition work (exchange "self-time") |
| `send_time` | time | exchange | no | time sending batches downstream (wait-ish) |
| `fetch_time` | time | exchange | no | **downstream-pull wait — not work; never use as cost** |
| `build_time` / `join_time` | time | join | no | hash build / probe time (join self-time = build+join) |
| `build_input_rows` / `input_rows` | count | join | no | rows on build side / probe side |
| `build_mem_used` | memory | join | no | hash-table memory |
| `output_batches` | count | join + others | no | record batches emitted |
| `peak_mem_used` | memory | aggregate | no | peak operator memory |
| `spill_count` / `spilled_rows` | count · `spilled_bytes` memory | aggregate / sort | usually 0 | spilling under memory pressure — **tier-2 iff non-zero** |
| `skipped_aggregation_rows` | count | aggregate | usually 0 | rows skipped by adaptive aggregation |
| `row_replacements` | count | sort (TopK) | no | TopK heap replacements |
| `selectivity` | ratio | filter (some builds) | no | fraction of rows kept |

Types the design can rely on for formatting: **count** (plain int, thousands-sep),
**time** (µs/ns/ms/s, pre-formatted label), **bytes** (`605 B`, `3.1 MB`),
**memory** (same as bytes), **ratio** (`50%`). The engine attaches the `type` and a
ready-to-print `label`; the design never re-derives units.

---

## 5. Data constraints the layout must handle

**5.1 Metrics are many, mostly zero, operator-specific.** Scan ~24, projection 2.
On small data most pruning/error counters are 0. → needs a presentation *strategy*,
not fixed slots (see §6.3).

**5.2 No universal "time" or "memory" field.** `elapsed_compute` is `1ns` on scans
and `null` on join/exchange; each kind reports its own time metric. → the engine
will send a single derived **self-time** per node (§7) so the design has one
comparable number for the chip / bar / hotspot.

**5.3 `rows` and `time` can be `null`.** Every `RepartitionExec` has no rows;
join/exchange have no `elapsed_compute`. Chips must handle absence.

**5.4 `detail` is long.** A `ParquetExec` detail is ~300 chars (file paths +
`projection` + `predicate` + `pruning_predicate`). Wrap, and ideally clamp to 2
lines with expand.

**5.5 Single-node & shallow plans are the common case.** `SELECT *` → one node.
`SELECT … LIMIT n` → 1–3. The 18-node tree is the exception. The view must look
intentional at 1 node (no dangling connectors) and legible at 18 (deep indent).

---

## 6. Proposed visual (design owns this — this is a starting point)

**6.1 Toolbar (~40px):** Physical / Logical tabs (both trees always available,
incl. ANALYZE, which defaults to physical) · summary reflecting the active tab
(`Plan with metrics · 18 operators` / `Physical plan · N` / `Logical plan · N`) ·
ANALYZE badge (physical tab only) · Raw/Tree toggle (Raw = `physicalText` /
`logicalText`). No per-query totals — the data has none.

**6.2 Node card:** indented `depth × 22px`, left border + square in the kind
colour; header = square · name · optional HOTSPOT badge; detail line (wrap/clamp,
§5.4); then the metrics block (ANALYZE only).

**6.3 Metrics — the core problem, three tiers:**
- **Tier 1 headline (always):** `rows` (if present) · **self-time** (§7) · `bytes`
  (sources) · the time-share bar (normalised on self-time).
- **Tier 2 insights (only when non-zero):** small callouts that carry signal —
  `pruned 3/4 row groups`, `pushdown removed 1.2k`, `spilled 4 MB`, `peak 3.4 KB`,
  non-zero `*_errors`. Zeros never appear here.
- **Tier 3 full metrics (collapsed):** `Metrics (24) ▸` → typed, grouped grid;
  zeros hidden behind a "show zeros" toggle.

> Applied to the events scan: headline `7 rows · 15.6 ms · 605 B` + bar; tier-2
> `matched 2 row groups`; collapsed `Metrics (24)`.

**6.4 Tree/indent:** indent by `depth`; optional faint connector lines that
degrade to nothing at a single node; cards stay full-width, indent eats from the
left.

**6.5 States:** single node (no connectors) · logical vs physical (tabs when both
present) · Raw (indent text, h-scroll, mono) · error (reuse the query-error banner)
· running ("Explaining…" spinner).

---

## 7. Self-time — the one derived value the engine adds

Because there's no single time field (§5.2), the engine computes one comparable
**self-time** per node ("work done here") and sends it as `time`. It drives the
time chip, the time-share bar, and HOTSPOT. **`fetch_time`/`send_time` are excluded
— exchange wait, not work** (`fetch_time` was 337ms on a plan that ran ~30ms wall).

| kind | self-time = | fallback |
| --- | --- | --- |
| source | `time_elapsed_processing` | `time_elapsed_scanning_total` → `elapsed_compute` |
| join | `build_time` + `join_time` | `elapsed_compute` |
| exchange | `repartition_time` | 0 |
| agg / sort / filter / proj / coalesce / merge | `elapsed_compute` | 0 |

HOTSPOT = self-time ≥ 60% of the max self-time in the tree. On the reference plan
this correctly flags the two Aggregates (4.79/4.06ms) and the events scan (15.6ms)
— which the current `elapsed_compute`-only value misses entirely.

---

## 8. The engine ↔ design contract (tell me the shape)

The UI renders whatever `PlanNode` shape we agree on — the engine already holds all
the values above and can emit them however the design needs. Today `metrics` is a
flat string; the **proposed target** (drop-in, contained engine change) is:

```
PlanNode {
  name, detail, kind, depth,
  rows:     int | null,
  selfTime: { ms: number, label: string } | null,   // §7
  metrics: [                                          // typed, ordered, pre-labelled
    { name, value, type, label, zero }                // type ∈ count|time|bytes|memory|ratio
  ]
}
```

With that, the design builds tier-1/2/3 purely from `metrics` (filter by `type`,
`zero`, and a name allowlist) — no parsing, no unit math.

**Open questions for the designer** (answers drive what the engine emits):
1. Tier-2 shortlist — confirm which metrics deserve a callout (draft: pruning
   matched/pruned, pushdown removed, spills, `peak_mem`, `build_mem`, non-zero
   errors).
2. Tier-3 grouping — group headers you want (draft: Output · Time · I/O · Pruning ·
   Memory/Spill · Exchange · Join · Errors)?
3. Do you want self-time as the headline time, `elapsed_compute` as a secondary,
   or both shown?
4. Detail: clamp-to-2-lines-with-expand, or full always?
5. Single-node layout: same card centred, or a lighter treatment?

Say the word on #3/#8-shape and I'll switch the engine from the flat `metrics`
string to the structured list (small, contained change — flagged in DEV_TASKS S12).
