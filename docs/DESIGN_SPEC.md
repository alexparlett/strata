# Strata — Design Spec

Status: **implementing.** The canonical design is the Claude Design prototype
`Strata.dc.html` (handoff bundle; the product was renamed Strata → Strata in the v5 redesign). §1–§13 below capture the
model and the DataFusion facts; **§14 (Strata addendum)** is the current source of truth for stack, design tokens, UI
surfaces, and the SQL DDL policy — read it first and treat it as overriding earlier stack notes.

Stack (current): **Dioxus 0.7 desktop** (RSX in a webview, so the prototype's HTML/CSS ports faithfully) + **Apache
DataFusion 43** (Arrow 53) on a background Tokio thread. Editor: **`dioxus-code` / `dioxus-code-editor`** (tree-sitter).

---

## 1. Goals

A local, **Athena-style** parquet query tool: point it at files, query across them with SQL, no Glue/metastore setup.
The mental model is a database console (catalog + query editor + results), not a file browser.

Design principles:

- **Catalog-centric.** The app manages a catalog of tables and views. Everything else (file inspection, path picking) is
  in service of creating catalog objects.
- **Declarative + persistent.** Catalog objects are saved specs; a project on disk can be reopened and rebuilt
  deterministically.
- **Read/query first.** Data is queried and exported, not edited in place.

---

## 2. Product model

- A **project** is a directory containing a `.ps/` metadata dir (like
  `.git`/`.vscode`). Opening a folder with `.ps/manifest.json` opens the project; a folder without one can be
  initialised.
- The catalog has **two citizen types**: **external tables** (file-backed) and **views** (query-backed). Both persist as
  specs in `.ps/`.
- **Namespaces/schemas are out of scope for v1** — everything lives in the default `public` schema; table names are
  unique within the project (auto-dedup). Optional cosmetic "group" tag for visual organisation only. Real DataFusion
  schemas are a clean future upgrade.

### Main surfaces

- **Catalog panel** — tables + views, each expandable to its schema; actions (query `SELECT *`, insert column/name,
  edit, remove).
- **Query console** — SQL editor (highlighting, format, autocomplete) + run + results. Supports multiple query buffers
  and history.
- **Data viewer** — the results grid, including nested-type rendering (§7).
- **Object detail** — selecting a table/view shows Data / Schema / Metadata (reuses the components already built for the
  file view).

---

## 3. Catalog objects

### 3.1 External tables

A file-backed table (`ListingTable`). "External table" is DataFusion's DDL name for exactly this; `register_parquet` is
the programmatic form — same object.

- **Sources:** a directory, an explicit set of paths, or a glob — and any mix. A directory is just the one-element case.
  Use `new_with_multi_paths` (§App A).
- **One file format per table.** Parquet / CSV / JSON / Arrow / Avro — a table is a single format. You cannot mix
  formats within one table; combine different formats via separate tables + a `UNION ALL` view.
- **Same object store per table.** All paths must share one store (all local, or all `s3://`, not mixed). Remote stores
  are future scope.
- **Schema is inferred** from the files (§5). Partition columns (§6) stack on top.

### 3.2 Views

A saved SQL query (`CREATE [OR REPLACE] VIEW`). Virtual, **non-materialized** — stores the query, re-runs on reference,
holds no data. Ideal for saving a cross-file join as a reusable "table". Persisted as raw `.sql`.

### 3.3 In-memory / materialized tables — DEFERRED

`CREATE TABLE AS SELECT` (a `MemTable`) copies data into RAM. Distinct from external (reference-in-place). Not a v1
creation path; revisit only if a copy-in/snapshot semantic is wanted.

---

## 4. Table creation & update flows

### New external table

1. **Pick sources** — directory, file multi-select, and/or glob. Any mix into one path list.
2. **Inspect** — for each selected file show its schema + a small preview (uses
   `read_parquet`, no catalog pollution).
3. **Schema-compatibility pre-check** (§5) — done client-side from the per-file schemas, *before* registering, because
   DataFusion's own inference only samples a subset of files.
4. **Partition detect + confirm** (§6) — scan path shape, propose Hive partition columns with types, user
   confirms/edits; enforce a single uniform scheme.
5. **Name** — derived, deduped (`derive_table_name`: lower_snake, `t_` prefix for leading digits, `_2` on collision).
6. **Create** — async engine command; surface `Ok`/`Err` against the pending row.

### New view

From the current editor buffer / a query → `CREATE OR REPLACE VIEW name AS <sql>`.

### Update table config

Editing a table = edit its saved spec (paths, options, partitions) → re-register → surface errors. "Save the spec, let
DataFusion validate, surface errors" is the model (no elaborate revalidation beyond the pre-check).

---

## 5. Schema handling

### Multi-file merge (DataFusion behavior — verified)

Registration merges the files' schemas by **field name**:

- Identical columns → OK.
- Disjoint / superset columns → merged to the **union**; at scan, a file missing a column reads it back as **NULL**.
  Handled automatically.
- Same column, differing nullability → merged to nullable.
- **Same column name, incompatible types** → merge **fails**, registration errors.
- Metadata-only differences → avoid with `ParquetFormat::with_skip_metadata(true)`.

**Critical caveat:** inference samples only *up to a configured number of files/records*, so a conflict in an unsampled
file will NOT fail at creation — it surfaces later mid- **query**. Therefore do the pre-check across **all** selected
files using the schemas the app already loads, rather than trusting inference.

### Conflict resolution (when a same-named column has incompatible types)

Do not auto-register. Show the offending column (s) and file (s), and offer:

1. Exclude the conflicting file (s), combine the rest.
2. Register as separate tables.
3. Generate a `UNION ALL` view that `CAST`s each source to a chosen common type.

### Nested types (struct / list / map)

- Arrow exposes the **full recursive** schema: `Struct(fields)` → child fields;
  `List/LargeList/FixedSizeList(field)` → element type; `Map(entries,_)` → a struct of `key`/`value`. Every level is
  walkable via `field.data_type()`.
- **Store the structure, not a flattened string.** Replace the flat
  `ColumnInfo{name,dtype:String,nullable}` with a recursive shape (`{name, type_label, nullable, children}`) built by
  walking the Arrow schema on the engine side. Pretty-print each level (`struct`, `list<…>`, `map<k,v>`).
- Renders as an expandable schema tree in the Schema view.

---

## 6. Hive partitioning

- Layout: `…/year=2024/month=01/part.parquet`. Partition columns live in the **path**, not the files. They appear only
  when the **root dir** is registered as a table with partitioning configured.
- **Table-level & uniform.** One partition scheme per table; every path/file must conform. **Cannot mix Hive + flat, or
  ragged depths, in one table.** Mixed sets → suggest separate tables + a `UNION ALL` view (synthesising `NULL`partition
  columns for the flat side).
- **UX: detect-and-suggest, then confirm** (not silent auto, not bare toggle). On picking a directory, scan whether
  children look like `key=value/`; if so, pre-fill the partition toggle ON, list the detected columns each with a **type
  picker** (default string), user confirms/edits. If not detected, toggle off.
- **Drive it from the explicit API** (`with_table_partition_cols(vec![(name,
  type)])`), not `infer_partitions_from_path` — your scan proposes, the typed call registers deterministically. Types
  matter: auto-inferred partition columns default to `Utf8`, which breaks predicates like `WHERE year = 2024`.
- **Persist the resolved partition config** (concrete `(name,type)` list or none)
  in the table spec; do not re-detect on reload.

---

## 7. Data viewer (results grid)

- Scalar cells: string-formatted (current `ArrayFormatter` path).
- **Nested cells: emit real JSON, not Arrow display text.** Arrow's formatter produces `{name: Ada}` (unquoted, not
  parseable). Emit valid JSON per nested cell (via `arrow-json` or per-cell serialization) so the viewer can render it
  two ways:
    - **In the grid:** compact single-line JSON, monospace, truncated with an expand affordance (`{…}` / `[…]`). Column
      header shows a type badge.
    - **On click → detail panel:** full value as a **collapsible JSON tree** (no truncation). A whole-row "row detail"
      JSON view is the highest-value feature.
