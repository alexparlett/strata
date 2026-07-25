# Strata — project guide

Strata is a local, **Athena-style parquet query workspace**: a polished dark IDE for querying
parquet/csv/json with SQL over **Apache DataFusion**, with no Glue catalog or schema setup. Catalog
of external tables + saved views, a tabbed SQL editor, a virtualized results grid, a column
inspector, table config, export via `COPY … TO`, a command palette, and query history. Product
name **Strata** (uneven sedimentary layers = data strata).

The current effort is a **UI migration from Dioxus (wry/webview) to Freya 0.4 (Skia/native)**. Read
this whole file before starting work — most of it is context that's otherwise expensive to
rediscover.

---

## Build & run

```bash
cargo run              # root default-member = the Freya app (strata-freya)
cargo run --release    # first build pulls DataFusion + compiles Skia; give it time
```

(`crates/strata-dioxus` — the old Dioxus app — **no longer builds**; reference code only. See the
engine-model note below.)

After **any theme change**, regenerate + verify the schema:
`UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync` (the committed
`themes/theme.schema.json` must match `theme.rs`'s `REGISTRY`).

> **Environment note:** some agent sandboxes can't build this (no crates.io access, no Skia
> toolchain). If you're in one, you can't run `cargo build`/`test` — verify changes against the fork
> source instead (see below) and hand off to a Mac build. Claude Code running locally on the Mac has
> no such limit: build and test normally, and treat a clean build + `schema_in_sync` as the check.

## Ways of working

The engineering bar and every settled Strata/Freya convention are enshrined in @AGENTS.md — it is
imported into every session alongside this file; hold all work to it, and update it in the same
change whenever a review settles (or overturns) a convention. Headlines: generic capability over
tactical stubs; real end-states, no placeholder scaffolding; verify APIs from the fork source
before agreeing; framework-native idiom (never bridge Dioxus-era patterns); model impossible states
out of existence and fail loud; follow [`marc2332/valin`](https://github.com/marc2332/valin) (the
Freya author's own IDE) for module layout, per-window data scoping, and stateful tabs.

---

## Workspace layout

A virtual workspace (no root package). `cargo run` at the root targets the **Freya** app.

Members:

- **`strata-freya`** — the Freya (Skia/native) frontend. **The port target** and the default build.
- **`strata-core`** — engine logic: the DataFusion boundary (query/plan/profile/serialize), config,
  theme, SQL language service. The only place DataFusion is touched.
- **`strata-model`** — leaf data vocabulary, serde-only (schema, results, catalog, form, log,
  query_error). No logic.
- **`strata-code-editor`** — vendored Skia code editor (Rope buffer + tree-sitter highlighting) used
  by the Freya SQL editor.
- **`strata-forms` / `strata-forms-macro`** — headless forms layer + `#[derive(Form)]`.

Excluded from the workspace (deliberately):

- **`crates/strata-dioxus`** — the old Dioxus app (the mature, webview implementation we're
  porting *from*), kept as **reference code only**: it references the engine protocol that was
  deleted from `strata-core` with P2-01, so it **no longer builds** — read it for feature
  behaviour, don't try to fix its build. (It was always its own workspace because its editor
  stack and ours both set `links = "tree-sitter"`.)
- **`crates/freya`** — our **Freya fork checkout** (see below).

## The Freya fork

`crates/freya` is a **git submodule** pointing at our fork, `github.com:alexparlett/freya`.

- The build resolves Freya from the **local checkout path** (`[workspace.dependencies]` in the root
  `Cargo.toml`), *not* from git. So edits to `crates/freya/**` are picked up on Alex's next
  `cargo build` — no push, no `cargo update` needed for local builds.
- **But** the committed submodule gitlink must be pushed to the fork remote, or a fresh clone / CI
  can't init the submodule. After changing the fork, push it.
- For reproducible CI/release builds the path deps are meant to be swapped back to
  `{ git = "…", rev = "…" }` (pin a rev).

---

## Freya: skill, reference, examples

When writing Freya code, lean on these in roughly this order:

1. **The `freya` skill** (`freya:freya`) — best-practices for components, hooks, elements, events,
   state (local / Radio / context / Readable-Writable), theming (`define_theme!` / `get_theme!`),
   async, keying, a11y. Invoke it when writing or refactoring Freya UI. It's the fast reference for
   *how* to structure things.
