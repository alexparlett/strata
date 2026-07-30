# P4-11 · Configure-table window (register / edit + import options)

**Phase:** 4 · **Status:** ✅ `[core ✓]` · **DEV_TASKS:** U14 / D7 / D8 · **Depends on:** —

## Goal
The window that registers and edits an external table: name, format, source paths, format-specific
**import (read) options**, and Hive partition columns — built to `Configure.dc.html`.

> **This folds the old P4-11 (register/edit) and P4-12 (import options) into one task.** They were
> split when the import block was believed to be a later addition to a modal that already existed.
> Neither is true: nothing is built, and the two halves are not separable — the format dropdown
> *is* what selects the option set, the option set changes the file-extension filter (compression),
> and both land in the same `TableSpec` through the same Save. Two tasks would mean building the
> draft, the persisted def and `register_external`'s format construction twice.

> **This adds a new `project.json` mutation site** — the third def writer after Save and the drop
> confirm. Route its persist through **P4-15**'s funnel (today: P3-13's `actions::persisted`, which
> logs the failure and returns whether the write landed), and gate the window's own success on the
> answer. Do **not** copy the surrounding `if let Err(e) = … { tracing::error!(…) }` idiom — that
> silence is exactly what P4-15 exists to remove, and a registration the project file never heard
> about reverts on the next open.

## Built
`apps/configure/` (root · model · seven views), `platform/configure.rs`, and the two triggers: a
catalog row's **Configure** and the TABLES section's **+**. Core grew the typed
`SourceFormat`/`CsvRead`/`JsonRead` the def and the spec now carry, `detect_partitions`, and the
read-option wiring in `register_external`.

`strata_model::form::ConfigForm` — the Dioxus-era draft, which said in its own doc comment that it
went with this task — is **deleted**, and `strata-model/src/form.rs` with it: a draft belongs to
its window, exactly as `ExportDraft` does. Its `all_dirs` / `file_count` / `scanning` /
`scan_error` fields were the pre-flight scan D9 already dropped, and are not rebuilt.

## Location: local disk only — do not build the toggle
The canvas opens with a **LOCATION** segmented control (Local disk · Object store) and, in the
remote arm, a provider pill, a CONNECTION picker, a bucket prefix on the path box and a
single-path variant of the list. **None of it is built here.** Connections do not exist, so the
toggle would offer one option, and a one-option toggle is a control that cannot be operated.

Leave the section out entirely rather than shipping it disabled: everything downstream of
`cfgMode` in the VM (`multiPath` / `singlePath` / `showPrefix` / `bucketPrefix` /
`pathsLabel` / `pathPlaceholder`) collapses to its local branch, so the path list is simply the
path list and the label is simply `SOURCE PATHS`. **W7 ▸ 04** adds the toggle and the remote
branch back; its file records what it re-introduces.

## Settled during the build

Four things this task decided that were not in the plan, each because the plan turned out to be
wrong about something:

- **No per-window theme.** The window carries no `define_theme!` block of its own. Its chrome is
  the app's **sheet** (`background` · `surface_*` · `border` · the four semantic slots) and
  everything form-shaped is the shared `form` theme. A sixteen-field block per window resolving to
  the same handful of sheet slots is four blocks to keep in step for one reskin — which is exactly
  what a shared vocabulary exists to prevent. **The three windows that still carry one (export,
  settings, launcher) predate this and should follow**; that is a task of its own, and until it
  lands the app is inconsistent in the direction of the new rule rather than the old one.
- **`REQUIRED` is a `Row`, not a label.** The marker went on `components::form::Row::required()`
  in both registers, beside the title and the explanation it belongs with, rather than as a
  per-window label component. A window that drew its own would be a window whose label line drifts
  from every other one's.
- **A trigger sets a slot; one place opens the window.** A catalog row's menu is built inside an
  event handler, where no hook may run, so every handle it will need has to be resolved at the
  *row's* render — and opening a window needs the window's app-globals, engine and log, which no
  row has any business holding. So `views::configure_launch` is the drop-confirm shape: the
  trigger sets `ConfigureRequest` and stops, and `ConfigureLauncher` (mounted at the project root,
  where those handles live) acts on it. Adding a trigger is setting the slot.
- **No ADVANCED disclosure, though the canvas draws one.** It was built with the accordion the
  canvas specifies and then flattened (Alex): the export canvas folded its own away because a
  format's advanced controls are just more of that format's options, and that reasoning is not
  specific to exporting. In a window whose entire subject is how a file is read, the split is one
  more thing to open before reaching a CSV's quote character. Both windows are now the same
  shape, which is worth more than either canvas's local choice — and it is why `OptionList` has
  no disclosure of its own to inherit.