- **Null vs empty:** render null as a distinct muted `null` token, not blank — nested data has many nulls and empty
  lists.
- **Truncation:** keep grid truncation for layout; detail panel shows untruncated.
- Power users can `unnest()` / `get_field()` / `col[i]` to flatten at query time; the viewer must handle
  nested-as-returned regardless.

---

## 8. Query editor

Effort/priority order: **highlighting < formatting < autocomplete**.

- **Syntax highlighting** (easy — egui supports it). Use `TextEdit::layouter` to build a `LayoutJob` colored per token.
  Token source: `sqlparser` (bundled in DataFusion) `Tokenizer`. Memoize on a text hash (layouter runs every frame). Be
  error-tolerant on partial SQL; colors from the theme module. Alternatively evaluate `egui_code_editor` as a
  near-drop-in.
- **Formatting** (small crate). DataFusion has no beautifier (its `sqlparser`
  reserialization is single-line and requires valid SQL). Use the **`sqlformat`**
  crate — token-based, tolerant of partial SQL. On-demand (button/shortcut), not as-you-type; non-destructive on
  failure.
- **Autocomplete** (the one real project — egui gives nothing here). The *vocabulary* is free: tables/columns from the
  catalog (incl. nested), functions from DataFusion's `SessionState` function registries, keywords from
  `sqlparser`. The *widget* is a custom overlay (track current identifier, popup near caret, intercept
  Up/Down/Enter/Tab/Esc, splice into text). Tiers:
    - T0 (have it): click a catalog table/column to insert.
    - T1 (target): fuzzy word-completion popup over the union of the vocabulary.
    - T2 (defer): context-aware via `sqlparser` tokens (tables after FROM, etc.).