2. **The fork source** — `crates/freya/`. The ground truth for exact APIs. Key spots:
   `crates/freya/crates/freya-core/src/events/` (event data + names),
   `crates/freya/crates/freya-components/` (built-in `Button`, `Input`, `ScrollView`,
   `VirtualScrollView`, etc.), `crates/freya/src/_docs/` (in-source docs). `crates/freya/AGENTS.md`
   (a.k.a. its `CLAUDE.md`) documents Freya's own dev workflow.
3. **`crates/freya/examples/`** — 150+ runnable examples. `component_*.rs` (button, input, select,
   context_menu, table, table_virtual, resizable_container, tooltip, popup, drag_drop, sidebar…),
   `animation_*.rs`, plus platform samples. The canonical "how do I wire X" reference.

### Conventions

The Freya conventions that bite (component shape, builder pattern, event/handler traps, reactivity,
logical units) and the this-codebase conventions (standard components, typography, naming,
user-facing text register, state placement) are enshrined in [AGENTS.md](AGENTS.md) §3–§4 — that
file is the single copy; don't restate them here.

---

## strata-freya module map

```
src/main.rs                      Freya launch + startup routing (reopen every project that had a
                                 window at the last quit, else the launcher; argv[1] wins);
                                 discovers ThemesCtx + creates the two app-globals — the reactive
                                 AppConfig store and the live window registry. Each window's theme
                                 is pure derived state (`use_strata_theme`)
src/platform/windows.rs          the **window model**: the live registry (WindowId → launcher /
                                 project folder), `open_project` / `open_launcher`
                                 (focus-if-open), `close_this_window` (the launcher takes over
                                 when it was the last), and quit (closes every window + keeps the
                                 persisted open-set, so a restart reopens them)
src/menu.rs                      the macOS menubar: App · **File** (Open… · Open Recent ·
                                 Close Project) · Edit. Not static — `app_menu` hands back
                                 `MenuHandles`, and the *focused* window keeps the File menu
                                 pointed at itself (`use_file_menu`): its recents, and Close
                                 Project only when it has a project to close
src/state/mod.rs                 `AppCtx` — the four app-globals `main` creates once (themes ·
                                 config · window registry · menubar handles), handed to every
                                 window root as one value rather than four parameters
src/state/config.rs              THE app-global store: one `RadioStation<AppConfig, ConfigChan>`
                                 (settings · recents · open-project set) created once in main and
                                 shared into every window (`use_share_config`). Channels keep a
                                 project open from waking theme readers; `write_config` is the only
                                 write path (mutate + notify + persist — nothing re-reads the file).
                                 `use_open_project` ties a window's project to recents + the
                                 open-set for its lifetime
src/theme.rs                     Freya theme application: `theme_registry!` / `strata_components!`
                                 macros, Pref→Preference coercion, ThemesCtx (the shared
                                 ThemeRegistry handle, discovered once in main, provided at every
                                 window root), schema-sync test. The theme data model + loader +
                                 ThemeRegistry (built-ins + user themes dir) + schema gen live in
                                 `strata-core::theme`; the theme files themselves in root `themes/`
src/components/                  shared component library
  divider, dot, icon, run_button, segmented_toggle, toggle_button, typography
  badge.rs                       tinted label pill (PART · HOTSPOT · ANALYZE · dtype · keycap).
                                 NOT Freya's `Chip` — that's a selectable, focusable control
  sidebar_row.rs                 the left pane's row shell: a preset over Freya's `SideBarItem`
                                 (+ `Activable` for selection), so hover/selected dress and a11y
                                 are shared by the catalog and, later, connections (W7)
  type_palette.rs                the seven per-`Kind` hues (`"type_palette"` theme group) +
                                 `kind_color`. Named for Kind, not Arrow; the EXPLAIN plan
                                 borrows the same ramp for operator kinds
src/apps/launcher/               the launcher / welcome window (P4-02, `Launcher.dc.html`)
  mod.rs                         root + window config + the `launcher` component theme
  model.rs                       ProjectList: the filter + PINNED/RECENT split (unit-tested)
  views/                         title_bar · rail (SidebarRow) · projects · row · open (rfd pick)
src/apps/project/                the project window (Valin-shaped)
  project.rs                     root component; spawns the engine, provides EngineCtx
  contexts/engine_ctx.rs         EngineCtx = Arc<Engine>, provided via use_provide_context
  state/                         per-window state (Radio): channel, hooks, session
                                 session.rs = SessionState + stateful QueryTab (each tab owns its
                                 CodeEditorData, keyed on Chan::{Tabs, Tab(id)}); Layout too
                                 project.rs = ProjectState — **the catalog**: persisted defs +
                                 `Reg<T>` (Loading/Ready/Failed), per-section ProjChan channels
                                 catalog.rs = CatalogSelection, the inspected column (context)
  model/                         window-local view models
  views/
    header/
      mod.rs                     the header bar — and the window's macOS title bar: brand ·
                                 switcher · ⌘K/⌘, cluster, drag + double-press-to-fill
                                 (`window_drag`), traffic-light gutter
      project_menu.rs            the project switcher: trigger + Open… / open set / recents
                                 dropdown (data real; opening a project is P4-13's seam)
    sidebar/
      mod.rs                     sidebar shell — pane-specific header (the catalog's filter +
                                 refresh row) over the active pane
      catalog/                   P3-02: mod (pane + sections), section, entry (entry/column/
                                 saved-query rows), columns (flatten + tests), interaction (tests)
    workbench/
      mod.rs, empty.rs           workbench shell + no-query empty state
      editor/                    SQL editor: tab, toolbar
      tab_bar/                   bar, tab, controls (new/navigate/overflow), drag, menu (context menu)
      results/
        mod.rs                   results panel — freya-query-driven states (empty / running /
                                 grid / explain / error) off the workbench's `request` slot
        datagrid/                mod, header, cell, model  (sticky typed header, virtualized cells,
                                 per-column resize + double-click autofit)
        selection.rs             cell/row/column selection model + SelCtl controller
        find.rs                  find-in-results (P2-09): FindState + the page-local filter
        toolbar.rs, status_bar.rs, running.rs, explain_plan.rs, empty.rs, error.rs
```

