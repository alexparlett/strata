# Export options — the as-built surface

What the Export window (P4-10) offers per format, and the `COPY … TO` it produces. This replaces
the pre-build capability survey: that document ranked options **Core / Advanced / Skip** for a
disclosure the canvas then removed, and described DataFusion 43.

Two sources of truth sit under this, and they are the ones to change:

- **`strata-core::engine::export`** — `ExportSpec` and the SQL it renders. Every option a spec can
  name is a field DataFusion honours; there is no key/value bag.
- **`strata-freya::apps::export::model`** — `ExportDraft::groups`, the list the window renders.
  Options are **data**: a group is a label, an optional hint and a control, and every control
  carries the `Edit` it performs. Adding an option is a row in that function, not a branch in a
  component, and it is unit-tested without a renderer.

Engine: **DataFusion 54**. Its `COPY` planner lowercases bare option keys and applies the
`format.` prefix itself, so the keys below resolve onto `CsvOptions` / `JsonOptions` /
`TableParquetOptions` field names.

```sql
COPY (SELECT * FROM <snapshot> [ORDER BY "col" ASC|DESC NULLS LAST] [LIMIT n OFFSET m])
TO '<path>' STORED AS <FMT> [PARTITIONED BY (a, b)] [OPTIONS (…)]
```

---

## Always shown, whatever the format

| Group | Control | Values | Default |
|---|---|---|---|
| ROWS TO EXPORT | segmented | `All · <n>` · `This page` | All |
| HIVE PARTITIONING | toggle + column transfer | see below | off |

**Scope is applied after the sort, not before.** The `ORDER BY` goes ahead of the `LIMIT/OFFSET`
window, so "this page" means the page the user is looking at rather than an arbitrary slice
re-ordered afterwards. `NULLS LAST` in both directions, matching the grid.

**The sort is the grid's**, carried in as a launch value — it is not a control here.

---

## CSV

| Group | Control | Values | Default | `OPTIONS` key |
|---|---|---|---|---|
| HEADER ROW | toggle | on / off | on | `HAS_HEADER` |
| DELIMITER | text (max 8) | any single char; `\t` resolved | `,` | `DELIMITER` |
| NULL VALUES AS | segmented + custom text (max 16) | Empty · `NULL` · `NaN` · custom | Empty | `NULL_VALUE` |
| QUOTE CHARACTER | char | single char | `"` | `QUOTE` |
| ESCAPE CHARACTER | char | single char; blank = double-quote | blank | `ESCAPE` (omitted when blank) |
| DOUBLE-QUOTE | toggle | on / off | on | `DOUBLE_QUOTE` |
| COMPRESSION | select | None · Gzip · Zstd · Bzip2 · XZ | None | `COMPRESSION` |

- The delimiter, quote and escape are sent as **byte values**, not quoted strings —
  `export::ascii_byte` rejects a non-ASCII character with a message naming the field rather than
  emitting SQL that fails in the planner.
- Compression changes the destination's extension (`orders.csv` → `orders.csv.gz`), and the save
  dialog is pre-filled with the suffix so what is offered matches what is written.

## JSON

Newline-delimited only (one object per line). DataFusion's writer can also emit a JSON array, but
the canvas offers NDJSON alone, so the spec doesn't spell the option.

| Group | Control | Values | Default | `OPTIONS` key |
|---|---|---|---|---|
| COMPRESSION | select | None · Gzip · Zstd · Bzip2 · XZ | None | `COMPRESSION` |

## Parquet

| Group | Control | Values | Default | `OPTIONS` key |
|---|---|---|---|---|
| COMPRESSION | select | Zstd · Snappy · Gzip · Brotli · Lz4 · Uncompressed | Zstd | `COMPRESSION` |
| COMPRESSION LEVEL (min–max) | number | zstd 1–22 · gzip 1–9 · brotli 1–11 | 3 | (rides in the codec string) |
| STATISTICS | segmented | None · Chunk · Page | Page | `STATISTICS_ENABLED` |
| MAX ROW GROUP SIZE | segmented | 128K · 512K · 1M · 2M **rows** | 1M | `MAX_ROW_GROUP_SIZE` |
| WRITER VERSION | segmented | 1.0 · 2.0 | 1.0 | `WRITER_VERSION` |
| DICTIONARY ENCODING | toggle | on / off | on | `DICTIONARY_ENABLED` |

