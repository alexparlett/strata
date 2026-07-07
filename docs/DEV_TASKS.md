# Strata — Dev backlog

Living backlog and source of truth for outstanding work. Design reference:
`Strata.dc.html` (**v10 handoff**) + `FEATURES.md`, `CHART_SPEC.md`, and the v3
`EXPLAIN_PLAN_SPEC.md` (all in `docs/`). Product was **renamed Parquet Studio →
Strata** in the v5 redesign (section **S**). The **v6–v8** drops added the chart
view, grid selection / copy / record view, engine settings, tab drag-reorder,
launcher pinning, and a results / workspace status-bar rework — tracked in section
**R** + **S17–S22** + **B11a/b**, with per-tab result state in **A5** and view/
saved-query dirty tracking in **A6**. The **v9** drop unified Settings/Export/
Configure into floating windows (shipped via the A3/A4 `Window`); **v10** then
pulled **Connections** out of Settings into a **project-scoped sidebar pane** and
added remote (S3/GCS/Azure/HTTP) sources — tracked as **S21** — and made the
launcher open the one shared Settings window (**B11b**). Settings is now 100% global.

**Status:** ✅ Done · 🟡 Thin (wired but shallow) · ⬜ Todo · 🚧 Blocked (design pending) · ⛔ Dropped / superseded

---

## A. Architecture foundation

| ID | Task | Status | Notes |
| --- | --- | --- | --- |
| A1 | Project-scoped state | ✅ Done | `Project` domain model in `project.rs`; `AppState` holds it + engine handle + global prefs. Locked: theme/panel sizes are **global**; tables persist as **specs, re-register on open**; recents/prefs in an app-config store. |
| A2 | Action enum + dispatch | ✅ Done | One `Action` enum + exhaustive `dispatch` (`action/mod.rs`), domain handlers (`query/tab/catalog/panel/overlay`). Menus emit the same actions; only editor/cmdk bindings + modal form state stay inline. |
| A3 | Overlay architecture (containers + store) | ✅ Done | Shipped as reusable **containers** (`Popup`/`Dialog`/`Window` in `ui/components/`) + a per-window **overlay store** (`overlays.rs`, a `GlobalSignal`) driving always-mounted hosts (cmdk/Settings/Export/Config); co-located popups (menus/project/remove/cell) stay on local `use_signal`. Killed the app-global `*_open` bools + `CloseOverlays` coupling. Not the `Vec<Modal>` stack originally sketched — stacking deferred (the `EscStack` upgrade path). See `OVERLAY_ARCHITECTURE.md`. |
| A4 | Modal form state off `AppState` | ✅ Done | **Export:** component-local `use_signal`; `RunExport(opts)` carries the snapshot. **Config:** component-local `draft` seeded from a store `ConfigTarget` (`New`/`Edit(name)`); the project stays **immutable until a successful register** (no placeholder). `RegisterTable(draft)` sends the engine spec + stashes the row in `overlays::pending_register`; on `Registered` the success path builds the real catalog row from the stash + engine columns and autosaves (engine events skip the dispatch autosave), the load-time path updates the existing row, and failure shows an inline `config_err` with the window still open. `AppState` now holds neither `cfg` nor `export`. See `OVERLAY_ARCHITECTURE.md`. |
| A5 | Per-tab query state | ✅ Done | **Results / plan / error scoped to the tab** (FEATURES §10). Query output moved off `AppState` into **`crate::runs`** — a per-window `GlobalSignal<HashMap<u64, WorkspaceRun>>` keyed by tab id (chosen over `AppState.runs` for reactivity isolation: AppState is one coarse `Signal`, so the store keeps find-in-results / paging / plan-toggle re-renders on the results panel only). Reducer routes by `ws_id` via `runs::is_pending` + `edit_existing` (drops results for closed/superseded tabs); reaped on tab close, cleared on project open. Also decoupled persistence: `project::Workspace` is now `{ id, name, sql }` with **no `serde(skip)`** — the tab `id` persists (`normalize` repairs legacy/dup). App-bar `status_text` stays global; results-panel status (R1) derives from the active tab's run. Compiles + runs. |
| A7 | Split `ui/workspace.rs` into a module | ✅ Done | Broke the 683-line `workspace.rs` into `ui/workspace/` — `mod.rs` (`Workspace` shell), `tabs.rs` (`Tabs`), `editor.rs` (`Editor`), `results.rs` (`Results` switch + `ResultsToolbar` / `Pager` / `Running` / `ErrorView` / `Empty` / `EmptyState` → future **R1**), `grid.rs` (`ResultsGrid` + `CellDialog` → future **R3–R6**), `plan_view.rs` (`PlanView` → future **S20**). Each pane is a `#[component]` pulling `AppState` from context → **independent reactive scopes** (typing in the editor no longer re-renders the grid); the tab menu + cell view are now component-local signals, not threaded from `Workspace` / `AppState`. Leaf render helpers called per-item (`render_cell`, `plan_node_card`, `fmt_int`, `tab_menu_items`) stay plain fns. `Workspace` is the only `pub` item. |
| A6 | Tab architecture: `.strata/` split + dirty tracking | ✅ Done | **Persistence split** (A6.1, matches Athena/VS Code): project is now a `.strata/` dir — `project.json` (committed: tables/views/saved-queries) + `session.json` (gitignored: tabs/active/history/geometry) + auto `.gitignore`. Runtime `Project` stays unified; split at load/save via `DefsFile`/`SessionFile` DTOs. Migrates legacy single-file `*.strata`/`*.psproj` on open (legacy left in place). Autosave routes: def-touching actions write both, session-only actions write `session.json` only (`touches_defs` in `dispatch`; reducer autosaves defs on view-change/deregister). **Binding + dirty** (A6.2–4): `Origin { Scratch, View(n), SavedQuery(n) }` + `origin_hash` (FNV-1a of the bound SQL) on `Workspace`, set on open (edit-view/open-saved/select-*) and rebound on ⌘S/save-as-view; `is_dirty` = a View/SavedQuery-bound tab whose SQL diverged from its baseline hash; **scratch tabs (Tier 2 session buffers) are never dirty** (they have no committed def to diverge from, and restore from `session.json`). Tab dirty-dot (orange `.tdot`) + header Save emphasis; discard-on-close confirm via `overlays::close_confirm` + `CloseConfirmHost` + `CloseTabForce`. Supersedes C1. **Unverified — needs compile.** |

