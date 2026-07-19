# Strata — Feature Spec (implementation handoff)

Redesign of the local-Athena parquet viewer (Rust/egui + DataFusion). This lists every feature in the `Strata.dc.html`
design concept so they can be implemented in the real app. Grouped by area; each item notes the intended behaviour and
the DataFusion/engine hook where relevant.

> **Name:** the product is **Strata** — the icon is uneven sedimentary layers (data strata).

---

## 0. Two-window model (launcher + main)

Follows the JetBrains (RustRover/IntelliJ) pattern:

- **Launcher / Welcome window** — shown when no project is open. Left rail: brand + a single **Projects** item (Learn /
  Settings removed until they have content). Main area:
  **Open…** action + a **RECENT PROJECTS** list (name, path, branch, color dot), the current one flagged.
- Opening a project **closes the launcher and opens the main window**; **Close project**
  (File menu) returns to the launcher. These are conceptually separate windows.
- **Open… is the only entry point** — there is no separate "New project". Open points at a folder: if a `.ps/` project
  dir exists there it's opened; if not, one is created from the folder name. (Open-creates-if-missing.)

## 1. App shell / layout

- Three-region workspace: **left catalog sidebar**, **center editor + results**, **right column inspector**.
- **Collapsible sidebar** — collapses to a **thin left rail** (RustRover-style), not fully hidden; the rail keeps a
  reopen affordance. Persist collapsed state.
- Top header: project switcher · engine badge (`DataFusion 43`) · command-palette search field.
- **Tab strip context menu** works **anywhere on the bar** (empty space included), not only on a tab; right-clicking a
  tab targets that tab. Menu uses a **vertical** ellipsis affordance.
- Bottom status bar (left): registration/exec status, and a click target that opens the event log. The old bottom-
  **right** status readout was removed (stats live in the results strip).
- Theme is themeable (see Settings). Density, zebra striping, type-colored cells are togglable.

## 2. Event log (bottom panel)

- The bottom-left status is clickable and opens an **expandable bottom panel** (full-width, pushes content up — **not**
  a floating modal).
- Header shows `Event log · N`; controls: **Clear**, expand/restore, close.
- Entries are seeded and appended on real events: query execution (success/failure), table register/validate, exports.
  Each row: status dot, message, relative time.

## 3. Project model & persistence

- Everything lives under a **Project**.
- A project persists **definitions and state, not data**: external-table defs (name → sources, format, options,
  partition spec), saved views, **saved queries**, query tabs + history, UI state.
- **Project is concrete files under a `.ps/` directory in the opened folder** — not opaque app storage. This makes
  projects **committable to source control and shareable** with a team.
- Files stay on disk; project stores pointers (reference model). **Reference is the default.**
- Optional (future) **import** = copy files into project storage for portability; **materialize** = persist an expensive
  query result as parquet in the project cache.
- Header **project switcher** menu: Open… + RECENT PROJECTS list.
- Autosave: definitions persist on change; scratch SQL carries a dirty indicator.
- Reference projects reference absolute local paths — surface this; sharing the `.ps/` dir shares the definitions, not
  the data files.

## 4. Catalog (sidebar)

- Sections: **TABLES**, **VIEWS**, **QUERIES**, each with a count.
- **Filter catalog** search box at top.
- Each entry badged by **kind** (table / dataset / view icons) and, for tables, shows meta (`248 files · 3 partitions`,
  `9 rows · 1 file`).
- **Row menu is user-friendly** — not a bare "SELECT *". Primary click opens the table; a (vertical-ellipsis) row menu
  offers the labelled actions: open, **Configure** (tables only — label is "Configure", not "Configure sources…"), edit
  query (views), rename, **Remove**.
- **Remove table/view/query requires a confirmation dialog** (easy to misclick otherwise).
- **New** button in the TABLES header → opens table configuration for a new table.
- Views section is query-backed · always persisted; each view has open, **edit-query**, and **remove** actions mirroring
  the table row layout.

## 5. Saved queries (distinct concept)

Saved queries are their own catalog concept, separate from **views** and from **history**:

- A **view** is a queryable table (`SELECT * FROM it`); **history** is the transient auto-log; a **saved query** is a
  named SQL snippet you keep and reopen.
- **Save query** action in the run bar and **⌘S** save the active tab under a name into the project's **QUERIES**
  section.
