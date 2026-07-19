# Import (read) options — per-format breakdown (for Configure-table design)

Companion to `EXPORT_OPTIONS.md`. These are the **read** options that apply when registering an external table
(DataFusion `ListingTable` / `CREATE EXTERNAL
TABLE`). Today the Configure modal has **none** — CSV/JSON register with DataFusion defaults, so a non-default CSV
registers *wrong* (wrong delimiter → one giant column; no-header file → first row treated as header). Options should be
**format-specific**: shown only when the selected format needs them (same pattern as the export modal), and persisted
into the table spec.

Priority: **Core** = surface prominently · **Advanced** = behind a disclosure · **Skip** = supported but niche.

Source of option names: DataFusion Format Options (<https://datafusion.apache.org/user-guide/sql/format_options.html>).
Re-validate exact `CsvFormat`/`JsonFormat` builder methods against v43 when wiring.

---

## Parquet / Arrow

**No read options.** Schema is self-describing (Parquet footer / Arrow IPC), partition columns come from the Hive path.
The Configure modal shows only name / sources / partitioning for these — no format section.

---

## CSV

The one format that genuinely needs options — without them, many real CSVs can't be registered correctly.

| Control            | Effect                                            | DataFusion option           | Values                                  | Default                  | Priority |
|--------------------|---------------------------------------------------|-----------------------------|-----------------------------------------|--------------------------|----------|
| Has header row     | Treat row 1 as column names (vs `column_1…`)      | `HAS_HEADER`                | on / off                                | inferred (usually on)    | **Core** |
| Delimiter          | Column separator (single char)                    | `DELIMITER`                 | comma · tab · semicolon · pipe · custom | comma                    | **Core** |
| Null value / regex | Text(s) that mean NULL                            | `NULL_VALUE` / `NULL_REGEX` | any string / regex                      | none                     | **Core** |
| Quote char         | Field quote character                             | `QUOTE`                     | single char                             | `"`                      | Advanced |
| Escape char        | Escape character                                  | `ESCAPE`                    | single char                             | none                     | Advanced |
| Newlines in values | Allow quoted fields to contain newlines           | `NEWLINES_IN_VALUES`        | on / off                                | off                      | Advanced |
| Comment char       | Skip lines starting with this char                | `COMMENT`                   | single char                             | none                     | Advanced |
| Schema-infer rows  | Rows scanned to infer column types (0 = all Utf8) | `SCHEMA_INFER_MAX_REC`      | integer                                 | engine default           | Advanced |
| Compression        | Read gzip/bzip2/xz/zstd-compressed CSVs           | `COMPRESSION`               | none · gzip · bzip2 · xz · zstd         | none (or infer from ext) | Advanced |

Notes:

- Delimiter is a **single character** — offer named presets (comma/tab/semicolon/ pipe) + optional custom single-char
  input, mirroring export.
- These flow through to `register_external` as `CsvFormat::default().with_*(…)`
  and must be **persisted in the table spec** (deterministic reload).

---

## JSON

DataFusion's JSON reader is **NDJSON only** (one record per line). So options are minimal.

| Control           | Effect                          | DataFusion option      | Values                          | Default        | Priority |
|-------------------|---------------------------------|------------------------|---------------------------------|----------------|----------|
| Compression       | Read gzip/… -compressed NDJSON  | `COMPRESSION`          | none · gzip · bzip2 · xz · zstd | none           | Advanced |
| Schema-infer rows | Records scanned to infer schema | `SCHEMA_INFER_MAX_REC` | integer                         | engine default | Advanced |

Not supported natively (would be **custom/future** work, flag as such):

- **"JSON records path"** — digging into a nested array inside a whole-document
  `.json` file. DataFusion only reads NDJSON; whole-document JSON-as-table needs a custom reader (relates to the
  FEATURES.md "JSON shape detection": NDJSON dir = one table, vs whole-document `.json` = one record / not tabular).

---

## Design implications

1. **Format-swapped options panel** — the Configure modal shows a CSV options section only for CSV, a (minimal) JSON
   section for JSON, nothing for Parquet/Arrow. Same mechanic as the export modal.
2. **Core vs advanced** — CSV surfaces header + delimiter + null prominently; the rest behind "Advanced". JSON's are all
   advanced.
3. **Persisted, not re-detected** — options live in the table spec so reload is deterministic (same rule as partition
   `(name, type)`).
4. **Validation ties in** — with a wrong delimiter the schema-consistency / file-match readout (C3) should still be
   meaningful; options are applied before inference.