## B. v2 design (all shipped)

| ID | Task | Status | Notes |
| --- | --- | --- | --- |
| B1 | Event Log panel | ✅ Done | Fed from real engine events (query/register/view/export/errors), colour dot by kind, cap 200. Now folded into the S5 drawer. |
| B2 | Catalog row context menu | ✅ Done | Kind-specific items (table/view/saved-query), cursor-positioned, backdrop closes. |
| B3 | Workspace tab context menu | ✅ Done | Rename / close / others / right / all / reopen (⇧⌘T) + closed-tab stack. |
| B4 | Inline rename | ✅ Done | Tabs (inline input, Enter/Esc/blur) + catalog objects (engine rename = deregister+register / drop+create). |
| B5 | Remove-confirmation dialog | ✅ Done | "Drop table/view?" modal gating the sidebar ✕ and context-menu Remove. |
| B6 | Resizable panels | ✅ Done | Drag handles for sidebar / inspector / editor / log sizes; clamped; body cursor on drag; persisted. |
| B7 | Saved Queries catalog section | ✅ Done | Project SQL snippets (distinct from real views) + "Save query to project" editor action. |

## B2. v4 design

| ID | Task | Status | Notes |
| --- | --- | --- | --- |
| B8 | Welcome / Launcher + multi-window | ✅ Done | `window.rs`: each project its own window + engine; separate launcher window (opens only when the last project closes); ⌘` cycling; titlebar drag; per-project geometry in `.strata`; per-engine snapshot scoping. **Thin:** geometry in physical px (off on mixed-DPI). |
| B9 | Collapsed sidebar rail | ✅ Done | 46px icon rail (expand / catalog / new-table) when `!sidebar_open`. |
| B10 | Open in current-vs-new window prompt | ✅ Done | When `open_pref == Ask`, opening from a project window (Open Project **or** Open Recent) shows the prompt — This Window / New Window / Cancel + a "remember, don't ask again" toggle that persists the choice. `overlays::open_prompt` + always-mounted `OpenPromptHost` → `OpenPromptCard` (child mounted only while open, so the checkbox resets each open) + `Action::OpenChosen`; `projects::open_with_pref` routes this/new/ask. Launcher unaffected (spawns windows directly). Also folded in: `open_pref` `String` → `config::OpenPref` enum (serde lowercase, back-compat), and a reusable `ui::components::Checkbox` (button `role=checkbox`, controlled, our own — dioxus-primitives is unreleased + needs the CLI). |
| B11a | Launcher project actions + pinning | ✅ Done | Launcher Projects pane (§0): per-row actions — **Pin/Unpin** (PINNED/RECENT sections), **Open in new window**, **Reveal on disk**, **Remove from list** — plus a **search** box over name/path, colour-initial avatars, and a pin badge. Row actions `stop_propagation` over the row's open-click; reactive `recents` signal re-renders on mutate. Shipped alongside: **reopen the whole set of open projects on startup** — `AppConfig.open_projects` (added on `install`, removed on every window-close path), `decide_startup` reopens all still-existing entries; setting renamed "Reopen projects on startup". |
| B11b | Launcher Settings entry point | ⬜ Todo | **v10 de-duplicated this** — the launcher no longer re-implements settings as an inline pane. Its left rail is **Projects** (top) + a **Settings** gear (bottom, IntelliJ-style) that opens the **same draggable Settings window** the workspace uses (`onOpenSettings`); all five global categories show (no per-project settings). Work = wire the launcher gear to open the shared Settings window from the launcher's separate VirtualDom, and drop the old inline-pane idea. No longer gated on S21 (the Settings window already exists). |
| B12 | Tab: Duplicate | ⬜ Todo | **v10:** a **Duplicate** entry in the per-tab right-click menu (between Rename and Close) — clones the tab's current SQL into a new "&lt;name&gt; copy" tab inserted immediately to the right and made active (same `onTabAction` dispatch, `act: "duplicate"`). |

## S. Strata redesign (v5 handoff — current priority)

| ID | Task | Status | Notes |
| --- | --- | --- | --- |
| S1 | Rename to Strata + new icon | ✅ Done | Renamed Parquet Studio → Strata; sediment-layers logo; `.psproj` → `.strata` (opens legacy too); app icon via `Dioxus.toml [bundle]` + runtime `with_window_icon`; full branding sweep. |
| S2 | Theming: JSON theme files + token injection | ✅ Done | Themes are JSON (`theme.rs`) — token map + `extends` + mode; loader discovers built-ins + user dir; injected as `--*` CSS vars; whole stylesheet swept to `var()`; persists to config. **Remaining:** plugin theme dirs, live reload. |
| S3 | Settings modal | ✅ Done | `⌘,` modal, 4 categories (Appearance / Data display / System / Keymap); all prefs **persist + enforced**. Follow-ups owned elsewhere: Ask-prompt → B10, confirm-close → S14, OS-sync live listener + search box + reload-themes (minor). |
| S4 | Toolbar moved into header | ✅ Done | Run (accent + ⌘↵) / Format / Clear / Save-view / Save-query now header icon-btns (shown when a tab is open); editor run-bar removed; dropped DataFusion badge + proj meta. Run↔Cancel toggle is S14. |
| S5 | History + Events → tabbed bottom drawer | ✅ Done | One drawer (`drawer.rs`) with History / Events tabs, tab-aware Clear, expand/close; history single-click loads, double-click runs (idempotent). Added `LogKind::Run`/`Warn`. |
| S6 | Error view on failed queries | ✅ Done | Results-area error banner (typed class + `line:col` + code frame + caret + hint) **and** expandable Events rows, via shared `errview::error_detail`; `query_error.rs` parses DF error strings (unit-tested). Follow-up: engine-surfaced line/col. |
| S7 | SQL autocomplete | ⬜ Todo | Context-aware completion (tables after FROM/JOIN, columns after `alias.`, else pooled), ⌘Space, caret dropdown, ↑↓/Enter/Esc, flip-up. Editor is uncontrolled → likely a custom overlay; investigate feasibility first. |
| S8 | Tab-bar controls | ⬜ Todo | Pinned right cluster: + new-tab, "show all tabs" searchable popover, ⋯ overflow (close all / others / reopen). |
| S9 | Catalog row menu polish | ✅ Done | Vertical-⋮ row menu (Open / Configure / Edit / Rename / Remove); fixed the ellipsis glyph (was horizontal). |
| S10 | Reduce redundant status text | ✅ Done | The redundant right-aligned `rows · ms · cols` readout is gone from the status bar (empty right/spacer, matching the design); result stats live only in the pager. The left status line legitimately shows the latest log/status message. |
| S11 | File menu (native app menu) | ⬜ Todo | Native File menu (Open / Recent / Close / Settings / Save All) via muda. *Not in design — Alex's add.* |
| S12 | EXPLAIN query-plan view | 🟡 Thin | Dedicated engine path walks DataFusion's **typed** LogicalPlan/ExecutionPlan + live `MetricsSet` — **no text/JSON parsing**. Physical/Logical tabs (both modes), Raw/Tree, HOTSPOT + per-node metrics. Reflects the pre-v3 design; **v3 rework outstanding → S20** (self-time, 3-tier metrics, detail field-rows). ANALYZE still buffers via `collect` (stream-drain follow-up). |
| S13 | Query-lifecycle states in results area | ✅ Done | Four-way switch: no-results / running spinner / error banner / grid. Cancel button on the Running state → S14. |
| S14 | Query cancellation + confirm-on-close | ⬜ Todo | Run↔Cancel + Esc aborts the in-flight query (engine drops the task) → `cancelled` history + `warn` event. Plus enforce the confirm-before-closing-a-running-window dialog (setting already persists). |
| S15 | System / behaviour settings | ✅ Done | System Settings category — reopen-last (threaded into startup: launcher vs project window), default project dir, opening-a-project pref, row limit — all **live + persisted**. Settings moved off `AppState` into a per-window `settings` `GlobalSignal` store (flattened into `AppConfig`); density/zebra now reactive, OS-sync follows `ThemeChanged` live. Remaining slivers owned elsewhere: the confirm-on-close **dialog** → S14; settings search box → S18; Engine category → S17. See `settings.rs`. |
| S16 | Author-friendly theme files + JSON Schema | ⬜ Todo | Restructure theme JSON into named groups + ship `theme.schema.json` (autocomplete/validation); loader flattens groups → `--*`; keep flat-map back-compat; regenerate `REQUIRED_TOKENS` from the schema. |
| S17 | Engine settings category + SQL escape hatch | ⬜ Todo | 5th Settings category (§15): a curated **~11-knob** subset of DataFusion `ConfigOptions` grouped Execution / Memory & spill / SQL parser / Result format / Optimizer — type-aware controls (number / text-with-suffix / segmented / toggle), each row shows the full `datafusion.*` key, a changed row gets a **MODIFIED** badge + per-row reset, and a **Reset all (n)** clears every override. Only *overrides* are stored (`engineCfg`), so `SHOW`/reset stay honest; `format.null` wired **live** to the grid. **Editor escape hatch:** a lone `SET` / `RESET` / `SHOW` / `SHOW ALL datafusion.*` is intercepted in the run path (no scan) → applied to overrides (`SET`/`RESET` confirm via event log + toast; `SHOW`/`SHOW ALL` open a synthetic config result set); unknown `datafusion.*` keys accepted; non-`datafusion.` statements fall through as queries. Every knob searchable in the settings box. |
| S18 | Settings: history-limit + search box | ⬜ Todo | **Query-history limit** control (25 / 50 / 100 / 200, default 50) under System → History; lowering it trims the current list immediately and new runs drop the oldest once capped. **Settings search** box above the category nav — a live index over label / keywords / category that replaces the category list while typing; Enter jumps to the top hit, clicking a result switches to its category + scrolls it in + briefly flashes the setting, Esc clears. (S3 flagged search as a minor follow-up.) |
| S19 | Tab drag-to-reorder | ⬜ Todo | Press-drag a tab: a solid clone lifts + follows the cursor, the source dims, an accent insertion line shows the drop slot; drop commits the reorder (active tab tracked by **identity** so focus never changes). Auto-scroll near either overflow edge; a 5px threshold keeps plain clicks + the ✕ working; suppressed while a tab is being renamed. Pointer events + a floating clone (native HTML5 DnD doesn't fire reliably in the webview). Distinct from S8's tab-bar controls. |
| S20 | EXPLAIN plan → v3 rework | ⬜ Todo | Rebuild the plan view to `EXPLAIN_PLAN_SPEC.md` **v3** (supersedes S12's shape). Engine sends a derived **self-time** per node (§7: scan `processing`, join `build+join`, exchange `repartition`, else `elapsed_compute`) driving the headline time chip, the time-share bar, and **HOTSPOT = self-time ≥ 60% of max**. **3-tier metrics** (ANALYZE, physical tab): tier-1 headline (`rows` · self-time · `bytes` · bar) · tier-2 priority-ordered non-zero insights (errors → spills → row-group pruning → pushdown → peak/build mem) · tier-3 collapsed **typed grid** grouped Output / Time / I/O / Pruning / Memory / Exchange / Join / Errors / Other, category-coloured left bars, values coloured by metric `type`, per-node **show-zeros** toggle. `detail` parsed into aligned **key/val field rows** (top-level-comma split, bracket-aware; clamp-by-field-count + show-more). Toolbar de-dup (drop the redundant `Logical plan · N` text). |
| S21 | Connections pane + remote object-store sources (v10) | ⬜ Todo | **Redesigned in v10** — Connections are a **project-scoped tool window on the left activity rail**, not a Settings category. Rail top group = **Catalog** \| **Connections** (`sidebarPane`; clicking the active pane collapses the sidebar, VS Code model). Pane lists saved object stores (provider icon · bucket · **status dot**: green *Connected* = env / anonymous / access-key-with-keys, amber *Needs credentials*); ⋮/right-click row-menu **Edit connection** / **Forget connection** (latter via remove-confirm); empty state + **Add**. **Add/Edit modal:** Bucket URL (Add-only, → provider label; read-only subtitle on Edit), **Authentication** segmented **Environment / Access key / Anonymous**, **Region**, + Access Key ID/Secret for access-key; disabled until the URL resolves. Keyed by **scheme+authority (bucket)** in `connections` — the **same map** the Configure-table auth panel reads, so connecting a bucket flips referencing tables *Needs credentials → Connected* live. **Creds scoped to the connection + saved with the project** (no OS-keychain/global split; prototype = session state). **Remote sources in Configure-table** (FEATURES §6): paths may be `s3://` / `gs://`·`gcs://` / `az://`·`abfs://` / `http(s)://` through one `ListingTableUrl` (globs/dirs/Hive work identically); remote panel names derived bucket/provider + live status; **public-bucket** toggle (→ `aws.SKIP_SIGNATURE true`, HTTP never needs creds); **one-table = one object store** guard (derived from the first path — mixing buckets/providers or local+remote is flagged + blocks Register, *"UNION in a view"*); a cloud path with no connection and not public blocks Register with an inline *connect-this-bucket* prompt. Design: v10 `Strata.dc.html` + FEATURES §6/§15b + CHANGELOG. *(The v9 Settings/Export/Configure window-frame unification is already shipped via the A3/A4 `Window`; Settings is now 100% global — Appearance / Data display / System / Keymap / Engine.)* |
| S22 | Top bar doubles as the native macOS title bar (v10) | ⬜ Todo | **v10:** the app header *is* the OS title bar (macOS `titleBarStyle: hiddenInset` + `titlebarAppearsTransparent` + `fullSizeContentView`) — no separate OS bar, no wasted height. `-webkit-app-region: drag` on `<header>` with `app-region: no-drag` on every interactive control (logo / switcher / search / settings); **88px left inset** reserves the traffic-light zone (OS draws them); project-switcher dropdown offset tracks the shifted button. **Windows/Linux TODO (not stubbed):** controls belong top-*right* — native frame or a custom min/max/close cluster. |

