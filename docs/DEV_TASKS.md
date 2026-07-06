# Strata — Dev backlog

Living backlog and source of truth for outstanding work. Design reference:
`Strata.dc.html` (v5 handoff) + `FEATURES.md` (both in `docs/`). Product was
**renamed Parquet Studio → Strata** in the v5 redesign (section **S**).

**Status:** ✅ Done · 🟡 Thin (wired but shallow) · ⬜ Todo · 🚧 Blocked (design pending) · ⛔ Dropped / superseded

---

## A. Architecture foundation

| ID | Task | Status | Notes |
| --- | --- | --- | --- |
| A1 | Project-scoped state | ✅ Done | `Project` domain model in `project.rs`; `AppState` holds it + engine handle + global prefs. Locked: theme/panel sizes are **global**; tables persist as **specs, re-register on open**; recents/prefs in an app-config store. |
| A2 | Action enum + dispatch | ✅ Done | One `Action` enum + exhaustive `dispatch` (`action/mod.rs`), domain handlers (`query/tab/catalog/panel/overlay`). Menus emit the same actions; only editor/cmdk bindings + modal form state stay inline. |
| A3 | Overlay architecture (containers + store) | ✅ Done | Shipped as reusable **containers** (`Popup`/`Dialog`/`Window` in `ui/components/`) + a per-window **overlay store** (`overlays.rs`, a `GlobalSignal`) driving always-mounted hosts (cmdk/Settings/Export/Config); co-located popups (menus/project/remove/cell) stay on local `use_signal`. Killed the app-global `*_open` bools + `CloseOverlays` coupling. Not the `Vec<Modal>` stack originally sketched — stacking deferred (the `EscStack` upgrade path). See `OVERLAY_ARCHITECTURE.md`. |
| A4 | Modal form state off `AppState` | 🟡 Thin | **Export:** ✅ done — component-local `use_signal`; `RunExport(opts)` carries the snapshot; `AppState.export` gone. **Config:** design locked, not built. Model: the **draft is a local working copy**, project data **immutable until a successful register** (so *no ghost row*, no write-back). Store `config: Option<ConfigTarget>` (`New`\|`Edit(name)`) seeds the draft (blank / copy of the project table); all edits mutate the local draft; the source **scan** becomes a component-side future keyed on `draft.sources` (local `scanning`/`file_count`/`scan_error`). Save → client-validate → `dispatch(RegisterTable(draft))`; on engine success the project store gains/replaces the table + window closes; on **failure the window stays open** with an inline error routed via a store `config_error: Option<String>`. Deletes `AppState.cfg` + the `Registered` ghost-row cleanup. |

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
| B10 | Open in current-vs-new window prompt | ⬜ Todo | "Open Project" modal (This Window / New Window / Cancel + "don't ask again") when opening from inside a project window. **Designed (v7)**, not built. |

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
| S12 | EXPLAIN query-plan view | ✅ Done | Dedicated engine path walks DataFusion's **typed** LogicalPlan/ExecutionPlan + live `MetricsSet` — **no text/JSON parsing**. Physical/Logical tabs (both modes), Raw/Tree, HOTSPOT + per-node metrics. **Known gap:** bar/HOTSPOT key off `elapsed_compute` (≈0 for scans) — self-time fix in `EXPLAIN_PLAN_SPEC.md` §8; ANALYZE buffers via `collect` (stream-drain follow-up). |
| S13 | Query-lifecycle states in results area | ✅ Done | Four-way switch: no-results / running spinner / error banner / grid. Cancel button on the Running state → S14. |
| S14 | Query cancellation + confirm-on-close | ⬜ Todo | Run↔Cancel + Esc aborts the in-flight query (engine drops the task) → `cancelled` history + `warn` event. Plus enforce the confirm-before-closing-a-running-window dialog (setting already persists). |
| S15 | System / behaviour settings | ⬜ Todo | System Settings category (reopen-last, confirm-close, default dir, row limit). **Largely covered by S3's System category** — expose any remaining as toggles. |
| S16 | Author-friendly theme files + JSON Schema | ⬜ Todo | Restructure theme JSON into named groups + ship `theme.schema.json` (autocomplete/validation); loader flattens groups → `--*`; keep flat-map back-compat; regenerate `REQUIRED_TOKENS` from the schema. |

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

Section **S** (the v5 Strata redesign) is the priority. Remaining S, in rough
dependency order: **S8** tab-bar controls → **S7** autocomplete → **S11** File
menu → **S14** query cancellation (adds the Cancel button to S13's Running state).
Theming follow-ups: **S16** author-friendly theme files, OS-sync live listener,
live theme reload, plugin theme dirs.

Remaining pre-v5: **C5** palette depth · **C8** nested JSON · **C9** PART badges ·
**C10** folder/JSON-shape detection · **C12** editor undo/redo.

Blocked on design: **B10** open-in current-vs-new window · **C13** import options.

Architecture cleanups: **A4** — Config form state off `AppState` (a dedicated
`cfg` store, since it's engine-coupled; Export's form is already localized).
(**A3** shipped — see the overlay containers + store.)

Small follow-ups: debounce autosave · scratch-SQL dirty indicator · self-time cost
metric for the plan view (`EXPLAIN_PLAN_SPEC.md` §8) · stream-drain ANALYZE to
bound memory · B8 window geometry in logical (not physical) px · typed source
paths don't auto-relativize (only picks do).

## Done (reference)

Shell / 3-pane layout · DataFusion 43 engine on a background thread + channels ·
DDL policy gate (sqlparser AST) · snapshot-streaming pagination (single pass,
bounded memory) + cleanup lifecycle · `tracing` of all engine failures ·
`dioxus-code` editor (sql/json) · macOS integrated title bar · global scrollbar
theming. Plus everything marked ✅ above (A1–A2, B1–B9, S1–S6/S9/S10/S12–S13,
C1–C4). The old runtime `sample.rs` generator is gone — dev launch opens the
bundled `sample/sample.strata` via the real project-open path.