- **Closing discards the draft, and does not ask.** Cancel, Esc and the red button all close
  outright — including mid-registration, where the pass belongs to the project window's driver
  and lands on the catalog row whether this window is watching or not. A dirty-close confirm was
  considered and **declined** (Alex): nothing here is written until Save, so what a close costs
  is a form, not data, and the window is light enough that guarding it would be friction on
  every dismissal to protect the rare one. Don't add one back without a reason that isn't
  symmetry with the T2 confirm — that dialog exists for *running queries*, which this window
  never has.
- **Browse is one button with two answers.** `NSOpenPanel` is configured for files *or* folders,
  never both, so the canvas's single "Browse… (file or folder)" is a button opening a two-item
  menu. Picking files is multi-select, because a table *is* many paths.

## Shape: a window, not a modal
The canvas is a 620 × 640 **window** — traffic lights, drag bar, resize grip, its own footer — not
an in-project modal, and the old task titles ("config modal") predate that. Build it as
`apps/configure/`, on P4-10's export-window shape: a native child window of the project window that
asked, pinned above it (`platform/configure.rs`, mirroring `platform/export.rs`), closing when its
owner leaves the registry, and skipped by `Windows::is_last()`.

Where it differs from Export is **single-instance, keyed by target**. Export deliberately has no
such rule because each window carries a different run. A Configure window carries a *def*, and two
windows on one def would both `upsert_table` + `save_defs`, so the second silently reverts the
first — the same reason two windows cannot share a project. So `open_configure` focuses an open
window whose target matches, where the target is `New` or `Edit(name)`. Two different tables may be
open at once; one table twice may not.

Its geometry is not persisted (like Settings and the launcher, unlike the project window), so the
title bar's double-press-to-fill is not wired.

## Build, to the canvas
Body order, top to bottom (`Configure.dc.html` lines 53–334) — this is the **whole** contract, and
it supersedes DEV_TASKS D7's "status below import-options, above Hive": on the canvas the busy and
error blocks are the **last** things in the scroll body, after Hive. The canvas is newer; follow it.

1. **TABLE NAME** (eyebrow + `REQUIRED`) beside **FORMAT** — a 128 px `Select`. Formats are
   **parquet · csv · json · arrow**, four not the canvas's five: there is no Avro in the build
   (no `datafusion-datasource-avro` in `Cargo.lock`), and `register_external`'s `_ =>` arm would
   read an "avro" table as parquet without saying so. While there, make that arm exhaustive rather
   than a fallthrough — a format string with no reader must fail the register, not become parquet.
   Name validation is `ProjectState::name_in_use` (tables and views share one namespace,
   case-insensitive); on an edit, the def's own name does not clash with itself.
2. **SOURCE PATHS** (eyebrow + `REQUIRED` + the ⓘ resolution tooltip, whose body is spelled out in
   the canvas: a file, a directory, a recursive glob, one path per row, combined into one table).
   Drop the subtitle (D7). Then the three-button 28 × 28 toolbar — add · remove · browse — over a
   bordered list of path rows with an empty state ("No paths yet — add one to point at your data.").
   The remove button targets the **selected** row; a row is selected by pressing it.
   > `components::form::DirectoryField` is the path-box-plus-picker, but it picks a *folder* and a
   > source path may be a file or a glob. Widen it with a pick mode rather than adding a second
   > control beside it — one buffer, typed or picked, is what keeps the box and the value from
   > disagreeing. The canvas puts Browse in the toolbar (acting on the selected row) rather than on
   > each row, which is the same component with its button lifted out; decide which shape the
   > widened `DirectoryField` supports before writing the list.