## R. Results, charting & workspace (v8 handoff)

The results panel + its status bar were substantially reworked in v6–v8. Per-tab
result state is **A5**; the UI is below.

| ID | Task | Status | Notes |
| --- | --- | --- | --- |
| R1 | Unified results status bar + workspace rework | ⬜ Todo | Rework the results panel to the v8 layout. A single **40px status bar** at its foot in **every** state (no-run · running · failed · grid · plan): state-coloured dot + monospace label + subtext (`⌘↵ to run` / `scanning <target>` / error type / `· N ms` / `N operators`), the **snapshot clock chip** (`⏱ snapshot 3m ago`, ticking ~15s), the live **selection readout** (R3), and the **pager** (page-size ▲ dropdown + first/prev/page-input `of M`/next/last) pinned right, shown **grid-only**. Plus the results **toolbar** row: **Find-in-results** (grid-only, live match count) · **Grid/Chart** segmented toggle (R2) · **Refresh** (re-run + reset snapshot label + clear selection) · **Download** (export). Today `workspace.rs` renders only a bare `pager()` and the app's `statusbar.rs` (Events + History) stays separate; the state-driven token, snapshot chip and selection readout don't exist. Snapshot timestamp is **per result-set** (pairs with A5). |
| R2 | Chart view (snapshot) | ⬜ Todo | New results view — full spec in **`CHART_SPEC.md`**. Grid/Chart toggle swaps the grid for a two-pane chart layout (left encoder strip + right canvas). Six types (bar / line / area / scatter / histogram / pie) on a **dependency-free canvas**, theme-aware via CSS vars. Encoders X / Y / Series / type + **Aggregate** (client-side GROUP BY over the snapshot, **default ON**) with `sum/avg/median/min/max/count/distinct`; scatter trendline; line/area moving-avg / running-total overlay. Schema-driven default encoding, always visible + overridable. **Guardrails** (never silently sample): >200k rows → too-big · aggregate-off >2k → raw-too-big · >60 groups → non-blocking hi-card banner; the **Add GROUP BY in SQL** CTA scaffolds real, user-owned SQL into a **new editable tab** (`date_bin`-bucketed for temporal X). Config **per result-set**; never re-queries source files. Real-engine upgrades (aggregate in DataFusion via `MemTable`, `approx_distinct`, window overlays) in §8. |
| R3 | Grid selection + live aggregate | ⬜ Todo | Spreadsheet-style selection on the results grid (§10): **cell/range** (click, drag-sweep, shift-extend; accent focus outline + tinted block), **row multi-select** (gutter click / drag / ⌘-toggle / shift-run), **column multi-select** (header click / drag / ⌘-toggle; persists across pages). **⌘A** / top-left `#` = whole page; **Esc** / empty-area click clears; cleared on Refresh + re-run. **⌘C** copies — a range → **TSV**, a row → **CSV w/ header**, a column → **CSV w/ header over the whole snapshot** (skipped when a text field is focused). **Live aggregate** in the status bar: `N cells · R×C` (or `K columns · N values`) + `sum / avg / min / max` over numeric cells (cell/row = current page, column = whole snapshot). |
| R4 | Copy affordances (cell / row / column / all) | ⬜ Todo | Right-click any result cell → copy menu (§10): **Copy cell**, **Copy row (JSON)**, **Copy row (CSV)**, **Copy column "&lt;name&gt;"** (all snapshot values), **Copy all as CSV** / **Copy all as Markdown**. Operates on the local snapshot honouring the active sort; CSV cells quote-escaped, nested values as inline JSON; confirms via a status-bar toast (`Copied 48,213 rows as CSV`). |
| R5 | Row detail (record view) | ⬜ Todo | Double-click a row's **number cell** (or "Open row detail" from its right-click menu) → the whole row as a vertical **key→value card** (field name + Arrow type left, value right); nested struct/list/map render as pretty JSON, null dimmed. Header **Row N of M** with prev/next stepping the whole snapshot (respects active sort), **Copy JSON / Copy CSV**, close + backdrop-close. Ideal for wide / nested tables. |
| R6 | Column sort (chevron, snapshot) | ⬜ Todo | Per-header **▲/▼ chevron** cycling **asc → desc → clear** (accent + name highlight when active). Sorts the **local result snapshot** (the same data pagination reads) — covers the whole result set, **nulls last**, re-paginates from page 1, **no re-query**. Remembered **per result-set**. Sort moved off the header **click** (which now selects the column for R3) onto the explicit chevron. |