**Note on the two frontends:** the **Freya** app is a clean-slate, Valin-shaped rewrite with its
own architecture (Radio `SessionState`, stateful `QueryTab`s, `EngineCtx` in context) — never carry
a Dioxus-app pattern (`GlobalStore`, `dispatch`/`action`, the muda menu, the old keymap/hotkeys
registry) across. When working in `strata-freya`, follow **`docs/FREYA_STATE_ARCHITECTURE.md`** and
[AGENTS.md](AGENTS.md), and treat `crates/strata-dioxus` as behavioural reference only.

---

## Docs index (`docs/`)

Migration:

- **`FREYA_PORT_PLAN.md`** — why we're migrating and the phased plan (webview-tax motivation, spike
  results, Valin as reference).
- **`FREYA_STATE_ARCHITECTURE.md`** — the **definitive** per-window state design for the Freya app;
  every API verified against Freya 0.4 source. **Supersedes `FREYA_PORT_PLAN.md` §4.**
- **`freya-state-dataflow.mermaid`** — data-flow diagram for the above.
- **`FREYA_THEME_SPEC.md`** — the native JSON theme format (sheet + components + tokens + fonts).

Product / design:

- **`DESIGN_SPEC.md`** — **§14 is the current source of truth** (stack, design tokens, UI surfaces,
  DDL policy).
- **`FEATURES.md`** — full feature spec (every surface + its DataFusion/engine hook).
- **`DEV_TASKS.md`** — the backlog, split into UI-surface audits (design-vs-code drift: align vs
  build) and functional workstreams.

The **design handoff** lives in **`.claude/design-handoff/`** (gitignored — local only, not
committed). It's a Claude Design (claude.ai/design) bundle: the `.dc.html` HTML/CSS prototypes that
are the **pixel-perfect source of truth** for every surface (`Strata`, `Settings`, `Launcher`,
`Windows`, `DrawerProblems`, `StatusBar`, …), plus `strata-windows.js`, reference `screenshots/`, and
a per-bundle README. The DEV_TASKS Part-1 audit and `DESIGN_SPEC.md` are derived from these canvases.
Read the `.dc.html` source directly (dimensions/colours/layout are spelled out there); don't render
or screenshot them unless asked.