- Clicking a saved query opens it as a tab. Saved queries appear in the command palette.
- **No row-count** shown on saved queries (a saved query isn't a materialized result — a count would be misleading).

## 6. Tables = one logical table over many files (ListingTable)

- A table maps to DataFusion's `ListingTable` / `CREATE EXTERNAL TABLE` — **one table, many source paths**.
- **Configure** modal (gear per table, or New):
    - Table **name**.
    - **Format** dropdown: PARQUET / CSV / JSON / AVRO / ARROW.
    - **Source paths** list — each row is a file, a directory, or a recursive glob (`**/*.parquet`). Per-row type
      auto-detected and shown; per-row **Browse…** (native file/folder pick) or type a glob; add/remove rows.
    - Live resolution readout: `N paths · M files matched · schema consistent`.
    - **Hive-style partitioning** toggle: auto-detects `key=value/` path segments and the `**/*.parquet`
      lake convention, pre-fills partition columns.
        - Each partition column has a **type picker** (Utf8 / Int32 / Int64 / Date32), defaulting to Utf8 with a warning
          that string partitions make `WHERE year = 2024` need a cast.
        - The resolved `(name, type)` list is persisted into the table spec (source of truth for deterministic reload)
          rather than re-detected.
    - Footer: `references files on disk · persists in project`; Cancel / Save (or Register) actions.
- **Async register/validate**: on Save/Register the button shows a spinner + "Validating…" while it reads files and
  infers schema, then either succeeds (closes; status "schema validated") or fails with a **specific error**, keeping
  the modal open. Error cases include:
    - a `.parquet` path under a CSV-typed table (format/path mismatch),
    - no files matched,
    - **can't mix Hive-partitioned data with a flat file** — every path must share the same partition layout (same
      keys + depth).
- **CSV/JSON import options** (future/optional): delimiter, header row, null token, JSON records path.

## 7. Homogeneous-folder rule (JSON/CSV as one table)

- A folder of like-shaped files (e.g. 100 same-schema NDJSON files) registers as **one table** by default — same as
  partitioned parquet.
- "Register each file as its own table" is the rare escape hatch, not the default.
- **JSON shape detection**: newline-delimited JSON (one record per line, dir-as-table) vs whole-document `.json` files
  (each file = one record / not tabular) — detect and state which.
- **Schema-consistency check** before committing to one table: report `100 files · schema consistent ✓`
  or `97 match · 3 have an extra column`.

## 8. Column inspector & Arrow nested types

- Clicking a catalog column selects + inspects it (right panel), and expands nested types inline.
- The inspector is **free metadata only** — it does **not** fabricate column statistics.
    - **Removed**: the profile/histogram/distinct/mean stats. Those require scanning the column, which on a large
      parquet table is an expensive query — and this is a query tool: if the user wants a profile they write
      `SELECT count(distinct …), avg(…) …`. A button that fakes a scan is just a worse query editor.
    - Inspector shows: name, Arrow type, source, and schema-derived metadata; **no value distribution**.
- **Nested Arrow types** fully supported: `Struct`, `List<...>`, `Map<K,V>`, and arbitrary nesting.
    - Catalog renders nested types with distinct type colors; **expandable inline to any depth** — chevron toggles, and
      expanded nested **types can be collapsed again** (fixed).
    - For a nested column the inspector shows a **NESTED FIELDS** tree (field name + Arrow type, indented by depth),
      with a note on the container kind.
- **Partition columns** badged **PART** in the schema and called out as prunable.

## 9. SQL editor & workspace tabs

- Syntax-highlighted SQL editor (keyword/function/string/number/comment coloring), caret line:col readout, DataFusion
  dialect label.
- **Workspace tabs** — each tab keeps its own SQL; new-tab/close-tab wired; switching swaps the buffer (highlight
  layer + textarea synced).
