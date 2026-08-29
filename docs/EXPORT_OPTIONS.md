# Export options — the Export window and the write it makes

What the Export window offers per format, and the write it produces. The window opens from
the results toolbar on the run that is on screen, and it **pins that snapshot**
(`SnapshotReads::pin`) for its whole life — so a re-run in the tab behind cannot retire or
truncate the table under a running write, and an export is always an export of what was on
screen.

Two sources of truth sit under this, and they are the ones to change:

- **`strata-engine::export`** — `ExportSpec` and the `CopyJob` it builds. Every option a spec can
  name is a field DataFusion honours; there is no key/value bag.
- **`strata-freya::apps::export::model`** — `ExportDraft::groups`, the list the window renders.
  Options are **data**: a group is a label, an optional hint and a control, and every control
  carries the `Edit` it performs. Adding an option is a row in that function, not a branch in a
  component, and it is unit-tested without a renderer.

Engine: **DataFusion 54**. **Nothing here composes SQL.** The window builds the same
`statements::copy_job::CopyJob` a typed `COPY … TO` destructures into — the rows as a plan, the
writer out of the session's own format registry, the options as a map — and
`copy_job::run_copy` gates it and drives it. So there is one write path, one set of gates and one
option spelling; the `COPY (SELECT …) TO '…' OPTIONS (…)` this used to render, and the escaping
rules that came with it, are gone.

The plan the window builds is:

```text
Projection: "col_a", "col_b", …          -- the result's columns, never the ordinal
  Limit: skip=m, fetch=n                 -- only for `This page`
    Sort: "col" ASC|DESC NULLS LAST, "__strata_ord" ASC NULLS LAST
      TableScan: <snapshot>
```

The projection names the result's columns **explicitly, never `*`**: the snapshot carries the
`__strata_ord` bookkeeping column, and a write must not put bookkeeping in the user's file. The
ordinal is what the read *sorts by* instead — alone for an unsorted export, as the tie-break under
a user sort — so even an unsorted export is ordered, which is what makes "the file matches what
was on screen" true rather than hopeful: an unordered `LIMIT/OFFSET` over a split scan is
nondeterministic (measured — `docs/SNAPSHOT_SPEC.md` §9). See `export::snapshot_rows` and its
tests.

Option keys reach the plan **already namespaced** (`format.has_header`,
`execution.keep_partition_by_columns`). A typed `COPY`'s bare keys are lowercased and prefixed by
the SQL planner; a plan-built one never passes through it, so `export::namespaced` restates that
one rule and the tables below name the bare key it is applied to.

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

Newline-delimited only (one object per line) — not a choice Strata makes, but the only shape
DataFusion writes: its `JsonSerializer` is an `arrow::json::LineDelimitedWriter` with no array
mode, so there is no shape option to spell. `format.newline_delimited` is a **read** option
(`IMPORT_OPTIONS.md`), and `STORED AS JSON` is how this shape is written.

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
- **Row-group size is a row count**, not a byte size.
- Per-column knobs (encoding, bloom filters) are not offered: they are per-column settings and this
  is a per-export surface.

## Arrow