---

## 9. Data mutability

DataFusion's **entire** DML surface is `COPY` and `INSERT` — **no `UPDATE`,
`DELETE`, or `MERGE`**. Parquet is immutable by design.

- **Supported:** `COPY {table|query} TO 'file' [STORED AS …]` (write results out, optionally hive-partitioned);
  `INSERT INTO t …` (append new files to an external table).
- **The app is read/query + export.** The natural write feature is **export**:
  `COPY (SELECT …) TO 'file'` to save a result or transformed copy as new parquet/CSV — never mutating source data.
- **"Updating" = read → transform → rewrite** the file (s), then atomically replace (write to a new location, rename;
  never overwrite files being read). Treat any cell/row edit as "regenerate the file", with a backup and a clear
  warning — not spreadsheet-style live editing. Likely out of scope for v1.
- **True mutability** (row-level `UPDATE`/`DELETE`/`MERGE`, atomic commits, time travel) requires a **table format** —
  Delta Lake or Iceberg — via separate crates (`delta-rs`, `iceberg-rust`). Big architectural add; future only.

---

## 10. Project persistence — the `.ps/` format

### Layout

```
my-project/                    ← user opens this folder
├── .ps/
│   ├── manifest.json          ← format version, project name, created/modified
│   ├── settings.json          ← project-level defaults
│   ├── tables/                ← one external-table spec per file
│   │   ├── events.json
│   │   └── users.json
│   ├── views/                 ← one view per file
│   │   └── active_users.sql   ← raw SQL (+ optional .json sidecar for metadata)
│   ├── history.jsonl          ← append-only query log, rotated (keep last N)
│   ├── workspace.json         ← ephemeral UI state (open tabs, panel sizes)
│   ├── cache/                 ← inferred-schema cache (regenerable)
│   └── .gitignore             ← ignores workspace.json + cache/
└── data/                      ← optional: data living inside the project
```

### Rules

- **One file per object** (per table, per view). Git-diffable, no cross-object merge conflicts, and a corrupt spec
  isolates to its object.
- **Views as raw `.sql`** — most human-editable/diffable form.
- **History as `.jsonl`** — append-only (one line per query: sql, ts, status, duration); rotate to bound size.
- **Relative paths when data is under the project dir; absolute otherwise.** This is the portability decision — keep it
  explicit. `.ps/` references data by path; it does not contain the parquet.
- **Persist specs; re-derive the rest.** Schemas/column lists are re-inferred on load (a `cache/` copy is allowed but
  keyed on file mtime, never authoritative). Never persist results or previews.