Feature specs: `COMPLETION_SPEC.md` (the as-built P2-04 completion design — supersedes
`SQL_LANGUAGE_SPEC.md` §4), `CONNECTIONS_SPEC.md`, `EXPLAIN_PLAN_SPEC.md`,
`EXPORT_OPTIONS.md`, `IMPORT_OPTIONS.md`, `SQL_LANGUAGE_SPEC.md`, `EDITOR_LANG_SPIKE.md`,
`F7-shared-state.md`.

---

## Task backlog (`.claude/tasks/`)

The Freya-rewrite backlog lives in **`.claude/tasks/`** (committed): a top `README.md` index, then
**one folder per phase / workstream**, each with its own `README.md` and **one file per task**. Each
task file is self-contained — current state, what to build, acceptance, Freya components, and the
`DEV_TASKS.md` ID it traces to — so a session can pick up a single task (e.g. in a worktree) without
loading the rest. Every migration phase (2–6) and both workstreams (Connections, Chart) are broken out
into task files; the near-term ones (phases 2–3) carry the most detail. Read the top `README.md` first
(status legend, phase order, known bugs).

The near-term critical path is done: P2-01 (engine facade + snapshots, `docs/SNAPSHOT_SPEC.md`
agreed), P2-02 (results driven by `use_query`) and P2-03 (grid renders the real `QueryPage`;
fixture deleted; snapshot page reads via `FetchSnapshotPage`, paged from the status bar) are ✅ —
sort/filter/export now rest on the snapshot read model. Results are **freya-query** off the tab's
SQL (no runs-by-id store, no query *results* on the session — state-arch §2): each `QueryTab`
owns its Run trigger (`QueryTab::request: Option<QuerySpec>`, on the dedicated
`Chan::Request(id)` channel, so one tab's press/cancel never touches another tab's results and
keystrokes never wake the results pane); the `running` mirror is threaded as **struct-field
props** — props for known shallow consumers, context only for DI handles (`EngineCtx`) and deep
trees (`Selection`).

Phase 3 has started: P3-01 (layout shell) and **P3-02 (catalog sidebar)** are ✅. Note the
correction P3-02 carried: **the catalog is the `ProjectState` store, not a query.** Earlier drafts
of `FREYA_STATE_ARCHITECTURE.md` (and the P3-03 / P3-06 task files, now fixed) described a
`FetchCatalog` freya-query capability — it was never built and must not be. Introspecting
DataFusion would surface the `__snap_*` result snapshots and hide defs whose registration failed,
which are precisely the rows the catalog exists to show. Catalog mutations call the engine and
then `ProjectState`'s own methods on the matching `ProjChan`; nothing refetches.

---

## Engine model

The engine (`strata_core::engine::Engine`) is a **direct-call async facade**: it owns a private
multi-thread Tokio runtime (DataFusion's operators need a Tokio context; query CPU never touches
the render thread), spawns each call onto it, and the caller awaits the `JoinHandle` — which is
executor-agnostic, so Freya's non-Tokio UI executor awaits engine methods like any async fn. No
UI-side runtime, no channels, no request ids. freya-query capabilities call the facade directly
(`engine.query(…)`, `engine.fetch_page(…)`); snapshot lifecycle (supersede / cancel / retire) is
the facade's own bookkeeping — see **`docs/SNAPSHOT_SPEC.md`**. In Freya the handle is `EngineCtx`
(an `Arc<Engine>` + Deref) held in context — not stored in any god-object `AppState`. Managed DDL
policy: the editor runs `SELECT`/`EXPLAIN`/`SHOW`/`DESCRIBE` **only**. Views are Save's artifact,
never typed DDL — ⌘S / Save-as-view wraps the buffer's *plain query* in `CREATE OR REPLACE VIEW`
itself (`Engine::create_view`), so typed `CREATE`/`DROP VIEW` is blocked (validation points at
Save / the catalog), like `CREATE EXTERNAL TABLE` / CTAS / `INSERT` (use Table Config) and the
hard-blocked `CREATE DATABASE`/`SCHEMA`.

> The Dioxus-era `Command`/`Event` channel protocol + worker loop was **deleted from
> `strata-core`** with P2-01. `crates/strata-dioxus` still references it and therefore **no longer
> builds** — it is kept as *reference code only* for porting features to Freya. Don't try to fix
> its build.
