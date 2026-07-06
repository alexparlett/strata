# Strata

A local, **Athena-style parquet query workspace** — a polished dark IDE for
querying parquet with SQL, no Glue catalog or schema setup. Built with
[Dioxus](https://dioxuslabs.com/) (desktop) and
[Apache DataFusion](https://datafusion.apache.org/).

Implements the `Strata` design (Claude Design handoff). See
[`docs/DESIGN_SPEC.md`](docs/DESIGN_SPEC.md) — **§14** is the current source of
truth (stack, design tokens, UI surfaces, DDL policy).

---

## What it does

- **Catalog** of external **tables** (parquet/csv/json over files, directories, or
  globs — one table over any mix) and **views** (saved SQL), in a filterable
  sidebar with type-coloured columns and Hive `PART` badges.
- **Query workspace** — tabs, a syntax-highlighted SQL editor (DataFusion
  dialect), Run (⌘/Ctrl+Enter), Format, Clear, Save-as-view.
- **Results grid** — type-coloured cells, nested cells open a JSON popover,
  find-in-results, pagination.
- **Column inspector** — type, stats over the current result, nested-field tree,
  completeness.
- **Table Config** — multi-path sources with browse, format, and Hive-partition
  detection (typed, with the string-cast warning).
- **Export** (via `COPY … TO`), **command palette** (⌘K), **query history**.
- **Managed catalog DDL policy** — the editor runs `SELECT`/`EXPLAIN`/`SHOW`/
  `DESCRIBE` and captures `CREATE`/`DROP VIEW`; it blocks `CREATE EXTERNAL TABLE`
  / CTAS / `INSERT` (use Table Config) and hard-blocks `CREATE DATABASE`/`SCHEMA`.

---

## Prerequisites

Rust toolchain via [rustup](https://rustup.rs). Dioxus desktop uses a system
webview:

- **macOS** — nothing extra (WKWebView is built in).
- **Linux** — `webkit2gtk` + `libxdo`, e.g. on Debian/Ubuntu:
  `sudo apt install libwebkit2gtk-4.1-dev libxdo-dev libayatana-appindicator3-dev`.
- **Windows** — WebView2 runtime (ships with modern Windows).

Optional: the Dioxus CLI (`cargo install dioxus-cli`) gives `dx serve` with
hot-reload.

---

## Build & run

```bash
# Plain cargo (first build pulls Dioxus + DataFusion — give it a few minutes)
cargo run --release

# …or with the Dioxus CLI + hot reload
dx serve --platform desktop
```

Click **✨ / Generate sample data** (command palette, ⌘K) to write two demo
parquet files and register them, then Run the pre-filled cross-file join.

---

## Architecture

```
assets/main.css             design system (tokens + component styles), injected at the app root
src/main.rs                 Dioxus launch + window config
src/app.rs                  root component; owns Signal<AppState>; engine bridge; controller actions
src/state.rs                AppState (+ a seed matching the prototype)
src/engine.rs               DataFusion on a background thread; channels; multi-path register;
                            view create/drop; typed + nested-aware results; sample generation
src/ddl.rs                  DDL policy classifier (DataFusion's DFParser / sqlparser AST)
src/util.rs                 Kind (type→colour map), name/byte helpers
src/ui/{header,sidebar,workspace,inspector,statusbar,modals,icons}.rs
```

The DataFusion engine runs on its own thread with a Tokio runtime; the UI talks
to it over `tokio::mpsc` channels and a Dioxus coroutine drains engine events
into the single `Signal<AppState>`. Syntax highlighting comes from
`dioxus-code` / `dioxus-code-editor` (tree-sitter): `CodeEditor` for the SQL
editor, `Code` for the nested-cell JSON view.

---

## Status

This is the first Dioxus cut of the redesign, built to match the prototype. It
was **assembled against the design, not compiled in the authoring environment**
(the sandbox blocks crates.io), so expect to shake out compile fixups on the
first `cargo build` — particularly around exact Dioxus 0.7 / `dioxus-code` APIs.
Report errors and they're quick to resolve.

## License

MIT.