## C. Outstanding from v1

| ID | Task | Status | Notes |
| --- | --- | --- | --- |
| C1 | Project persistence + app-config store | ✅ Done | `.strata` save/load + `preferences` app-config (recents); source paths relative-when-inside-project; RustRover directory-based open; centralized autosave via `is_durable`. **Thin:** autosave not debounced; no scratch-SQL dirty indicator. |
| C2 | Real Export | ✅ Done | `COPY … TO` for CSV / JSON / Parquet / Arrow (+ partitioned Hive dir), bounded memory; redesigned modal (scope / format / options / partition chips / live preview); clipboard md/tsv/csv/json. See `EXPORT_OPTIONS.md`. |
| C3 | Table Config validation states | ✅ Done | Synchronous bounded fs scan: format-match, matched-count, directories-only partitioning gate, **real `key=value` partition detection** + per-key type pickers, identifier check, ≥1-path, disabled-until-valid, failed-registration cleanup; combined file/folder picker; single-file format auto-detect. Remaining → C10. |
| C4 | Open catalog SQL in its own tab | ✅ Done | `open_in_tab` — edit-view / SELECT * / open-saved-query reuse a same-named tab or append; never clobber the active tab. |
| C5 | Command palette depth | 🟡 Thin | Typed `PaletteCommand` + concrete actions done. Remaining: columns group, full keyboard nav (↑↓/Enter/hover), per-item metadata + shortcut hints, type-coloured icons. |
| C6 | Inspector stats | ⛔ Dropped | v5: inspector is **free-metadata only** (name · Arrow type · source badge · nested-fields tree). No fabricated distinct/mean/histogram. |
| C7 | Find-in-results → Filter-in-SQL | ⛔ Dropped | Not wanted. |
| C8 | Nested cells as real JSON | 🟡 Thin | Emit parseable JSON (arrow-json) in the cell popover + recursive nested schema in the inspector. |
| C9 | PART badges on auto-registered tables | ⬜ Todo | Echo partition metadata in the `Registered` event so partitioned tables show `PART`. |
| C10 | Homogeneous-folder / JSON-shape detection | ⬜ Todo | Schema-consistency report (`100 files · consistent ✓` / `97 match · 3 extra column`). |
| C11 | Theming controls | ⛔ Superseded | By S2 + S3 (token system + Settings). v5 drops the accent-swatch picker; density/zebra stay settings. |
| C12 | Undo/redo in the SQL editor (per tab) | ⬜ Todo | ⌘Z / ⇧⌘Z in the active tab's buffer. Open Q: rely on `dioxus-code` history vs an explicit per-workspace stack (survives tab switch/reload). Distinct from `ReopenTab`. |
| C13 | Import (read) options for CSV/JSON | 🚧 Blocked | CSV read options (delimiter/header/null/quote) → `TableSpec` → `register_external`; JSON = NDJSON-only. **Design pending** (`IMPORT_OPTIONS.md`); Alex getting an updated design. |