- **The level group only exists for codecs that take one.** It appears and disappears with the
  codec, because a level on snappy is a control that changes nothing. The level also rides *inside*
  the codec (`Codec::Zstd(3)` → `zstd(3)`), so a level can't be set on a codec that would ignore it.
- **Row-group size is a row count**, not a byte size — the master canvas's stale `FMT_META` mock
  labels it in MB, which is wrong.
- Per-column knobs (encoding, bloom filters) are not offered: they are per-column settings and this
  is a per-export surface.

## Arrow

No write options exist, so `Format::Arrow` carries no fields and the window shows a
[`Note`](../crates/strata-freya/src/components/form/row.rs) saying so. An empty row would read as
"still loading". (Arrow IPC *can* carry LZ4/ZSTD at the format level; DataFusion doesn't expose it.)

---

## Hive partitioning — every format

`PARTITIONED BY (a, b)` writes a **directory** of `a=<value>/b=<value>/<part>.<ext>` instead of one
file, so the destination flips from a save-file dialog to a choose-folder one, and the suggested
name loses its extension.

| Group | Control | Values | Default |
|---|---|---|---|
| HIVE PARTITIONING | toggle | on / off | off |
| (columns) | two-pane transfer, ordered | numeric and string columns | none |
| Keep partition columns inside files | toggle | on / off | off |

Rules that cost something to rediscover:

- **The toggle gates the selection, it doesn't clear it.** `PartitionDraft::effective` is the one
  answer every consumer reads (preview, suggested name, spec) — they once disagreed.
- **Numeric and string columns only.** A directory name has to be a short stable scalar; a
  timestamp or a struct has none.
- **Reordering is ▲▼ buttons, not drag-and-drop.** The canvas uses HTML5 drag events, which have no
  equivalent here, and order is the whole meaning of the list (outermost level first).
- **Column names must be a single bare word.** DataFusion 54's COPY parser re-renders each
  identifier with `Ident::to_string()`, so a quoted name arrives with its quotes attached and
  matches no field. The export says so plainly rather than emitting SQL that fails on a stray token.
- **Keep-columns is a session config** (`execution.keep_partition_by_columns`), not a COPY option,
  so it is set per partitioned export.
- **A NULL in a partition column is refused, not warned about** — see below.

### NULL partition values

DataFusion 54 has no `__HIVE_DEFAULT_PARTITION__`: given `(1,'emea'), (2,NULL), (3,'amer')`
partitioned by `region`, it writes `region=emea` and `region=amer` and files the NULL row **inside
`region=amer`**. It reads back claiming a value it never had — no dropped row, no error, and
unrecoverable once the source result is gone.

`export::partition_columns_have_no_nulls` refuses the export and names the column. Two things make
that cheap and reliable:

- **It is a footer read, not a scan.** The snapshot is a parquet file we wrote, so the per-column
  null count is already in its metadata — which is why `query::snapshot_writer_props` sets
  `EnabledStatistics::Chunk` explicitly rather than trusting parquet-rs's default.
- **The rule is "proceed only on an exact zero"**, which also disposes of DataFusion's statistics
  ambiguity: `Precision::Exact(num_rows)` doubles as its "no statistics for this column" fallback.
  An all-NULL column and one we can't vouch for are both reasons to decline.

**Schema nullability is not the signal and cannot be** — DataFusion reports every column of every
real table as nullable, parquet sources included (measured). Gating on `ColumnInfo.nullable` would
empty the column list and make partitioning unusable.

---

## Deliberately not built

Each of these is in the canvas or the pre-build survey and was dropped on purpose:

- **The ADVANCED disclosure.** The list is flat: a format's advanced controls are just more of that
  format's options.
- **The size estimate** (`≈ 1.2 MB`, in the footer and over the preview). It came from invented
  per-codec compression factors; a fabricated byte figure beside real ones is what the column
  inspector rejected (P3-08). The footer quotes the real row count.
- **The NULL partition warning banner.** The engine refuses and names the column, so a standing
  banner warned about something that could not happen.
- **The high-cardinality warning** as the canvas computes it — a distinct count over an 80-row
  sample is a derived-from-what's-on-screen number of exactly the sort P3-08 rejected.
- **The Clipboard tile.** The canvas dropped it (2026-07-12) once the grid grew its own copy
  controls, so "export" here always means a file on disk.
- **A hand-built file browser.** The destination is the native `rfd` dialog; duplicating an OS
  dialog is not the deliverable.
