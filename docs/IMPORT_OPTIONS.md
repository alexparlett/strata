# Import (read) options — what Table Config offers per format

Companion to `EXPORT_OPTIONS.md`. These are the **read** options the Configure (table config)
window offers when registering an external table, and how they reach DataFusion's readers.

Options are **persisted in the table def**: `SourceFormat` (`strata-model::catalog`) is a tagged
enum where the format *is* its options — `Csv(CsvRead)`, `Json(JsonRead)` — so a delimiter cannot
be written down on a parquet table. The options flow into the reader at registration, in both
halves of the read path (schema inference and the scan), which makes reload deterministic: a
project reopens reading its files exactly as it did. Every field defaults to DataFusion's own
default, so a def written before read options existed registers the way it always did.

Two sources of truth sit under this, and they are the ones to change:

- **`strata-model::catalog`** — `CsvRead` / `JsonRead` / `SourceFormat`: the persisted fields and
  the doc comments carrying each option's semantics and exclusions.
- **`strata-freya::apps::configure::model`** — `ConfigureDraft::options()`, the list the window
  renders (options are data, same mechanism as the export window).

The options render as **one flat list** per format — there is no ADVANCED disclosure. The export
window folded its own away on the grounds that a format's advanced controls are just more of that
format's options, and that reasoning holds here too: a disclosure would only be one more thing to
open before a CSV's quote character can be reached, in a window whose whole subject is how a file
is read.

---

## CSV

The format that genuinely needs options — without them, many real CSVs cannot be registered
correctly (wrong delimiter → one giant column; headerless file → first row eaten as names).

| Control | Effect | Default |
|---|---|---|
| HEADER ROW | Row 1 holds column names (off: columns are `column_1`, `column_2`, …) | on |
| DELIMITER | Field separator — free text, `\t` accepted for tab | `,` |
| QUOTE CHARACTER | Wraps fields containing the delimiter | `"` |
| ESCAPE CHARACTER | Escapes a quote inside a quoted field (blank = none) | blank |
| COMMENT CHARACTER | Skip lines starting with this character (blank = none) | blank |
| NEWLINES IN VALUES | Allow quoted fields to contain line breaks | off |
| RAGGED ROWS | Pad rows — or whole files — short of a column with nulls instead of failing the read | off |
| SCHEMA-INFER ROWS | Rows scanned to infer column types; 0 reads every column as text | engine default |
| COMPRESSION | None · gzip · bzip2 · xz · zstd | None |

Notes:

- **NEWLINES IN VALUES costs the parallel file split** (`CsvSource::supports_repartitioning`),
  which is why it defaults off.
- **RAGGED ROWS** (`truncated_rows`) also covers the multi-path case: one source path carrying a
  column the others lack reads as the union of the columns found, padded with nulls.
- **SCHEMA-INFER ROWS** persists as `Option<usize>`: unset means the engine's default, and the
  window's `0` means DataFusion's "disable inference" arm — every column as text.

### Deliberately not offered

Every offered field reaches **both** halves of the read path — inference and scan. That bar
excluded options that look available and are not (`CsvRead`'s doc comment is the full argument):

- **NULL value / regex** — `null_regex` is wired into `CsvFormat`'s *inference* only; the scan
  never sees it. Setting it re-types a column and then fails the scan parsing the very token it
  was told was null — strictly worse than leaving it off, where the column simply infers as text.
- **Line terminator** — the mirror image: wired at scan, absent from inference, so the schema and
  the rows would be read by different rules.
- `double_quote`, `null_value`, date/time formats and the rest of the writer's options — no read
  path references them (the export window is where they live).

---

## JSON

| Control | Effect | Default |
|---|---|---|
| SHAPE | One record per line (NDJSON) · JSON array | one record per line |
| SCHEMA-INFER ROWS | Records scanned to infer the schema; 0 scans every record | scan every record |
| COMPRESSION | None · gzip · bzip2 · xz · zstd | None |

- **Both shapes are read** — DataFusion 54's `JsonFormat::with_newline_delimited` covers the
  whole-document array as well as NDJSON, so shape is an option rather than a rule the reader
  enforces. One caveat rides with the array shape: DataFusion cannot range-split such a file, so
  a single array file over `datafusion.optimizer.repartition_file_min_size` (10 MB) fails its
  scan with a `NotImplemented` — loud and self-describing, and only above that size.
- **Schema inference defaults to scanning every record**, deliberately: the reader exists to
  notice a type conflict, and a capped scan that misses one types the column wrong and then fails
  at *query* time on a table the catalog called healthy. The cap is there to opt into speed on
  files known to be uniform.

---

## Parquet / Arrow

**No per-table read options — and no empty section saying so.** Twice over:

- The schema is self-describing (parquet footer / Arrow IPC), so there is nothing a read needs
  telling.
- Every `ParquetFormat` knob DataFusion does have is an engine-wide setting with a control in
  **Settings ▸ Engine** already; a per-table copy would be a second place to set the same key
  (`apps/configure/views/options.rs`).

---

## Compression and file extensions

Whole-file compression applies to the text formats only — parquet and Arrow carry compression
*inside* the file. The codec changes the extension the source listing filters on: a gzipped CSV
is `events.csv.gz`, and a listing filtered on `.csv` alone would match none of them, so
`SourceFormat::extension` composes the format's extension with the codec's
(DataFusion's own suffixes: `.gz`, `.bz2`, `.xz`, `.zst`).

---

## Hive partition detection

Not a format option, but part of the same window. The partitioning section appears when
`key=value` levels are found in the sources — spelled as globs in the path (`year=*`), or
discovered by **listing** the source's store (`engine::catalog::detect_partitions`, which lists
through the session's registered object store and so works identically over a local folder and a
bucket). It is format-agnostic.

Discovered columns register **typed**: each level defaults to text with a type picker beside it,
and a standing warning explains the consequence of leaving one as text — partition values are
read as text, so `WHERE year = 2024` needs a cast until the column is given its real type. A
*literal* `key=value` segment in a source path deliberately does not declare a column: the path
is the listing root, so a literal segment means that level is already consumed, and declaring it
would produce a table that registers cleanly and returns zero rows for every query.