- **Versioned** (`manifest.json.version`) for forward migration.
- **Atomic writes** — temp file in `.ps/` then rename over target. Autosave on mutation (add/edit table, save view, run
  query→append history), debounced.
- **Ephemeral vs durable** — `workspace.json` + `cache/` are per-user/regenerable and gitignored; committed project
  carries tables, views, settings, (optional)
  history. Decide whether history is shared or personal (leans personal).

### Table spec (`tables/events.json`)

```json
{
  "name": "events",
  "format": "parquet",
  "paths": [
    "data/events/"
  ],
  "options": {
    "file_extension": ".parquet",
    "skip_metadata": true
  },
  "partitions": [
    {
      "name": "year",
      "type": "Int32"
    },
    {
      "name": "month",
      "type": "Int32"
    }
  ]
}
```

### Reload model

Open project → read `.ps/` → **replay specs against a fresh `SessionContext`**, using the same async creation path as
first-time creation:

- **Order:** external tables first, then views (views depend on tables, possibly on other views → ideally topological;
  "tables then views in saved order" is fine for a prototype, with the caveat noted).
- **Resilient per item:** each object loads independently. External tables are live pointers to disk, so a
  moved/deleted/changed path fails just that table (`RegStatus::Failed` + a fixable-path message), never aborting the
  whole load.
- **Re-infer schemas** on load; optionally hydrate the UI from `cache/` first.

---

## 11. Engine & error model

- DataFusion is async; the engine runs on a Tokio thread. UI↔engine over channels:
  unbounded Tokio channel (UI→engine, non-async send), `std::mpsc` (engine→UI, drained per frame); engine calls
  `request_repaint()` on each event.
- **Table creation is async + `Result`-based** (I/O: list files, read footers, merge schema). Two failure tiers to model
  in the UI:
    - **Creation-time** (from the `.await`): bad path, unreadable/invalid file, schema-merge conflict among sampled
      files, name collision.
    - **Query-time** (deferred): unsampled bad/conflicting file; source file changed/deleted after registration. Surface
      these in the console and offer a
      "re-validate table" action — a distinct class from a failed creation.
- Result display is capped (stream, stop at N rows); status reports truncation.

---

## 12. Relationship to current code

