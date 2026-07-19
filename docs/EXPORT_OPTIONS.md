# Export options — per-format breakdown (for modal redesign)

What the engine (DataFusion 43, via `COPY (query) TO '<file>' STORED AS <fmt> OPTIONS(...)`)
can actually do per format, so the modal only offers controls that map to real behaviour. By default an export writes a
**single file** (the destination path carries an extension) — the exception is **partitioning** (see the Partitioning
section), which writes a *directory* of files instead.

Priority legend: **Core** = worth surfacing prominently · **Advanced** = tuck behind an "Advanced" disclosure ·
**Skip** = supported but not worth a control.

---

## CSV

Yes to both of your questions — headers are optional and the delimiter is configurable, plus a few more that matter for
round-tripping into other tools.

| Control             | Effect                                                                       | Values                                       | Default | Priority |
|---------------------|------------------------------------------------------------------------------|----------------------------------------------|---------|----------|
| Include header row  | Write the column-name row                                                    | on / off                                     | on      | **Core** |
| Delimiter           | Column separator (single char)                                               | comma · tab · semicolon · pipe · custom char | comma   | **Core** |
| Null value          | Text written for NULL cells                                                  | any string (e.g. empty, `NULL`, `NaN`)       | empty   | **Core** |
| Quote char          | Character used to quote fields                                               | single char                                  | `"`     | Advanced |
| Escape char         | Character used to escape specials                                            | single char                                  | none    | Advanced |
| Double-quote        | Escape quotes by doubling (`""`) instead of escape char                      | on / off                                     | on      | Advanced |
| Date / time formats | strftime-style format strings for date / datetime / timestamp / time columns | format strings                               | ISO-ish | Advanced |
| Compression         | Gzip/… the output file                                                       | none · gzip · bzip2 · xz · zstd              | none    | Advanced |

Notes for design:

- Delimiter is **one character** — offer named presets (comma/tab/semicolon/pipe)
  with an optional "custom" single-char input.
- Compression changes the file extension (`.csv.gz`). If we expose it, the destination filename/preview should reflect
  that.

---

## JSON

**Only newline-delimited JSON (NDJSON)** — one JSON object per line. DataFusion's JSON writer does not support a
pretty-printed or single-array output. So the current "Pretty-print / newlines" toggle is a **no-op and should be
removed**.

| Control     | Effect                 | Values                          | Default | Priority |
|-------------|------------------------|---------------------------------|---------|----------|
| Compression | Gzip/… the output file | none · gzip · bzip2 · xz · zstd | none    | Advanced |

Notes for design:

- The only real JSON knob is compression. The format section for JSON can be essentially empty (or a short "records, one
  per line (NDJSON)" explainer).
- If we ever want a pretty JSON array, that's a custom writer we'd build ourselves — flag as a separate feature, not a
  DataFusion option.

---

## Parquet

The richest format. Compression is the headline; the rest are tuning knobs most users never touch.

| Control             | Effect                                         | Values                                             | Default | Priority                     |
|---------------------|------------------------------------------------|----------------------------------------------------|---------|------------------------------|
| Compression         | Codec for column data                          | uncompressed · snappy · gzip · brotli · lz4 · zstd | zstd    | **Core**                     |
| Compression level   | Level for codecs that take one                 | integer (e.g. zstd 1–22, gzip 1–9)                 | zstd(3) | **Core** (paired with codec) |
| Statistics          | Column statistics written                      | none · chunk · page                                | page    | Advanced                     |
| Max row group size  | Rows per row group (memory vs scan efficiency) | integer                                            | 1048576 | Advanced                     |
| Writer version      | Parquet format version                         | 1.0 · 2.0                                          | 1.0     | Advanced                     |
| Dictionary encoding | Enable dictionary encoding                     | on / off                                           | on      | Advanced                     |
| Encoding            | Page encoding scheme                           | plain · rle · delta_binary_packed · …              | auto    | Skip (per-column, niche)     |
| Bloom filters       | Write bloom filters (per column)               | on / off + fpp / ndv                               | off     | Skip (per-column, niche)     |

Notes for design:

- Only **codec + level** are worth prominent placement. Level only applies to gzip / brotli / zstd — hide/disable it for
  snappy / lz4 / uncompressed.
- Everything else belongs under "Advanced".

---

## Arrow

**No configurable write options.** DataFusion writes an Arrow IPC file with no
`OPTIONS` support (the format-options reference documents only CSV/JSON/Parquet).

| Control | Effect | Values | Default | Priority |
|---------|--------|--------|---------|----------|
| —       | —      | —      | —       | —        |

Notes for design:

- The format section for Arrow should be empty — just a one-line explainer ("Arrow IPC file — schema-faithful, no
  options"). Arrow IPC *can* carry LZ4/ZSTD compression at the format level, but DataFusion doesn't expose it, so we
  can't offer it without a custom writer.

---

## Partitioning (Hive-style) — works with ALL formats

`COPY … TO '<dir>' STORED AS <fmt>, PARTITIONED BY (col1, col2 …)` writes a **directory** of hive-style partitioned
files instead of a single file. Confirmed supported for **parquet, csv, json, and arrow** (it's a general COPY clause,
not parquet-specific) — most useful with parquet, but available everywhere.

Output shape:

```
<dir>/col1=<value>/col2=<value>/<part>.<ext>
```

| Control                | Effect                                            | Values                       | Default | Priority                      |
|------------------------|---------------------------------------------------|------------------------------|---------|-------------------------------|
| Partition by           | Columns that become directory levels (ordered)    | any subset of result columns | none    | **Core** (parquet) / Advanced |
| Keep partition columns | Also write the partition columns inside the files | on / off                     | off     | Advanced                      |

Key facts:

- Output is a **directory**, not a single file — one subtree per distinct combination of partition-column values, with
  format-specific part files inside.
- Partition columns are **removed from the data files** by default (they live in the directory names); "keep" maps to
  `execution.keep_partition_by_columns = true`.
- Round-trips with the app's own hive-partition reading (e.g. the sample's
  `events/year=…/month=…`).
- Not wired in the app yet — export currently always writes a single file; the engine already supports this via COPY.

Design implications:

- Choosing "partition" flips the destination from a **Save File** dialog to a **Choose Folder** dialog (the app builds
  the hive tree inside).
- Needs an **ordered multi-select of columns** to partition by (from the current result's columns) — `col1` is the outer
  directory.
- Preview should show the directory-tree shape, not a filename.
- High-cardinality partition columns explode into many directories — worth a warning.

Priority: **Core-ish** for a parquet-focused tool (a headline workflow), but it's a bigger UI branch (folder picker +
ordered column select), so it may deserve its own "Partitioned export" mode rather than a single checkbox.

---

## Cross-cutting design implications

1. **Options are format-specific.** Today the modal shows "Include header row"
   (CSV-only) and "Pretty-print" (applies to nothing) regardless of format. The redesign should swap the options panel
   based on the selected format:
   CSV → header + delimiter + null + advanced; JSON → (compression only); Parquet → codec/level + advanced; Arrow →
   none; Clipboard → (see below).
2. **Compression is shared** by CSV / JSON / Parquet but means different things (whole-file gzip for CSV/JSON vs
   internal codec for Parquet) — label accordingly.
3. **Scope** (all rows vs current page) is supported by the engine but has no control yet — worth a toggle in the
   redesign. Default: all rows.
4. **Clipboard** is a separate path (no file, no DataFusion): currently copies the loaded page as a **markdown table**.
   Could grow a sub-picker (markdown / TSV / CSV / JSON) — design-dependent.
5. **Destination + preview** should reflect the chosen extension (and compression suffix) so the filename shown matches
   what's written.

> Caveat: option tables are from DataFusion's current docs; the exact set should
> be re-validated against v43 when each control is wired, but the Core ones
> (header, delimiter, null value, parquet compression) are stable.
