# Strata — Dev backlog

**Complete refresh — 2026-07-10**, re-derived from the current app state against the
**v19 design handoff**: the `Design.dc.html` design system + the per-window canvases
(`Strata` / `Settings` / `Launcher` / `Windows` `.dc.html`), and the specs in `docs/`
(`FEATURES.md`, `CHART_SPEC.md`, `CONNECTIONS_SPEC.md`, `EXPLAIN_PLAN_SPEC.md`).

Reframed around two axes:

1. **UI surfaces (Part 1)** — every part of the app **audited design-vs-code** against
   the v19 canvas: what's the concrete drift, and is it an *align* (restyle) or a *build*.
2. **Functional workstreams (Part 2)** — behaviour/feature work.

**v19 headline change:** a single **spacing + radius token scale** (`--sp-1..9`, `--r-xs..4`)
that every padding/margin/gap/border-radius snaps to — the next step after the S28
type/colour/control tokens. It's the universal **F3** task; every Part-1 row gets it *on top of*
the structural drift listed.

**Legend / drift rating:** `token-only` (just the F3 pass) · `restyle` (visual/treatment) ·
`partial-rebuild` (structural changes + some net-new elements) · `rebuild` (largely redo) ·
`build-new` (doesn't exist yet). Old IDs (S/R/C…) kept in parentheses for spec traceability.

---

## Foundations — design system (cross-cutting)

| ID | Item | Status | Notes |
| --- | --- | --- | --- |
| F1 | Type / colour / control / icon / dot tokens + components (S28) | ✅ | `src/ui/components/` fully migrated; ~90 dead legacy CSS rules retired. |
| F2 | Overlay / menu family (S29) | ✅ | `Popup`/`Backdrop`/`Tooltip` + `Select`/`DropdownMenu`/`ContextMenu`. |
| F3 | **Spacing & radius token scale (v19 §03)** | ✅ | Scale in `:root` (`--sp-1:2…sp-9:48` + `--r-xs:4…r-4:14`); snapped **~430 main.css + ~168 RSX-inline** padding/margin/gap/border-radius decls to tokens (radius uses the r-scale, spacing the sp-scale). Kept literal: 82px mac traffic-light inset + 60px empty-state pad (>52), negative resizer margins, `50%` pills. Off-target props (width/height/box-shadow/letter-spacing/font-size) untouched; braces balanced. ⚠️ snapping *shifts* many values ~1–2px by design (6/7/9→8, 11/13→12) — build + eyeball. |
| F4 | Theme tokens (S2) | ✅ | JSON themes → `--*` vars. Author-friendly restructure = W5. |
| F5 | **Platform shims behind a trait** | ⬜ | Consolidate the scattered macOS `objc` shims (`window::send_select_all` selectAll:, `paint_ns_background`, `ns_window`/traffic-light insets) behind one platform trait + per-OS impls, so the cross-platform seam is explicit and non-mac builds get real (not silently no-op) fallbacks. Today select-all is inline `#[cfg(macos)]` objc + a non-mac **no-op** stub (eval fallback deferred here); other shims are bare `#[cfg(macos)]` fns. macOS-first is fine — this just makes the boundary safe to extend. |
| F6 | **Direct Arrow → `serde_json::Value` (no text round-trip)** | ⬜ | Every JSON path (`serialize::PrettyJsonWriter`, `cell_pretty_json`, CSV/MD `flatten_nested`) currently goes Arrow → arrow-json compact bytes → `serde_json::from_slice` → `Value` → re-serialize; that reparse is pure waste and now lives in 3 places. Build one Arrow→`Value` path feeding all of them. Options weighed: **serde_arrow** (`arrow-54` feature, unifies on datafusion's arrow) is the maintained "arrow→serde directly" crate but deserializes `Decimal128` as a **string** (arrow-json emits a number), drops `Decimal256`, and limits timestamps to no-tz/UTC — a fidelity change; **hand-roll** a `to_value(array, idx)` mirroring arrow-json's private `encoder.rs` keeps decimal-as-number but we own the type matrix (~500 lines). Parked: all uses are user-initiated + page-bounded (ms cost), so no hot path yet — revisit when one appears or we accept the serde_arrow trade. |

---

## Part 1 — UI surfaces: alignment drift

This is the **align** work only — where a surface is **built but doesn't match v19**.
Things that don't exist yet are **not** drift: they're features, they live in Part 2, and
each surface points to the ones that land there as **Builds here →**. The rating describes
the align work only; F3 spacing is assumed on top of every row.

### U1 · Launcher — `restyle`
Drift: "Open folder…" → design's ghost **"Open"** (uppercase); project row has 4 actions vs design's 3 (drop open-in-new-window?); Projects nav pill has an extra `border-left` accent bar (design = tinted bg only); no-match empty-state copy/placement.
Builds here → **W1** (rail Settings gear).

### U2 · App shell — header · macOS title bar · activity rail — `restyle`
Rail active-state = the standard **toggle-button** treatment (accent tint) — our `IconButtonVariant::Toggle` already matches, **no drift** (the 2.5px edge-bar still in `Strata.dc.html` is stale, superseded by the toggle-button design).
Drift: Problems badge sits top-right of the icon with a 2px ring (reposition); header recent-rows lack the branch glyph. Two-window model + macOS title bar (inset 13/21) are faithful.
Builds here → **W7** (Connections rail button + pane), **W1** (cross-window settings sync).

### U3 · Sidebar / catalog — `partial-rebuild`
Drift (built content that's structurally wrong): section headers should be **collapsible chevron rows** (currently static); table-column rows need an **indent + expand chevron for `struct` columns** (currently flat dot+name+type).
Builds here → **W7** (Connections pane), **D5** (rescan button).

### U4 · SQL editor + workspace tabs — `restyle` 🟡 (restyle aligned; tab features open)
The **restyle** is aligned: editor / autocomplete / lint-hover / tab menus / inline rename match, the tab-close **dot→× on hover** drift is fixed (**T4**), and the Run control is the three-icon toolbar (**E4**). **Not done** — the tab *feature* builds below: **T1 drag-to-reorder** (no pointer-drag handler yet) and **T2** OS-close intercept.
Builds here → **E4** (Run → three icon buttons) ✅ · **T4** (tab-close dot→× hover) ✅ · **T1** (tab drag-to-reorder) ✅ · **T2** (intercept OS-triggered closes) ⬜ **not built**.

### U5 · Results grid — `restyle` ✅
Zebra / type-colour cells / sticky header match, and all builds shipped — **Rz3** (selection + live aggregate), **Rz6** (column sort), **Rz5** (record / row-detail view). Only remaining drift is note-only: the nested-cell popover is a `Dialog`+highlighted `Code` vs the design's centred backdrop modal + `<pre>` (accepted as-is).

### U6 · Results toolbar · status bar · pager — `restyle` ✅
Done: find is a **collapsible search popover** — a new `SearchDialog` component built on the S29 `Popup`/`Backdrop` base (trigger measures its own rect, anchors `BOTTOM_END`, dismisses via the backdrop which also clears the filter; ✕ + `on` active-state on the toggle). **Table/Chart** is a text-only `Segment`; right-cluster order is now **find · refresh · clear · export**, all bordered `Toolbar` icon buttons. **Rz8** clear-results is wired: `Action::ClearResults` → `query::clear_results` (drops the active tab's result/plan/error + find query → empty state, no-op mid-run) behind a trash button with a destructive red hover. Status bar + pager already matched. New: `SearchDialog` (+ export), `.res-find-panel` is now card-only (Popup owns position), `.ds-icon-btn.toolbar.res-clear`/`.on` rules.
Builds here → **Rz3** (status selection token). ⌘F-to-open works via the global-hotkey keymap (find state lives in `runs.find_open`; ⌘F → keymap → the active toolbar's `Find` registry owner). See **W4**.

### U7 · Results — chart view — *not built*
No align work — the whole surface is a feature → **Rz2** (`CHART_SPEC.md`).

### U8 · Results — query-plan view — ✅ v3 rebuilt (Rz-plan)
Rebuilt to the `EXPLAIN_PLAN_SPEC.md` v3 shape: engine emits typed, pre-labelled
metrics + derived per-node self-time (`crate::plan::{Metric, MetricKind, self_time_ms,
insights, metric_group}`; engine classifies each `MetricValue` by variant); UI is a
three-tier card (headline rows·self-time·bytes·time-share bar → non-zero insight
callouts → collapsed grouped grid w/ hide-zeros), depth guide-rails, 2-line detail
clamp, amber ANALYZE badge, active-tab summary. ⏳ awaiting Alex's green build on Mac.

### U9 · Column inspector — `partial-rebuild`
Drift (built content that's structurally wrong): metadata **fixed 2-col grid → bordered box of dynamic key/value rows** (label-left uppercase mono / value-right, per-row borders, only real facts); drop the bespoke numeric min/max strip (not in design); add the title **source-format badge**; indent nested-field rows by depth. Completeness bar + nested-fields tree match.
Builds here → **D4** (the PROFILE zone + profile-cost-confirm).

### U10 · Bottom drawer (Problems · Events · History) — `token-only`
In sync (S23/S25). Only nits: an extra `prob-code` chip the design lacks + empty-state wording.

### U11 · Command palette (⌘K) — core built; depth is a feature
The palette works; grouping, keyboard nav, per-item type-icons + shortcut hints are the "depth" feature → **T3**. (The footer already advertises "↑↓ navigate" — wire it under T3.) No standalone drift beyond that.

### U12 · Settings — `restyle` (built parts)
Drift on the built overlay: drop the "appearance & behavior" subtitle; tooltip-vs-caption affordance; theme-card source badge. Appearance / Data-display / System match structurally.
Builds here → **W1** (standalone window + Cancel/OK footer), **W2** (Engine category), **W3** (search box + History-limit), **W4** (rebindable Keymap).

### U13 · Export modal — `rebuild`
Functionally correct, but the UI has drifted so far from v19 that it's a **complete rebuild**, not a patch. One task → **D6** (rebuild the modal to the canvas; keep the export/backend logic).

### U14 · Config / Register-table modal — `restyle` (built parts)
Drift on the built (local) modal: status order (below import-options, above Hive); add the SOURCE PATHS **REQUIRED badge + resolution tooltip**; drop the subtitle. Honesty tidies (per-path counts, no-fake-stepper) + Hive partitioning already match.
Builds here → **W7** (LOCATION toggle + object-store branch), **D8** (import-read options).

### U15 · Dialogs — `restyle`
Open-prompt / close-while-running / nested-cell popover are faithful (token-level).
Builds here → **Rz5** (record / row-detail view) ✅ shipped, **D4** (profile-cost-confirm) — not yet.

---

## Part 2 — Functional workstreams

Feature/behaviour work; several own a Part-1 "build" gap (cross-refs above).

### Settings & configuration
| ID | Task | Status | Notes |
| --- | --- | --- | --- |
| W1 | Settings → standalone `wry` window + cross-window sync (S22; owns U12 window model + launcher gear) | ⬜ | Own OS window, opened from launcher gear + project windows (one canonical, focus-if-open); mutations broadcast live to every window's `settings` store. |
| W2 | Engine settings category + SQL escape hatch (S17; owns U12 Engine) | ⬜ | ~11 DataFusion `ConfigOptions` (type-aware rows, MODIFIED badge, reset); editor `SET`/`RESET`/`SHOW datafusion.*`. |
| W3 | Settings search box + history-limit (S18; owns U12 search + System→History) | ⬜ | Live search index over the nav; query-history-limit 25/50/100/200. |
| W4 | Keymap rebinding (S24; owns U12 Keymap rebuild) | 🟡 | **Foundation shipped:** `crate::keymap` (chord→`Command`, `resolve`/`run` + a context registry for focus-dependent commands like `Find`), persisted overrides (`Settings.keybinds`, `effective_chord` = override ?? default). Global commands are delivered as **OS hotkeys** (`crate::hotkeys::use_shortcuts` → `window().create_shortcut`, registered/removed on window focus so they're not held system-wide; the scope-less callback parks the cmd in a `PENDING` global signal under a `RuntimeGuard`, a scoped effect dispatches it). `handle_key` reduced to non-global keys (Esc). ⌘W freed by dropping the menu's Close Window. **Remaining:** the Settings rebinding UI — click-to-capture chord, conflict detection, per-row + reset-all + **unbind**. Live re-registration is ready by design: `use_shortcuts`'s effect already tears down all `ShortcutHandle`s and rebuilds from `effective_chord` each run, so it only needs to also **subscribe to `Settings.keybinds`** (currently read via `peek`) to re-register on a rebind — no manual refresh. **Unbind** needs an "explicitly unbound" override representation (sentinel/`disabled` set) so `hotkey_for` returns `None`. |
| W5 | Author-friendly theme files + JSON Schema (S16) | ⬜ | Named groups + `theme.schema.json`; loader flatten; live reload + plugin dirs. |
| W7 | Connections pane + remote object stores (S21; owns U2 rail button + U3 pane + U14 LOCATION) | ⬜ | Project-scoped connections (S3/GCS/HTTP), no app-managed secrets; Config-table LOCATION toggle. `CONNECTIONS_SPEC.md`. |

### Editor & SQL
| ID | Task | Status | Notes |
| --- | --- | --- | --- |
| E1 | Validator coverage (S31) | ⬜ | Unknown table/view (reuse S26 context resolver), bad leading keyword, unterminated string; accumulate all. |
| E2 | Autocomplete follow-ups (S7) | 🟡 | ⌘Space trigger, flip-up, caret-after-accept. Core shipped. |
| E3 | Undo/redo per tab (C12) | ⬜ | ⌘Z / ⇧⌘Z; `dioxus-code` history vs explicit per-workspace stack. |
| E4 | Editor Run = three icon buttons (owns U4; supersedes S30 split-button) | ✅ | Accent **Run** (⌘↵) + neutral **Explain plan** (list) + **Explain analyze** (stopwatch); Run→red **Cancel** while running. Explain buttons dispatch `Action::RunExplain(analyze)`; the handler `query::run_explain` wraps the SQL via `plan::as_explain` (strip+reapply `EXPLAIN [ANALYZE]`, unit-tested) and routes it through the shared engine explain path — **editor buffer untouched**, like Save-as-view. Added `IconButtonVariant::Primary` (accent-fill, `.stop`=red) + `IconName::List`/`Stopwatch`. |

### Results & data
| ID | Task | Status | Notes |
| --- | --- | --- | --- |
| Rz2 | Chart view (R2; owns U7) | ⬜ | 6 types, canvas, encoder strip, client aggregate, guardrails. `CHART_SPEC.md`. |
| Rz3 | Grid selection + live aggregate (R3; owns U5 selection + U6 selection token) | ✅ | Cell/range (click + shift-extend + drag-paint) · Excel-style **headers** (plain=select-only, ⌘=toggle one, ⇧=contiguous range via `run.sel_anchor`) · `#` corner=select-all · Esc/click-off clear · status-bar aggregate. ⌘A via a **context-aware Edit-menu item** that greys out of scope (grid/text-input focus tracked → `menu::set_select_all_scope`; input select-all re-emitted natively). Copy = Rz4. |
| Rz-cols | **Resizable columns (V20)** | ✅ | Per-column drag grip on the header right edge (8px, accent line on hover **and** held lit through the drag), double-click **auto-fit** (clamp 64–520). Widths keyed by col index on the run (`col_widths`, session-scoped, survive paging/sort, reset on clear). Drag via `ResizeTarget::Column` on the existing root move/up driver; default width = `Settings.default_col_width` (struct-only). Rows size to the width-sum (`grid-inner: max-content`) so scroll kicks in + the last column always grows. |
| Rz4 | Copy affordances (R4) | ✅ | Right-click selection → **Copy as TSV / CSV / JSON / Markdown** (⌘C = TSV, via a context-aware Edit-menu Copy item). One shared Arrow serializer (`crate::serialize`): the selection is projected + `take`n into a `RecordBatch`, then written by a `RecordBatchWriter` per format — arrow-csv, `PrettyJsonWriter` (arrow-json encode + whole-document serde_json pretty), a padded/right-aligned `MarkdownWriter`; nested struct/list/map stay real JSON, flattened to compact JSON for the flat formats; all carry headers. Clipboard is page-bounded — export→clipboard dropped, export is file-only. |
| Rz5 | Record (row-detail) view (R5; owns U5/U15 record view) | ✅ | Double-click the row-number gutter → centred modal (`RecordDialog`): the row as a **key→value** card — column name over its type-coloured Arrow type, scalar values grid-coloured, **nested struct/list/map as pretty JSON** (`serialize::cell_pretty_json`) in a recessed block. `Row n of total` header, ↑/↓ prev/next (page-local, clamped), `⋯` menu → Copy as TSV/CSV/JSON/Markdown (`Action::CopyRecord` → `query::copy_record`, single-row batch through the Rz4 serializer). |
| Rz6 | Column sort (R6; owns U5 sort) | ✅ | Header sort chevron cycles asc→desc→clear; applied as an `ORDER BY` over the on-disk snapshot at page-read time (`FetchPage.sort`, DataFrame `.sort()`), nulls always last, real Arrow-type ordering. `run.sort` per result set (survives paging, reset on new result); sort re-fetches page 1. |
| Rz8 | Clear-results button (R8; owns U6 clear) | ✅ | `Action::ClearResults` → `query::clear_results`; trash in right cluster → empty state, guarded mid-run. |
| Rz-plan | Plan view v3 rework (S20; owns U8) | ✅ | Engine emits a typed, pre-labelled `Vec<Metric>` (classified by `MetricValue` variant) + derived per-node **self-time** (`crate::plan::self_time_ms`, §7); UI = three-tier card (headline rows·self-time·bytes·time-share bar → non-zero `insights()` callouts → collapsed `metric_group()` grid w/ hide-zeros), depth guide-rails, 2-line detail clamp, amber ANALYZE badge, active-tab summary. `EXPLAIN_PLAN_SPEC.md` v3. ⏳ awaiting green build on Mac. |

### Catalog & sources
| ID | Task | Status | Notes |
| --- | --- | --- | --- |
| D4 | Column/table profiling (C14; owns U9 PROFILE zone + U15 cost-confirm) | ⬜ | Inspector FROM FILE / PROFILE (scan-derived, cached, cost-honest); nested never element-traversed. |
| D5 | Catalog re-scan (C15; owns U3 rescan button) | ⬜ | Re-infer ListingTable file sets; log; invalidate profiles. |
| D6 | Export modal — complete UI rebuild to v19 (C17; owns U13) | ⬜ | Functionally correct but heavily drifted → **rebuild the whole modal UI** to the v19 canvas, keeping the export/backend logic. Scope: data-driven per-format option groups (core + **ADVANCED** section) instead of hard-coded `match` arms; CSV delimiter as a **text input** (resolve `\t`/`\n`); compression via `Select`; drop the extra "Null as" segmented + the embedded **DESTINATION** field (filename → the separate Save-file browser); partition chips + **warning banner/hint**; UPPERCASE section labels. |
| D7 | Config-table honesty tidy (C16; owns U14 non-remote) | ⬜ | Status order, REQUIRED badge + tooltip, subtitle. (Counts/stepper already honest.) |
| D8 | Import (read) options CSV/JSON (C13; owns U14 import section) | ⬜ | **Designed in v19** (`Strata.dc.html` §~2205–2313): a format-specific import-read-options block in the config modal — core groups + a collapsible **ADVANCED**, data-driven inputs (CSV delimiter/header/null/quote/skip/comment, JSON settings). Wire the controls → `TableSpec` → `register_external`. *(Was mis-marked 🚧 blocked from the old C13 "design pending" — the design now exists; unblocked.)* |
| D9 | Nested JSON · PART badges · folder/JSON-shape detection (C8/C9/C10) | 🟡/⬜ | Parseable JSON (thin) · echo partition metadata for `PART` · schema-consistency report. |

### Tabs, windows & misc
| ID | Task | Status | Notes |
| --- | --- | --- | --- |
| T1 | Tab drag-to-reorder (S19) | ✅ | Mousedown arms a drag; the root pointer-driver promotes it past a threshold (`AppState.tab_drag`, mirrors the resize driver). JetBrains-style bespoke visuals: the dragged tab is **lifted out** of the strip (not rendered), a floating `.ws-tab-ghost` rides the cursor, and a `.ws-tab-slot` gap opens at the drop point. The drop index is computed in **visible (post-removal) order** from the hovered tab's midpoint (per-tab `onmousemove` + measured widths), so every slot — including the origin — is reachable. **Edge auto-scroll**: a spawned loop scrolls the track (`MountedData::scroll`) while the pointer sits near either edge. Reorder = `session::move_workspace` (active stays active by id); autosaves the session. |
| T2 | Intercept OS-triggered closes (A8) | ⬜ | Red-button / ⌘Q / dock → themed RunningClose (unsafe objc; iterate-together). |
| T3 | Command palette depth (C5; owns U11) | 🟡 | Grouping, keyboard nav, per-item icons + shortcut hints, columns group. |
| T4 | Tab close dot→× hover-swap (U4 nicety) | ✅ | Close slot shows × on a clean tab, and on a dirty tab an unsaved dot that becomes × on hover (CSS off `.ws-tab.dirty` / `:hover`); the dirty marker moved off the leading status dot into the close slot, per the canvas. |

---

## Part 3 — Done (reference)

**Architecture:** project-scoped state · `Action`/`dispatch` · overlay containers + per-window
store · modal form-state off `AppState` · per-tab run state · `.strata/` split + dirty tracking ·
workspace module split · **DataFusion 43→54 upgrade**.

**Design system:** **S28** (type/colour/control/icon/dot tokens + components, app-wide migration,
dead-CSS retired) · **S29** (overlay/menu family) · **S2** JSON theming.

**Features shipped:** launcher + multi-window + pinning + reopen-on-startup · permanent activity
rail · sidebar/catalog + filter + context menus + inline rename · SQL editor + **autocomplete (S7)**
+ **validator + squiggles + hover popover (S25/S26/S27)** · tabs (rename/context/overflow/show-all/
duplicate) · results grid + **unified status bar + pager (R1)** · plan view (thin, S12) ·
**cancellation + confirm-close (S14)** · error view · drawer (Problems/Events/History) · command
palette (core) · settings (Appearance/Data/System/Keymap, in-window) · **real export (C2)** ·
config/register-table (C3) · saved queries · native File/Edit/Window menu · themes.

---

## Rough order

1. **F3 spacing/radius tokens** app-wide — the v19 headline; cheap, high-impact.
2. **Restyle aligns** (built-but-wrong, cheap): U4 restyle done (**E4** Run buttons + **T4** tab-close; tab *features* **T1**/**T2** still open) · U6 ✅
   (collapsible-find + **Rz8** clear) · U1/U2/U12/U14 surface polish · U10. Makes the built app *feel* v19.
3. **Rebuilds of built surfaces:** U9 inspector metadata (grid → dynamic key/value box) ·
   U3 sidebar (collapsible sections + nested-column expand) · **D6** export modal (full UI rebuild, backend kept).
4. **Feature builds** (Part 2 — these *are* the "missing" surfaces): **W1** Settings window ·
   **W7** connections (lands the U2 rail button + U3 pane + U14 LOCATION) · **Rz2** chart ·
   **Rz3–Rz6** grid selection/copy/record/sort · **Rz-plan** plan v3 · **D4** profiling · **T3** palette depth.
5. **Functional polish:** E1 validator · E3 undo/redo · W2–W5 settings/theme · T2 OS-close.