Reuse as-is: the DataFusion engine (async command/event, sample-data generation), the theme, and the **Data / Schema /
Metadata** + **results grid** rendering components (repoint them from "a file" to "a table" / "a candidate in the import
wizard").

Change: the shell moves from **Explorer + standalone file-view tabs** to **catalog-centric + project persistence**. File
inspection folds into the create-external-table wizard rather than being a top-level mode. Optionally keep a lightweight
ephemeral "quick-preview a file without adding it" for casual peeks.

`ColumnInfo` changes from flat `dtype: String` to the recursive nested shape (§5).

---

## 13. Out of scope (v1) / future

- Namespaces/schemas (flat `public` catalog for now).
- In-memory/materialized (CTAS) tables.
- Remote object stores (S3, etc.).
- Delta Lake / Iceberg (true mutability, time travel).
- In-place data editing.
- Context-aware autocomplete (T2).

---

## Appendix A — DataFusion 43 API reference (verified)

Versions: `datafusion = "43"` (Arrow 53, `sqlparser` 0.51). Programmatic paths use
`datafusion::*`; SQL paths via `ctx.sql(...).await`.

**Register / read**

- `ctx.register_parquet(name, path, ParquetReadOptions::default()).await -> Result<()>`
  — `path` may be a file, a directory (append trailing `/`), or a **glob**
  (`/dir/**/*.parquet`). Single-format, single table.
- `ctx.read_parquet(paths, ParquetReadOptions::default()).await -> Result<DataFrame>`
  — DataFrame only, **no** catalog registration; `paths` accepts `&str`,
  `String`, or `Vec`.

**Multi-path external table (a set of files/dirs/globs → one table)**

```rust
use std::sync::Arc;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::datasource::file_format::parquet::ParquetFormat;

let paths = vec![
    ListingTableUrl::parse("/data/jan.parquet")?,
    ListingTableUrl::parse("/data/2024/")?,          // directory
    ListingTableUrl::parse("/archive/**/*.parquet")?, // glob
];
let options = ListingOptions::new(Arc::new(
ParquetFormat::default ().with_skip_metadata(true),
))
.with_file_extension(".parquet")
.with_table_partition_cols(vec![("year".into(), DataType::Int32)]); // if partitioned
let config = ListingTableConfig::new_with_multi_paths(paths)
.with_listing_options(options)
.infer_schema( & ctx.state()).await?;   // or .infer(&ctx.state()) for options+schema
let table = ListingTable::try_new(config) ?;
ctx.register_table("events", Arc::new(table)) ?;
```

- `ListingTableConfig`: `new(url)`, `new_with_multi_paths(Vec<ListingTableUrl>)`,
  `with_listing_options`, `with_schema(SchemaRef)`, `infer_schema(&state).await`,
  `infer(&state).await`, `infer_partitions_from_path(&state).await`.
- All paths must share one object store and one partition scheme.
- `ParquetFormat::infer_schema` = "infer the common schema… may fail if schemas cannot be merged"; samples up to a
  configured file/record limit.

**Schema introspection**

- `df.schema().fields()` → recursive `Field`s; `field.data_type()` yields nested
  `Struct`/`List`/`Map` DataTypes to walk.

**Execute / stream**

- `ctx.sql(sql).await -> Result<DataFrame>`; `df.execute_stream().await` (stream, cap rows); `df.limit(skip, Some(n))`;
  `df.collect().await`.
- Cell display: `arrow::util::display::ArrayFormatter` (scalars). For nested cells use `arrow-json` to emit parseable
  JSON (§7).

**DDL / DML (SQL)**

- `CREATE [UNBOUNDED] EXTERNAL TABLE [IF NOT EXISTS] name [(cols)] STORED AS
  {PARQUET|CSV|JSON|ARROW|AVRO} [PARTITIONED BY (cols)] [OPTIONS(...)] LOCATION 'p'`
  — Hive partitions auto-detected when pointing at a hive-layout root.
- `CREATE [OR REPLACE] VIEW name AS <query>`; `DROP VIEW [IF EXISTS] name`.
- `CREATE DATABASE` (catalog) / `CREATE SCHEMA` (namespace) exist — deferred (§2).
- `COPY {table|query} TO 'file' [STORED AS fmt] [PARTITIONED BY (...)] [OPTIONS]`.
- `INSERT INTO name { VALUES (...) | query }` (append). **No UPDATE/DELETE/MERGE.**

**Editor tooling**

- Highlighting: `sqlparser::tokenizer::Tokenizer` (bundled) → egui
  `TextEdit::layouter` `LayoutJob`.
- Formatting: `sqlformat` crate (separate dep).
- Autocomplete vocabulary: catalog (app state) + `SessionState` function registries + `sqlparser` keywords.

### Sources

- DDL (external tables, views, Hive partitioning): https://datafusion.apache.org/user-guide/sql/ddl.html
- DML (COPY, INSERT; no UPDATE/DELETE): https://datafusion.apache.org/user-guide/sql/dml.html
- `ListingTableConfig`
  (v43): https://docs.rs/datafusion/43.0.0/datafusion/datasource/listing/struct.ListingTableConfig.html

---

## 14. Strata addendum (current source of truth)

The implemented app follows the `Strata.dc.html` prototype. This section overrides earlier stack/UI notes.

### 14.1 Stack

- **Dioxus 0.7 desktop** — components in Rust/RSX, styled with the prototype's CSS injected at the app root
  (`assets/main.css`). One
  `Signal<AppState>` provided via context; components read it and call controller actions in `app.rs`.
- **DataFusion 43** engine on a dedicated thread (own Tokio runtime); UI↔engine over `tokio::mpsc::unbounded` channels;
  a Dioxus coroutine drains events into the state signal.
- **`dioxus-code` / `dioxus-code-editor`** (tree-sitter) provide highlighting:
  `CodeEditor` for the SQL editor, `Code` for the nested-cell JSON popover.

### 14.2 Design tokens (from the prototype)

- Surfaces: bg `#0b0e13`, panel `#0e121a`, main/results `#090c11`, elev `#12161f`
  / `#161b25` / `#1b212c`. Lines `#1c222c` / `#262e39` / `#2a3340`, hover
  `#37424f`.
- Text: `#e7ebf1` / `#c3cbd6` / `#aeb6c2` / dim `#8b95a3` `#6a7482` `#5a6472` / faint `#4a5462` `#3f4854`.
- Accent `#4cc6ff` (options `#7c8cff #4ade80 #fb923c #f472b6`); ink `#071019`. Green `#4ade80`, purple (views)`#d2a8ff`,
  red `#ff9aa2`/`#f87171`, orange
  `#ffa657`.
- **Type → colour** (dot + text + cell): str `#7ee787`, num `#79c0ff`, bool
  `#d2a8ff`, ts `#ffa657`, struct `#f0a5c0`, list `#8ad4ff`, map `#ffcf6b`. Cell text tints: num `#9fc6ff`, bool
  `#d2a8ff`, ts `#e2b98c`; nested cells are clickable → JSON popover.
- Fonts: **IBM Plex Sans** (UI 400/500/600), **JetBrains Mono** (code/labels 400/500/700). Base 13px.
- Metrics: header 48px, workspace tabs 38px, results toolbar 40px, pager 44px, status 26px; sidebar 288px, inspector
  292px; editor ~178px. Radii: buttons 8, small 6–7, cards 9–11, modals 14–16, badges 4–5. Density (`--rowpad`):
  Comfortable `9px 14px` / Compact `5px 14px`. Theme props: accent, density, zebra, type-colour-cells.

### 14.3 UI surfaces

Header (logo + wordmark + `DataFusion 43` badge + project switcher + ⌘K) · Sidebar catalog (filter; TABLES with
New→Config; expandable columns with type dots, PART badges, SELECT */config/remove; VIEWS with SELECT */edit/remove) ·
Workspace (query tabs; `CodeEditor`; run bar Run ⌘↵ / Format / Clear / Save as view; results toolbar find + Export;
results grid with type-coloured cells; pager with page-size + first/prev/next/last) · Column inspector (type, stats over
the current result, nested-field tree, completeness, numeric range) · Status bar. Modals: **Table Config** (name,
format, multi-path sources with browse/add/remove, validation idle/validating/error, Hive toggle + typed partition
columns + string-cast warning), **Export** (format cards, options, destination, preview via
`COPY … TO`), **Command palette** (⌘K), **History** (right drawer), **nested-cell JSON popover**.

### 14.4 SQL editor DDL policy (implemented in `ddl.rs`)

Classify the statement *before* `ctx.sql` (DataFusion executes DDL eagerly):

- **Allow → run**: `SELECT` / `WITH` / `EXPLAIN` / `SHOW` / `DESCRIBE` / `VALUES`.
- **Allow → capture into the project**: `CREATE [OR REPLACE] VIEW`, `DROP VIEW`.
- **Block → use Table Config**: `CREATE EXTERNAL TABLE`, `CREATE TABLE` / CTAS,
  `INSERT`/`UPDATE`/`DELETE`/`MERGE`, `COPY` (→ use Export), `ALTER`,
  `DROP TABLE`.
- **Hard-block**: `CREATE DATABASE`, `CREATE SCHEMA` (outside the flat-catalog model).

Blocked statements surface a helpful reason in the status bar and do not execute.

### 14.5 Files

```
assets/main.css      design system (tokens + component CSS)
src/main.rs                    Dioxus launch + CSS injection
src/app.rs                     root component, engine bridge, controller actions
src/state.rs                   AppState (+ seed matching the prototype)
src/engine.rs                  DataFusion engine (channels, multi-path register,
                               view create/drop, typed/nested results, sample gen)
src/ddl.rs                     DDL policy classifier
src/util.rs                    Kind (type→colour), helpers
src/ui/{header,sidebar,workspace,inspector,statusbar,modals,icons}.rs
```

- `ParquetFormat`
  (v43): https://docs.rs/datafusion/43.0.0/datafusion/datasource/file_format/parquet/struct.ParquetFormat.html