- **All tabs are closeable** — including the last one. Closing the last tab reveals an **empty state**
  (no "phantom last tab" that can't be closed). Empty state invites opening a table or writing SQL.
- **Resizable panes** — the editor/results split (and side panels) are draggable to resize.
- Editor actions: **Run** (⌘↵), Format, Clear, **Save as view**, **Save query** (⌘S).
- **Save as view**: turns the current SELECT into a named catalog view (query-backed, persisted), queryable by name —
  enables composing queries on views.
- **Edit-query** on a view opens its SQL in a **new tab** named after the view.

## 10. Query execution & results grid

- Run executes (async, with running state) and populates the results grid.
- Type-aware grid: column header shows name + Arrow type; cells colored by type (togglable).
- **Nested/complex cells**: struct/list/map values render as compact type-colored one-liners
  (`{device: iOS, os: 17.2, …}`, `[checkout, promo]`, `{plan: pro}`), clipped with ellipsis; scalars render plainly.
    - Clicking a nested cell opens a **JSON popover** with the full pretty-printed value + the column's Arrow type.
      Scalar cells are non-interactive.

## 11. Results: pagination & search

- **Server-style pagination** replaces any fixed row cap: page-size **dropdown** (`100 / page`, opens upward),
  first/prev/next/last nav, `page N of M`.
- **Find in results** field at the top-left of the results, enlarged, with live match count (`13 of 100 on page`) and
  clear button. Scopes to the currently loaded page.
- Results strip stats: total rows · exec ms · files scanned · cols · bytes.
- (Future) **Filter in SQL**: promote the find term into a `WHERE` clause to filter the full dataset.

## 12. Query history

- **History** button (top-right of the tab bar) opens a right slide-over.
- Lists past runs: status dot, timing · rows (or "failed to run"), relative time, 2-line SQL preview.
- Running a query prepends a "just now" entry; **Clear** empties it; clicking an entry loads that SQL into the current
  tab.

## 13. Export model

- **Export** button (results strip) opens the export modal.
- **Format** cards: CSV, JSON, Parquet, Arrow IPC, Clipboard — selecting swaps in format-specific options and
  regenerates the live preview.
- Per-format options:
    - **CSV**: delimiter, null representation, header toggle, quote toggle.
    - **JSON**: array / JSONL / columnar + pretty-print.
    - **Parquet**: compression (zstd / snappy / gzip / none) + row-group size.
    - **Arrow**: file / stream + LZ4.
    - **Clipboard / Copy as**: Markdown / CSV / TSV / **JSON** (with a pretty JSON array preview).
- **Rows to export**: All (`48,213`) or This page.
- **Destination**: editable filename with a live extension suffix + folder path (hidden for Clipboard).
- **Live preview**: renders real result rows in the chosen format with an estimated file size that reacts to
  compression.
- Confirm reports back in the event log + status bar (`Exported cross_file_join.csv · 12 rows`).
- (Future) **Stream full result to disk** with progress; **saved export presets**.

## 14. Command palette (⌘K)

- Opened via ⌘K / Ctrl+K or the header search field; Esc closes.
- Live fuzzy filter across groups: **Actions, Tables, Views, Saved queries, Columns** (columns/queries appear as you
  type).
- Keyboard nav: ↑↓ move, Enter selects, hover syncs the active row; active row shows accent icon + ↵.
- Type-colored column icons; per-item metadata and shortcut hints (Run = ⌘↵).
- Selection dispatches the real action:
    - Table → open (`SELECT * FROM t LIMIT 100`).
    - View → open its query in a new tab.
    - Saved query → open it in a tab.
    - Column → select + inspect.
    - Commands → Run · New query tab · Save query · Save as view · Export results… · New table… · Query history · Toggle
      sidebar · Open project…

## 15. Settings

Modal with a left category nav (Appearance & Behavior):

- **Appearance**
    - **Sync with OS** toggle on its **own row above Theme**. When on, it **overrides and disables**
      (greys out / makes unselectable) the manual theme grid.
    - **Theme** grid — populated from **all discovered themes**, not a fixed pair. Themes are **JSON files** (a serde
      `Theme`: `id`, `name`, `author`, optional `extends` base, `mode`
      hint, and the full `--c-*` + syntax/Arrow-type/plan/cell token table). Discovered from three sources, each card
      carrying a **source badge**: **Built-in** (bundled — ship *Midnight* + *Daylight* to start), **User**
      (`<app-config>/themes/*.json`), **Plugin** (plugin-contributed theme dirs). The loader validates hex tokens and
      resolves `extends` by merging onto the base; applying a theme injects its tokens as CSS variables on the root.
      Buttons: **Open themes folder**, **Reload themes**. So "add a theme" = drop a JSON file; users/plugins extend
      freely.
    - Sidebar/panels follow VS-Code-light convention in Daylight: **white** surfaces, separation from thin 1px borders
      (not grey fill), accent = blue tint. Zebra rows use a subtle grey band that reads on white.
- **Data display**
    - **Density**: Comfortable / Compact.
    - **Alternating row colours (zebra)** toggle — **defaults to on**; off removes results-grid striping.
- **Keymap** (read-only)
    - Lists all global shortcuts with styled key caps; note that ⌘ shortcuts also respond to Ctrl.

## 16. Keybindings (global, in `handle_key`)

Every ⌘ shortcut also responds to Ctrl (`meta || ctrl`), so they work cross-platform:

- **⌘K** — toggle the command palette
- **⌘T** — new query tab
- **⇧⌘T** — reopen the last closed tab
- **⌘W** — close the current tab
- **⌘S** — save the active query to the project
- **⌘↵** — run the current query
- **⌘`** — cycle focus between open project windows
- **Esc** — dismiss overlays / menus

---

## Suggested build order

1. Two-window model (launcher ↔ main) + project model & `.ps/` file persistence — the spine.
2. Table configuration (ListingTable sources) + async validate + partitioning.
3. Catalog (tables / views / saved queries) with kind/partition badges + confirm-on-remove.
4. Editor + workspace tabs (all closeable, empty state) + resizable panes + run + results grid.
5. Pagination + find-in-results.
6. Column inspector (metadata + nested field trees only — no fabricated stats).
7. Query history + event log panel.
8. Export model.
9. Command palette.
10. Settings (Appearance / Data display / Keymap) + keybindings + theming.
