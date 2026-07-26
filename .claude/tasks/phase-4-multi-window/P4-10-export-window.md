# P4-10 · Export window (rebuild to canvas)

**Phase:** 4 · **Status:** 🟡 `[core ✓]` — engine facade + option surface done; the window remains ·
**DEV_TASKS:** D6 / U13 · **Depends on:** P4-01, P2-01

## Goal
The export UI, rebuilt to the v19 canvas — keep the export backend (`COPY … TO`).

> **Report the outcome, and route a failure through P4-15.** An export is a write the user asked
> for, so it belongs in the event log (P3-13) on both arms — the canvas's own
> `Exported <n> rows → <path>` is the success line. Take the failure path through **P4-15**'s write
> funnel rather than inventing a second one here; if P4-15 has not landed yet, call P3-13's
> `persisted`-style helper and note it in P4-15's build list instead of adding a bare
> `tracing::error!`.

## Reading the canvas (settled — don't re-derive this)
`Export.dc.html` is the **window master for the markup**; the **live option data is
`Strata.dc.html`'s `exportGroups()`**, which Strata spreads in via
`<dc-import name="Export" dc-props="{{ exportVM }}">`. The master's own `FMT_META` is a
**standalone mock** (its header comment says so: "when Strata imports it … Strata's live
values/handlers win"), and it is *stale* — it offers an Arrow compression select, which does
not exist, and labels Parquet's row group in MB when `max_row_group_size` is a row count.
**Read `exportGroups()` for the option set; read `Export.dc.html` for the layout.**

Two more canvas facts that are easy to get wrong:
- **There is no ADVANCED section.** CHANGELOG "Export: fold format 'advanced' options into the
  main option list" — `exportGroups()` returns `core: [...core, ...adv]`, `adv: []`,
  `hasAdv: false`. The dangling `</sc-if>` in the master markup is that disclosure's leftover.
- **Clipboard is not a format** (dropped 2026-07-12; in-grid copy covers it). Formats are
  CSV / JSON / Parquet / Arrow IPC. Dead `clipShape` branches remain in the canvas JS.
- **The reference `screenshots/` are unreliable** — they predate both changes (they still show
  Clipboard and the side-by-side preview). Read the `.dc.html` source, use `CHANGELOG.md` for
  history.

## The option surface (authoritative — CHANGELOG 2026-07-07 §13 correction)
Every one of these maps to a real DataFusion 54 `COPY … OPTIONS` key, verified end-to-end by
`crates/strata-core/tests/engine_export.rs`.

| Format | Options |
|---|---|
| all | ROWS TO EXPORT (All · n / This page) |
| CSV | HEADER ROW · DELIMITER (text, resolves `\t`) · NULL VALUES AS (seg + custom) · QUOTE CHARACTER · ESCAPE CHARACTER · DOUBLE-QUOTE · COMPRESSION |
| JSON | COMPRESSION only (NDJSON *is* the format) |
| Parquet | COMPRESSION · COMPRESSION LEVEL (zstd/gzip/brotli only, range in the label) · STATISTICS · MAX ROW GROUP SIZE (**rows**: 128K/512K/1M/2M) · WRITER VERSION · DICTIONARY ENCODING |
| Arrow IPC | **none** — DataFusion exposes no Arrow write options |

Hive partitioning applies to every format: enable toggle → dual-pane transfer (AVAILABLE with a
filter header once >8 unselected, SELECTED with order badges + drag-reorder + remove ×) → keep-
columns toggle + warning. Available columns are **numeric or string only** (no ts/bool/nested).

## Done — the core (`[core ✓]`)
- **`strata-core::engine::export`** rewritten: `ExportSpec { path, scope, sort, format, partition }`
  where per-format options live *inside* the `Format` variant, so "a CSV delimiter on a Parquet
  export" is unrepresentable and `Format::Arrow` carries nothing.
- **`Engine::export(snapshot, spec)`** on the facade. Takes a `SnapshotId`, not a `WsId` — an
  export belongs to a *result*, not a tab. No dispatch nonce, no supersede: two exports are two
  files. It **counts as work in flight** (`Lifecycle::exports`, a count not a map) because
  closing mid-write leaves a truncated file under the user's chosen name.
- `Scope::Page` windows the rows **after** the sort, so "this page" is the page on screen.
- Single-char CSV options are sent as **byte values**, never characters: DataFusion parses those
  `u8` fields with `parse::<u8>()` first, so the character `9` would arrive as a tab.
- **Snapshot pins** (`Engine::pin_snapshot` → RAII `SnapshotPin`, SNAPSHOT_SPEC §4). A snapshot
  is retired the moment its workspace dispatches another run, so a re-run while the export
  window is open would deregister the table mid-`COPY` (truncated file) or make a later Export
  report no results when there are plainly some on screen. A pin **defers** the retire to the
  last release. The export window holds one for its whole life, so it always writes the result
  it was opened on; `Engine::export` brackets its own call too.
- 12 unit tests + 17 round-trip tests (`tests/engine_export.rs`) that write real files and read
  them back — a wrong option key fails the `COPY`, so green *is* the proof the surface is real.
  Five of them cover the pin: survives a re-run, survives the tab closing, counted holds,
  unpinned snapshots still retire at dispatch, and a lone release retires nothing.

## Done — the window
`apps/export/` — a real OS window (780×640, min 560×420), a child of the project window that
opened it, closing with it (`platform/export.rs`). **No single-instance rule**, unlike Settings:
a window is opened *on a result*, so focusing an existing one would show the wrong run.

- `model.rs` — `ExportTarget` (what is being exported) + `ExportDraft` (what was chosen) +
  the **data-driven groups**. Every control carries the `Edit` it performs (a `Choice` holds
  one; a text field holds `Make<String>`), so a control cannot write the wrong field and
  `apply` is exhaustive. The draft keeps **every format's** options side by side while the
  engine spec keeps only the active format's — switching to Parquet and back must not forget
  your delimiter, but the spec must not be able to name one on a Parquet export.
- `preview.rs` — the PREVIEW pane, from the run's real page-1 rows and real schema.
- `views/` — title bar · format cards · the flat option list · Hive partitioning · footer.
  Each control shape is **its own component**: the group list changes length with the format,
  so rendering the stateful ones inline would vary the hook count per render.
- Wired to the results toolbar's Download, which is disabled until a run settles rows.
  `ExportLaunch` carries the target + engine + app handles as a **prop** (AGENTS §4 — the
  toolbar is a shallow known consumer), which is also why the datagrid interaction tests still
  mount without app context.
- Reordering partition levels is **▲▼ buttons, not drag-and-drop**: the canvas uses HTML5 drag
  events, which have no Freya equivalent here, and order is the whole meaning of that list.
- 55 tests: 19 model, 16 preview, 20 end-to-end (`tests.rs` — draft → spec → `Engine::export`
  → a real file in a temp dir → read back), plus 18 engine round-trips in core.

**The outcome is reported**, per this file's own directive: on success the project window's
event log (P3-13) gets `Exported <n> rows to <path>` and this window closes; on failure it gets
an `Error` row *and* the footer keeps the message, so the user can change one option and retry
without rebuilding the spec. A `stopped_on_purpose` settle is not reported as a fault. P4-15's
write funnel had not landed, so this calls `log_event` directly — noted in P4-15's build list,
with the open question of whether an export (which writes where the *user* chose, not into
`.strata`) belongs in that funnel at all.

## Still to decide
- The high-cardinality warning the canvas shows is deliberately absent (see below).

The destination is the native `rfd` save-file / choose-folder dialog (partitioning writes a
directory, so it asks for a folder). The canvas's hand-built file browser lives in
`Strata.dc.html`, not in the export master, and duplicating an OS dialog is not the deliverable.

## ⚠️ DataFusion 54 misfiles NULL partition values — found building this
A partitioned export over a column containing NULLs **relabels those rows**. With
`(1,'emea'), (2,NULL), (3,'amer')` partitioned by `region`, DataFusion writes only
`region=emea` and `region=amer`, and the NULL row lands **inside `region=amer`** — it comes back
out claiming a value it never had. No `__HIVE_DEFAULT_PARTITION__`, no dropped row, no error.

Nothing on our side can prevent it (the writer picks the directory), so the window **warns**
whenever partitioning is enabled with a selection: "Rows with a NULL in a partition column are
written into another value's folder, so they read back with the wrong value. Partition on a
column with no NULLs."

Pinned by `a_null_partition_value_is_misfiled_under_another_value` in
`crates/strata-core/tests/engine_export.rs`, deliberately asserting the *broken* behaviour: if
a DataFusion upgrade fixes it, that test fails and the warning (`views/partition.rs`'s
`NullWarning`) can be deleted. **Open question for Alex:** whether a warning is enough, or
whether the AVAILABLE pane should refuse nullable columns / the export should pre-check for
NULLs (a scan) and fail loud instead.

## Honesty calls (per the P3-08 "only real facts" rule)
- **The size estimate is dropped.** The canvas's `estSize()` invents compression factors
  (zstd 0.11, gzip 0.13…); the footer shows the real row count instead, and the PREVIEW header
  loses its `≈ 1.2 MB`. Confirmed with Alex.
- The **preview** renders real rows from the page-1 batch (CSV/JSON) or the real schema
  (Parquet/Arrow). The partitioned preview shows the tree *shape* from values actually present
  in the page in hand — never a fabricated distinct count.
- The high-cardinality **warning** keys off column *kind*, not the canvas's distinct-count over
  an 80-row sample, which is a derived-from-what's-on-screen number of exactly the sort the
  inspector rejected.

## Acceptance
- [x] Export writes the snapshot via `COPY … TO`, with the active sort and scope.
- [x] Per-format options render data-driven (flat, no ADVANCED section); destination via the
      file browser; the window matches the canvas.
- [ ] **Verified on screen.** Everything above is proved by tests and a clean build; nobody has
      looked at the window yet. Run it, open a result, press Download.

## Freya / references
- Design: `Export.dc.html` (markup) + `exportGroups()`/`exportVM()` in `Strata.dc.html` (data).
  Snapshot = P2-01. DEV_TASKS D6/U13.
- **`strata-forms` is not available here** — it is a *Dioxus* crate (`use dioxus::prelude::*`).
  The earlier note in this file suggesting it for the option groups was wrong; build the groups
  as plain Freya components over a `State` draft.