---

## Suggested order (remaining)

The **v10 handoff** (section **R** + S17–S22 + B11a/b) is the new headline. Rough
order: **A5** per-tab result state is the enabler → **R1** results status-bar /
workspace rework → **R6/R3** column sort + selection → **R4/R5** copy + record
view → **R2** chart view → **S17** engine settings → **S20** plan v3. Older
section **S**, in rough dependency order: **S8** tab-bar controls → **S19** tab
drag-reorder → **S7** autocomplete → **S11** File menu → **S14** query
cancellation (adds the Cancel button to S13's Running state).
Theming follow-ups: **S16** author-friendly theme files, OS-sync live listener,
live theme reload, plugin theme dirs.

Remaining pre-v5: **C5** palette depth · **C8** nested JSON · **C9** PART badges ·
**C10** folder/JSON-shape detection · **C12** editor undo/redo.

Blocked on design: **B10** open-in current-vs-new window · **C13** import options.

Architecture cleanups: **A3** + **A4** both shipped — overlays run on reusable
containers + a per-window store; modal form state (Config/Export) is off
`AppState`. See `OVERLAY_ARCHITECTURE.md`. **A5** (per-tab query state + view/
saved-query dirty tracking) is the remaining foundation item.

Small follow-ups: debounce autosave · stream-drain ANALYZE to bound memory · B8
window geometry in logical (not physical) px · typed source paths don't
auto-relativize (only picks do) · verify the unified floating-window chrome
(A3/A4 `Window`) matches the v8 spec — non-blocking / no backdrop, multiple open
at once, click-to-raise z-order, per-session geometry. (Scratch-SQL dirty
indicator → A6; plan self-time → S20.)

## Done (reference)

Shell / 3-pane layout · DataFusion 43 engine on a background thread + channels ·
DDL policy gate (sqlparser AST) · snapshot-streaming pagination (single pass,
bounded memory) + cleanup lifecycle · `tracing` of all engine failures ·
`dioxus-code` editor (sql/json) · macOS integrated title bar · global scrollbar
theming. Plus everything marked ✅ above (A1–A4, B1–B9, S1–S6/S9/S10/S13/S15,
C1–C4). (S12 is now 🟡 Thin — v3 rework tracked as S20; A5 is Todo.) The old runtime `sample.rs` generator is gone — dev launch opens the
bundled `sample/sample.strata` via the real project-open path.