No write options exist, so `Format::Arrow` carries no fields and the window shows a
[`Note`](../crates/strata-freya/src/components/form/row.rs) saying so. An empty row would read as
"still loading". (Arrow IPC *can* carry LZ4/ZSTD at the format level; DataFusion doesn't expose it.)

## Registered formats

**The card list is the engine's format registry, filtered on what `COPY` can write** — not a
fixed four. The shipped formats are ordinary registrants (`docs/IMPORT_OPTIONS.md`), so a format
an embedder added with `EngineBuilder::with_format` gets a card here the moment it declares
`copy_to`, and a read-only one never does. `FormatId::offered` is the whole rule.

**Its name is a registry key and its option keys are map keys**, so neither is grammar and
neither is refused for its spelling. The name resolves the writer through the session's own
`get_file_format_factory` — the same lookup a typed `STORED AS` does, so a format the editor can
write is one the window can write, and a name nothing is registered under is refused by
`Format::file_type`. The option keys go through `export::namespaced` exactly as ours do: one that
already names a namespace keeps it, a bare one is filed under `format.`. (Both were refused
unless they were plain words while the window rendered SQL and spliced them into it; a plan
splices nothing.)

A registered format's card carries its own word and **no options section** — just a `Note` saying
the writer it was registered with decides how it is written. There is no options panel to draw:
this build knows nothing about that format's settings. A caller who needs them writes the `COPY`
in the editor, where `OPTIONS ('format.…' '…')` reaches the writer directly. The spec such a card
produces is `Format::Extension { format, options }` with the options empty, so nothing of ours is
attached to somebody else's writer.

---

## The PREVIEW pane

A full section of the window, showing what the chosen options will actually produce
(`strata-freya::apps::export::preview`). It re-renders on every edit, and it shows **only real
facts**: every row is a row the grid already fetched (the page in hand, carried in as
`ExportTarget::sample`), every type is the run's own schema, every count is read from the run.
Nothing is estimated.

Per format:

- **CSV** — the first rows of the page, rendered by a mirror of the writer's own rules: the
  chosen delimiter, quote, escape and null text, a field quoted exactly when it contains the
  delimiter, the quote or a newline, and the same escape resolution as the spec (`\t` previews as
  a tab). The header row follows its toggle.
- **JSON** — the same rows as NDJSON, strings quoted and numbers/booleans bare per the schema.
- **Parquet** — a schema summary (`message result { … }` with each column's physical type and
  repetition) plus the settings that will be written: codec and level, statistics, row-group
  size, writer version, dictionary. No rows — a parquet file has none to show as text.
- **Arrow** — the schema, and "(no write options)".
- **Partitioned** (any format) — the Hive tree the export will write, built **only from values
  genuinely present in the page in hand**: the first few branches, a trailing `…` when there are
  more, and an honest line — `shape from the N rows loaded; the full export covers M rows` —
  because the page is not the snapshot. It also states the levels in order and whether partition
  columns are kept in the files.

An empty result previews as `(no rows to preview)` rather than a bare header.

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
- **Reordering is ▲▼ buttons, not drag-and-drop**, and order is the whole meaning of the list
  (outermost level first).
- **Column names must be a single bare word.** A Hive directory segment is `name=value`, so a name
  the tokenizer does not read as one word can never equal the segment it was written under — and a
  typed `PARTITIONED BY ("order date")` arrives at the planner with its quotes attached and matches
  no field. `export::partition_columns_are_bare_words` is the one rule, shared by all three
  surfaces that carry a `PARTITIONED BY`.
- **Keep-columns rides in the write's own options**
  (`'execution.keep_partition_by_columns'`). It is a session config, but DataFusion's physical
  planner reads that exact key out of the COPY node's options first and only falls back to the
  session when it is absent — so an export states its answer and leaves the engine's setting alone. It was
  a `SET` once, and never restored: invisible for as long as nothing could read it back, and one
  export deciding the answer for every later one the moment `SET` and `SHOW` became statements a
  user can type. The `execution.` namespace stays on the key because `TableOptions::set` skips
  that namespace, which is what lets it reach the planner without the format refusing it.
- **A NULL in a partition column is refused, not warned about** — see below.

### NULL partition values

DataFusion 54 has no `__HIVE_DEFAULT_PARTITION__`: given `(1,'emea'), (2,NULL), (3,'amer')`
partitioned by `region`, it writes `region=emea` and `region=amer` and files the NULL row **inside
`region=amer`**. It reads back claiming a value it never had — no dropped row, no error, and
unrecoverable once the source result is gone.

`copy_job::no_null_partition_values` refuses the write and names the column. The Export window's
evidence is `NullEvidence::Snapshot`, and two things make it cheap and reliable:

- **It is neither a scan nor a footer read.** The snapshot is Arrow IPC, which carries no column
  statistics at all — but the file was never worth asking. `query::materialize` streams every batch
  to write it and `Array::null_count` is a stored field, so the exact per-column count is a running
  sum over data already in hand (`query::SnapshotStats`, held for the snapshot's lifetime). Free to
  produce, a slice index to read.
- **The rule is "proceed only on an exact zero."** `SnapshotStats` is exact by construction — it
  counted every row that was written — so there is no "unknown" reading to disambiguate. A missing
  entry is not zero nulls; it is a count we cannot vouch for, and that declines too.

**The typed `COPY` reaches the same refusal by a different route.** A statement the user types has
no snapshot behind it, so its evidence is `NullEvidence::Count`: one pre-flight aggregate over the
job's own input. Same gate, same exact-zero rule, same sentence — two ways of answering one
question, so the surfaces state one fact once (`STATEMENTS_SPEC.md` §6.4).

**Schema nullability is not the signal and cannot be** — DataFusion reports every column of every
real table as nullable, parquet sources included (measured). Gating on `ColumnInfo.nullable` would
empty the column list and make partitioning unusable.

---

## Deliberately not built

Each of these appeared in earlier designs and was dropped on purpose:

- **The ADVANCED disclosure.** The list is flat: a format's advanced controls are just more of that
  format's options.
- **The size estimate** (`≈ 1.2 MB`, in the footer and over the preview). It came from invented
  per-codec compression factors, and a fabricated byte figure beside real ones breaks the
  only-real-facts rule the column inspector settled. The footer quotes the real row count.
- **The NULL partition warning banner.** The engine refuses and names the column, so a standing
  banner warned about something that could not happen.
- **The high-cardinality warning** — a distinct count over an 80-row sample is a
  derived-from-what's-on-screen number of exactly the sort the only-real-facts rule rejects.
- **The Clipboard tile.** The grid grew its own copy controls, so "export" here always means a
  file on disk.
- **A hand-built file browser.** The destination is the native `rfd` dialog; duplicating an OS
  dialog is not the deliverable.