3. **Import (read) options** — format-specific, **one flat list** (see "Settled during the
   build": the canvas's ADVANCED accordion is deliberately not built). Hidden for parquet and
   arrow.
4. **HIVE PARTITIONING** — header + subtext on their own line, enable `Switch` below (Export's
   PARTITION BY COLUMNS rhythm), then one row per detected column: the column name and a pill of
   `Utf8 · Int32 · Int64 · Date32`. The string-cast warning shows while any column is left `Utf8`.
   The canvas's subtext says "the type **DuckDB** should read each partition column as" — a
   leftover from an earlier engine. Reword; nothing in Strata is DuckDB.
5. **Status** — the validating spinner and the error box, in that order, at the end of the body.
   Success has no block: it is the window closing. The error's detail is whatever
   `register_external` returned (P3-07 maps it inside the engine); **do not grow a second set of
   messages here**, and do not add a pre-flight file-count or schema-consistency readout — D9
   settled that the Register *is* the check.
6. **Footer** — Cancel · Save. Save is disabled while the name is blank, while every path is blank,
   and while a registration is in flight; its label is "Save" / "Validating…" and the busy label is
   "Registering table…" / "Updating table…".

Standard components throughout (AGENTS §3): the format picker is `Select`, the toolbar buttons are
`Button::new().flat()` at 28 × 28, the toggles are `Switch`, the type pills and the delimiter pills
are one `SegmentedToggle`. The eyebrow-label-over-control rhythm is `components::form`'s
`Variant::Fields` — the same register the export window uses, which is what this surface is.

## Import (read) options — the DataFusion 54 validation pass
The canvas's option set was drawn before anyone checked it against the engine. It was checked for
this task, against `datafusion 54.0.0` as vendored (`datafusion-datasource-csv`,
`-json`, `-parquet`, `-arrow`, `datafusion-common::config`, `arrow-csv 58.4.0`). Three canvas
options are wrong and two real ones are missing. **The verdicts below are the build list.**

The test applied is not "does `CsvOptions` have this field" — most of that struct is the *writer's*
— but "does it reach the read path", which means both halves of the read: `CsvFormat::infer_schema`
(→ `infer_schema_from_stream`, `datafusion-datasource-csv/src/file_format.rs:517-560`) and the scan
(`CsvSource::builder`, `.../source.rs:187-209`). An option wired into one and not the other is
listed as such, because the schema and the rows then disagree.

### CSV

| Canvas control | DataFusion 54 | Read-effective | Verdict |
|---|---|---|---|
| HEADER ROW (toggle) | `CsvFormat::with_has_header` | infer + scan | **build** — core |
| DELIMITER (seg + custom) | `with_delimiter(u8)` | infer + scan | **build** — core. It is a **byte**: refuse a multi-byte custom character rather than truncating it |
| NULL VALUE (text) | `with_null_regex(Option<String>)` | **infer only** | **do not build** — see below |
| QUOTE CHAR | `with_quote(u8)` | infer + scan | **build** — advanced |
| ESCAPE CHAR | `with_escape(Option<u8>)` | infer + scan | **build** — advanced |
| COMMENT CHAR | `with_comment(Option<u8>)` | infer + scan | **build** — advanced |
| NEWLINES IN VALUES (toggle) | `with_newlines_in_values(bool)` | scan | **build** — advanced. It works by turning off file splitting (`CsvSource::supports_repartitioning`), which is why the canvas hint says "slower scan" |
| SCHEMA-INFER ROWS (num) | `with_schema_infer_max_rec(usize)` | infer | **build** — advanced. The canvas hint "0 = read all as text" is **correct** here (`build_schema_helper`'s `disable_inference` arm) |
| COMPRESSION (seg) | `with_file_compression_type` | infer + scan | **build** — advanced, *and* it must move the file-extension filter (below) |
| — | `truncated_rows: Option<bool>` | infer + scan | **add** — missing from the canvas |
| — | `terminator: Option<u8>` | scan only | **do not build** |
| — | `double_quote`, `quote_style`, `null_value`, `date_format` / `datetime_format` / `timestamp_format` / `timestamp_tz_format` / `time_format`, `ignore_leading_whitespace`, `ignore_trailing_whitespace`, `compression_level` | none | not options — writer-only; no read-path reference exists in `datafusion-datasource-csv` |

**NULL VALUE is inference-only, and building it makes things worse.** `arrow-csv` supports a null
regex at parse time (`ReaderBuilder::with_null_regex`, consumed by `build_primitive_array`), but
DataFusion 54's `CsvSource::builder()` never wires `options.null_regex` onto the reader — it is set
only on the `Format` used for inference (`file_format.rs:548`). So `1,2,NA` with a null value of
`NA` infers `Int64` (inference sees a null) and then **fails the scan** parsing `NA` as `Int64`. The
same column with the option off infers `Utf8` and reads. A control whose only effect is to break a
table that otherwise works is not a control; leave it out and say why in the module. (The common
case is already covered: an empty field is null in `arrow-csv` regardless.) Revisit if a later
DataFusion wires it through — that is a one-line check at `source.rs`'s `builder()`.

**TRUNCATED ROWS earns its place precisely because this table is multi-path — and its absence is
the worst of the three, because it does not fail the register.** Measured, not assumed (the first
version of this note claimed it was needed for the *register* to succeed, and the round-trip test
written to prove that disproved it): schema inference merges differently-shaped CSV files happily,
so the table comes back with the union of the columns and looks perfectly registered. It is the
**scan** that then fails, on every query, for the files short of a column — `Csv error: incorrect
number of fields for line 1, expected 3 got 2`. With the option on, the missing columns are padded
with nulls and the same table reads. (Within a *single* file, a ragged row does fail the register.)
So this is the option whose absence produces a catalog row that looks fine and cannot be read.
Advanced group.

**TERMINATOR is wired at scan but not at inference** (`infer_schema_from_stream` sets header,
delimiter, quote, truncated-rows, null-regex, escape and comment — not terminator). A file with a
non-`\n` terminator would infer as one giant column and then read as many, so the option cannot be
offered honestly. Same one-line re-check as above if it ever lands.

### JSON

| Canvas control | DataFusion 54 | Read-effective | Verdict |
|---|---|---|---|
| — | `JsonFormat::with_newline_delimited(bool)` | infer + scan | **add as the core group** — see below |
| SCHEMA-INFER ROWS (num) | `with_schema_infer_max_rec(usize)` | infer | **build** — advanced, **minimum 1** |
| COMPRESSION (seg) | `with_file_compression_type` | infer + scan | **build** — advanced, + the extension rule |

**`newline_delimited` is the most valuable read option we have, and the canvas predates it.**
DataFusion 54 reads a whole-document JSON array (`[{…},{…}]`) when it is set to `false`
(`JsonFormat::with_newline_delimited`, carried into both `infer_schema` and `JsonSource`). Today
such a file is simply unreadable in Strata, and `catalog::json_shape_error` tells the user so as a
*rule* — "JSON sources must be newline-delimited, one record per line". With the option built, that
message is wrong: it must point at the option instead ("… or set the JSON shape to array in Table
Config" — one sentence, the register's own register). Build it as the JSON **core** group, a
two-value pill: newline-delimited (default) vs JSON array.

One caveat to record beside it, not to hide: array mode cannot be range-split, and unlike
`CsvSource`, `JsonSource` does **not** override `supports_repartitioning`. So an array-mode file
larger than `datafusion.optimizer.repartition_file_min_size` (default 10 MB) fails at *query* time
with DataFusion's `NotImplemented` ("JSON array format does not support range-based file
scanning"). The register still succeeds. This is a loud, self-describing failure rather than a
wrong answer, which is why it is acceptable where the NULL-value control was not — but it is a
known edge, and turning `repartition_file_scans` off engine-wide to hide it is not the fix.

**Infer-rows has no `0` meaning for JSON.** `JsonFormat::infer_schema` breaks out of its loop
before reading anything when `records_to_read == 0`, producing a table with no columns — so the
CSV hint must not be reused. Floor the field at 1 and treat blank as "engine default"
(`DEFAULT_SCHEMA_INFER_MAX_RECORD`).

### Parquet and Arrow — no import block, and that is a finding
`ArrowFormat` has **no options at all** in DataFusion 54 — no builder methods, not even compression
(it lives in the IPC container). `ParquetFormat` has plenty (`pruning`, `enable_page_index`,
`pushdown_filters`, `metadata_size_hint`, `skip_metadata`, `binary_as_string`, `coerce_int96`,
`force_view_types`), but they are engine-wide performance and compatibility settings and eight of
them are already `ENGINE_KEYS` entries under `datafusion.execution.parquet.*` — i.e. **P4-07's**.
A per-table copy would be a second place to set the same key, which is the drift AGENTS §5 exists
to prevent. So the canvas's `csv || json` gate on the whole block is right, for a better reason
than it knew.

### Compression moves the file-extension filter
`register_external` hardcodes `.with_file_extension(".csv")` / `".json"`. A gzipped CSV is
`events.csv.gz`, which does not match `.csv`, so the listing comes back empty and the user is told
"No files matched" for files that are right there. DataFusion's own answer is
`FileFormat::get_ext_with_compression` — format extension **plus** `FileCompressionType::get_ext()`
(`.gz` · `.bz2` · `.xz` · `.zst` · empty). Build the extension from the spec's compression in the
one place that builds it, and make `catalog::no_files_error`'s extension advice use the same
combined string — it currently reasons about `ext` and would name the wrong one.

## Core changes
- **`TableSpec` and `TableDef` grow read options.** Typed per format, not a string map:
  `ReadOptions::{None, Csv(CsvRead), Json(JsonRead)}`, so an option that a format ignores cannot be
  set on it (AGENTS §1 — model impossible states out of existence). `#[serde(default)]` on the def
  so existing `project.json` files load, and every default must be *DataFusion's* default, so a def
  written before this task registers exactly as it does today.
- **The def persists the active format's options; the draft keeps every format's side by side** —
  `ExportDraft`'s split, for the same reason: flipping the format dropdown while deciding must not
  discard what was typed, but the file should not carry CSV settings for a parquet table.
- **`register_external` builds the format from the options** and is still the only place DataFusion
  is touched. Its `match spec.format` becomes exhaustive (no parquet fallthrough) and its file
  extension becomes format + compression.
- **Options are data.** Reuse P4-10's shape — `ImportDraft::groups() -> Vec<Group>` with each option
  carrying the `Edit` it performs, rendered by one component per control *shape* (toggle, seg,
  char, text, num — exactly the canvas's `isToggle`/`isSeg`/`isChar`/`isText`/`isNum`), so a new
  option is a row in a table and no control can write the wrong field. `Group` / `Control` / the
  option views live in `apps/export/model.rs` + `views/options.rs` today; this is their second
  consumer, so **lift them to a shared home** (with `components::form`) rather than copying — and
  note it in P4-10's file. Unit-test the groups without a renderer, as the export draft is.

## Save
On Save: build the `TableSpec`, `upsert_table(def)`, persist through the funnel (**gate on the
answer**), `engine.register(spec)`, land it with `table_registered` / `table_failed`, then re-create
the dependent views (`views_to_refresh` + `refresh_order`) — a re-registered table invalidates the
views over it, which `table_registered` already knows. Record the outcome with `log_event`: this
layer observed it, so this layer writes it (AGENTS §2 — a log has no producer hook). On success the
window closes; on failure it stays open with the error block and the def as the user typed it.

**An edit that changes the name is a rename**, not an upsert: deregister the old name in the engine
and `remove_table(old)` before the insert, or the catalog keeps a row and a registration nobody can
reach. Views written against the old name break — that is the user's edit, and the failure lands on
their rows through the normal re-registration path; do not try to rewrite their SQL.

## Acceptance
- [x] Register a table over one or more paths / globs with a format and typed Hive partition
      columns; the REQUIRED badges and the resolution tooltip are present; a failure shows
      `register_external`'s own message and leaves the window open.
- [x] CSV shows one flat list (header · delimiter · quote · escape · comment ·
      newlines-in-values · ragged rows · infer rows · compression); JSON shows shape · infer-rows
      · compression; parquet and arrow show no import block.
- [x] Every one of those values reaches `TableSpec`, changes what is read, and survives a
      project reopen. A gzipped CSV registers from a `.csv.gz` file.
- [x] A whole-document JSON array registers and queries with shape set to array.
- [x] Configure on a table already open in a Configure window focuses that window.

**Tests.** 12 engine round-trips (`strata-core::engine::read_options_tests`) asserting each
option's *effect* rather than its call; 5 serde tests over the persisted format (including the
legacy bare-`"format"` string and a legacy `avro`); 4 over partition detection; 19 over the draft
(`apps::configure::model`). The whole workspace stays green, and `schema_in_sync` covers the one
theme field this added (`form.required_color`).

**Known interim panic** (same family as P4-01 item 5's, now-built read-side handling): a target
table gone between the open and the first render panics in `ConfigureCtx`'s initializer
(`apps/configure/mod.rs`, "no such table in this project"). P4-01's mechanism generalizes — the
fallible resolve in one `use_hook` at the window root, an `Err` arm rendering the fault dialog —
but the close differs: a Configure window is never the last workspace window, so it is a plain
`platform.close_current_window()`, no launcher rule. Fold in whenever this surface is next
touched.

## Freya / references
- Design: `Configure.dc.html` (markup + the `cfg` VM), `strata-windows.js` `SW.importOptsVM` /
  `SW.cfgView` / `SW.makeCfgHandlers` for the option list and the draft's shape.
- Core: `TableSpec` / `register_external` ([catalog.rs:63](crates/strata-core/src/engine/catalog.rs:63)),
  `ProjectState::{name_in_use, upsert_table, remove_table, table_registered, views_to_refresh}`.
- Window shape: `platform/export.rs` + `apps/export/` (P4-10). Form vocabulary:
  `components::form` (P4-05). Persist funnel: `actions::persisted` → **P4-15**.
- DEV_TASKS U14 (restyle) / D7 (honesty tidy) / D8 (import options) — all three land here.
- **W7 ▸ 04** owns the LOCATION toggle and the object-store branch this task leaves out.
