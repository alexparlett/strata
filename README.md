# Strata

A local, **Athena-style parquet query workspace** — a polished native IDE for querying parquet, CSV and JSON with SQL,
with no Glue catalog or schema setup. Built with
[Freya](https://freyaui.dev/) 0.4 (Skia, native — no webview) and
[Apache DataFusion](https://datafusion.apache.org/).

Work is organised into **projects**: a folder with a `.strata/` directory holding its catalog, session and query
history. Open one per window; the app reopens what you had at last quit.

---

## What it does

- **Catalog** of external **tables** (parquet/csv/json over files, directories, or globs — one table over any mix) and
  **views** (saved SQL), in a filterable sidebar with type-coloured columns and Hive `PART` badges.
- **Query workspace** — tabs, a syntax-highlighted SQL editor (DataFusion dialect) with completion and live
  diagnostics, Run (⌘/Ctrl+Enter), Explain and Explain analyze, Format SQL, Save and Save-as-view.
- **Results grid** — virtualized, type-coloured cells with per-column resize and autofit, sort, find-in-results,
  pagination, and cell/row/column selection with copy as TSV / CSV / JSON / Markdown. Double-click a nested cell for
  its JSON, or the row gutter for the whole record. `EXPLAIN` renders as a plan tree.
- **Column inspector** — type, nested-field tree, and **only facts that were actually read**: parquet footer
  statistics and the table row count, plus an opt-in full **scan** (behind a cost confirm) for the numbers the footer
  can't answer.
- **Bottom drawer** — **Problems** (every open tab's SQL diagnostics, grouped by tab), **Events** (what the session
  did), **History** (past runs; press to load, double-press to load and run).
- **Table Config** — multi-path sources with browse, format options, and Hive-partition detection (typed, with the
  string-cast warning).
- **Export window** (via `COPY … TO`) — parquet/csv/json/arrow with per-format options, Hive partitioning, and a
  preview built from the run's real schema and rows.
- **Settings window** — Theme (with sync-with-OS), System, Data display, and **Engine ▸ Properties**, which edits
  DataFusion's own configuration keys directly.
- **Themes** — `Midnight` (dark) and `Daylight` (light) ship built in; user themes are JSON files in the themes
  directory. See [`docs/FREYA_THEME_SPEC.md`](docs/FREYA_THEME_SPEC.md).
- **Managed catalog DDL policy** — the editor runs `SELECT`/`EXPLAIN`/`SHOW`/`DESCRIBE` **only**. Everything else is
  blocked with a message naming the surface that owns it: `CREATE TABLE` / `CREATE EXTERNAL TABLE` / `INSERT` → Table
  Config, `CREATE VIEW` → Save as view, `DROP` → the catalog, `COPY TO` → Export, `SET`/`RESET` → Settings.
  `CREATE DATABASE`/`SCHEMA` are refused outright.

---

## Prerequisites

- A Rust toolchain via [rustup](https://rustup.rs).
- **The `crates/freya` submodule.** Freya is our fork, and the build resolves it from the **local checkout path**
  (root `Cargo.toml`, `[workspace.dependencies]`) — without it nothing compiles:

  ```bash
  git submodule update --init --checkout
  ```

  `git submodule status crates/freya` should print no `+` or `-` prefix. (`git worktree add` does **not** update
  submodules — run the command above in each new worktree.)

There is no webview and no system GUI dependency to install; Freya renders with Skia, which compiles from source on
the first build. macOS is the platform Strata ships and is tested on — CI runs on macOS, and the menubar,
traffic-light gutter and child-window pinning are macOS-specific.

---

## Build & run

```bash
cargo run
```

The root's `default-members` is the Freya app, so a bare `cargo run` at the repo root is the app. `cargo run --release`
is the one to use for real work. Either way the first build pulls DataFusion and compiles Skia — give it time.

To try it against real data, open the repo's **`sample/`** folder as a project: it registers parquet, CSV and JSON
tables, a Hive-partitioned `events/` directory, and two saved views.

### Tests

```bash
cargo test --workspace --locked
```

`--workspace` rather than a bare `cargo test`, which `default-members` would narrow to `strata-freya` alone. After any
theme change, regenerate and verify the committed schema:

```bash
UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync
```

---

## Getting a build

For a distributable `.app` and DMG:

```bash
./scripts/bundle-macos.sh
```

Universal binary and DMG land in `target/dist/`; `--arch arm64` is roughly half the build time, `--no-dmg` stops at
the `.app`. The same script runs on a GitHub runner via **Actions → Release**, which can attach the DMG to a run or
publish a release page.

**[`docs/RELEASING.md`](docs/RELEASING.md)** is the full account — the workflow's inputs, cutting a version, what
signing rung the build took, and the `xattr -dr com.apple.quarantine` step testers need while builds are unsigned.

---

## Architecture

A virtual Cargo workspace (no root package):

```
crates/strata-freya         the Freya (Skia/native) frontend — the app; one module per OS window
                            under apps/ (project, launcher, settings, export, configure)
crates/strata-core          engine logic: the DataFusion boundary (query/plan/profile/serialize),
                            config, theme, SQL language service. The only place DataFusion is touched
crates/strata-model         leaf data vocabulary, serde only (schema, results, catalog, session…)
crates/strata-code-editor   vendored Skia code editor (Rope buffer + tree-sitter highlighting)
crates/freya                our Freya fork (git submodule), resolved by local path — excluded
                            from the workspace, but the build depends on this checkout
```

The engine (`strata_core::engine::Engine`) is a **direct-call async facade**: it owns a private multi-thread Tokio
runtime, spawns each call onto it, and the caller awaits the `JoinHandle` — so query CPU never touches the render
thread and Freya's non-Tokio UI executor awaits engine methods like any async fn. There are no channels, no request
ids and no worker loop; the Dioxus-era `Command`/`Event` protocol was deleted in the port.

The full per-module map is in **[`CLAUDE.md`](CLAUDE.md)**, and the state design in
[`docs/FREYA_STATE_ARCHITECTURE.md`](docs/FREYA_STATE_ARCHITECTURE.md).

---

## Docs

- [`CLAUDE.md`](CLAUDE.md) — build, workspace layout, module map, docs index.
- [`AGENTS.md`](AGENTS.md) — the engineering bar and every settled convention.
- [`docs/FREYA_PORT_PLAN.md`](docs/FREYA_PORT_PLAN.md) — why the migration, and the phased plan.
- [`docs/FREYA_STATE_ARCHITECTURE.md`](docs/FREYA_STATE_ARCHITECTURE.md) — the per-window state design.
- [`docs/SNAPSHOT_SPEC.md`](docs/SNAPSHOT_SPEC.md) — the result-snapshot read model.
- [`docs/DEV_TASKS.md`](docs/DEV_TASKS.md) — the backlog. Task files live in `.claude/tasks/`.

---

## Status

Under active development. Strata began as a Dioxus (webview) app and was rewritten on Freya for native rendering; the
Dioxus frontend has since been removed, so the Freya app is the only one. The catalog, editor, results grid,
inspector, drawer, export and settings surfaces are in place. Remaining work — the connections and chart workstreams
among it — is tracked in [`docs/DEV_TASKS.md`](docs/DEV_TASKS.md) and `.claude/tasks/`.

## License

MIT.
