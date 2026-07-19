# Strata — Freya port plan

Migrating Strata's UI from **Dioxus (wry/webview)** to **Freya 0.4 (Skia/native)**. Decision reached after the
`freya-spike/` validation: cross-window shared state, the Skia results grid, and the SQL editor ↔ language-service
binding all proved out on the Mac. See memory `freya-migration` for the spike findings.

**Why:** kill the recurring webview/macOS-hack tax (cross-window state, close-decision objc intercept, ⌘A/⌘C swallowing
driving the keymap/menu complexity). Freya gives native rendering, native events, and genuine cross-window shared state.

**Reference architecture:** [`marc2332/valin`](https://github.com/marc2332/valin) — the Freya author's own IDE. We
follow its module/data-scoping conventions.

---

## 1. Guiding principles

1. **Ports & adapters.** Pure domain logic lives in framework-agnostic library crates. The UI framework is a swappable
   *adapter* in a thin binary. Dioxus and Freya are two adapters over the same core.
2. **Placement follows scope** (valin's rule). Cross-feature state → central `state/`; feature-local state → co-located
   with the feature. The narrowest scope that covers all consumers wins.
3. **Coexistence, then cutover.** The Freya app is a new binary crate alongside the Dioxus one, both on the shared core,
   so we can run them side by side for parity checks and only delete the Dioxus app when Freya reaches parity.
4. **Rebuild the design system in Freya's idiom**, guided by the `.dc.html` canvases + theme JSON — not a literal CSS
   port (CSS doesn't map to Skia attributes).

---

## 2. Workspace crate layout (module ≠ crate)

Crates are units of **reuse / compilation / enforcement**, not a mirror of the module tree. The only hard requirement is
that the **Freya binary reuse the framework-agnostic core without pulling Dioxus** — that needs library crate (s), *not*
one-crate-per-module. Splitting granularly would force every shared type into inter-crate ceremony for marginal benefit
on a two-frontend project. So: **two core crates**, with the module boundaries we built preserved *inside* them.

**Status: phase 0 ✅ complete — builds + runs on macOS.** Both frontends are now sibling member crates on the shared
core; the workspace root is a **virtual manifest** (no root package), so neither frontend is privileged.

```
parquet-visualiser/            (virtual workspace root — no root package)
├── Cargo.toml                 [workspace] members + [profile.*] + default-members = strata-dioxus
├── sample/  themes/  .strata/ runtime data — stays at root (cwd-relative, not crate-relative)
└── crates/
    ├── strata-model           the DATA vocabulary leaf. Pure serde types: schema/results/
    │                          catalog defs (CatalogTable/View/RegStatus) / CatalogProfile /
    │                          Diagnostic / forms / logs / errors. deps: serde only. No
    │                          datafusion, no UI. Everyone depends down.
    ├── strata-core            the framework-agnostic LOGIC, as modules: `engine` (worker +
    │                          Command/Event protocol + the connection handle; `sql`/`plan`/
    │                          `profile` nested under it) + `config` / `theme` / `util`.
    │                          deps: strata-model, datafusion, tokio, arboard, preferences.
    ├── strata-dioxus  (bin)   the current Dioxus app — transitional, on the shared core.
    │                          Package name kept `strata` (binary + `dx` unchanged; Dioxus.toml
    │                          + assets/ live here). Deleted at cutover.
    ├── strata-freya   (bin)   the Freya app — target (coming in phase 1). Its own member,
    │                          excluded from `default-members` so Skia stays out of the
    │                          default `cargo build`/`run` (build it with `-p strata-freya`).
    ├── strata-forms           headless form layer (already renderer-agnostic — keep)
    └── strata-forms-macro
```

**Data vs logic:** `strata-model` holds the shared *data types* (serde-derivable, no heavy deps); `strata-core` holds
the *logic* over them (datafusion/tokio). Both binaries depend on model + core. The one real untangling: **lift the data
types out of the Dioxus store modules**
— `CatalogTable`/`CatalogView`/`Diagnostic`/`Project` currently sit interleaved with their
`GlobalStore`/mutators in `project.rs`/`diagnostics.rs`; the *type* moves to `strata-model`, the *store* stays app-side
(re-export shims keep `crate::project::CatalogTable` etc. resolving so call sites don't churn).

**Coupling check (measured):** `engine/` (except `mod.rs`), `sql/`, `serialize`, `config`,
`profile`, `util` are already pure (no `dioxus` import). `engine/mod.rs` *looks* coupled but only its
`#[derive(Store)]`/`GlobalStore` wrapper is — the `Engine` **handle struct itself**
(`cmd_tx`/`evt_rx`/`next_req` + `spawn`/`send`/`take_evt_rx`) is plain tokio channels + an atomic, so it **moves to
`strata-core` too**. The app keeps only a ~30-line *reactive-storage shim*: the `GlobalStore`/`GlobalSignal` that holds
the core handle, the `command!` macro, and the accessors delegating to it (the Freya app holds the same handle via
context/Radio). The
`functions` reactive field becomes frontend state fed by `Event::Functions`, leaving the core handle pure I/O. `ui/` (~
11K lines) + the stores + `main.css` (~5K) is the rewrite.

**Why two, not one:** `strata-model` is a genuinely zero-heavy-dep leaf (no datafusion), so a consumer wanting only the
vocabulary (tests, a CLI) gets it without pulling datafusion.
`strata-core` is where datafusion/tokio live. That's the one boundary worth a crate line.

---

## 3. `strata-freya` internal module layout (valin-style)

```
strata-freya/src/
├── main.rs            entry: build+enter a Tokio runtime (guard held for the program),
│                      create_global singletons, launch the initial window
├── theme.rs           Freya theme from theme JSON (ColorsSheet + define_theme! registry)
├── state/             GLOBAL cross-window singletons ONLY — settings, theme, recents
│                      (`create_global`). Per-window model state lives with its app (below).
├── engine/            engine bridge: worker-handle type + the freya-query Query/Mutation
│                      capabilities (shared defs; the handle is per-window, in context)
├── components/        generic cross-window DS widgets (buttons, inputs, overlay, dots, …)
├── apps/              one self-contained folder per OS window (a Freya `App` root)
│   ├── project/       the main window
│   │   ├── project.rs   root shell (rail · sidebar · workbench · drawer); composes views/
│   │   ├── state/       per-window Radio station: project defs / session-tabs /
│   │   │                per-tab view-state / layout / inspector selection / logs + channels
│   │   ├── views/       workbench/ (editor/ · grid/) · sidebar/ · inspector/ · drawer/ ·
│   │   │                command_palette/
│   │   └── commands.rs  palette command registry (trait-object, valin-style)
│   ├── launcher/
│   ├── settings/      root + views/ (appearance · data · system · keymap)
│   ├── export/        root + views/ (format · options · hive · preview)
│   ├── configure/     register / edit table
│   └── connections/
└── platform/          window spawn/close, native menu, keymap/hotkeys
```

**Each window is a self-contained "app."** Freya calls a window root an `App`
(`WindowConfig::new_app`), so `apps/<window>/` groups each window's root + its own `views/`

+ per-window `state/`. Every window owns its views symmetrically (no project-window special case). Only genuinely global
  state (`create_global` singletons), shared DS widgets, and the engine capability defs sit at the top level. (Call it
  `windows/` if you prefer the literal term.)

**Conventions (valin):** `*_ui.rs` (render), `*_state.rs` (feature-local state), `mod.rs`
(wiring, private submodules + re-export). Feature-local state co-located; app-wide model in the app's `state/`; truly
global in the top-level `state/`.

---

## 4. State architecture

A **client-state / server-state** split (the React-Query idea), each with the right tool (see memory `freya-migration`
for the reasoning):

| Concern                                                                                                                                   | Mechanism                    | Notes                                                                                                                                                                                                                    |
|-------------------------------------------------------------------------------------------------------------------------------------------|------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| **Durable client model** — project defs, session/tabs, layout, inspector selection, per-tab view state (sort/search/selection/page), logs | **per-window Radio station** | Channels per slice (`Tables`, `Views`, `History`, `ActiveTab`, `Panels`, …) = the dioxus-stores lens reactivity, made explicit. `derive_channel` for cross-slice reactions. Serializes as one struct for `session.json`. |
| **Server data** — query results/pages, profiles, explain, catalog-from-engine                                                             | **freya-query**              | Cache, dedup, invalidation, loading/error states — for free. Ephemeral (not serialized).                                                                                                                                 |
| App-wide singletons (settings, theme, recents)                                                                                            | **`State::create_global`**   | Shared across all windows, passed into each window root. Theme also uses `use_init_theme`.                                                                                                                               |
| Engine handle (`cmd_tx`)                                                                                                                  | **root context**             | Consumed via `consume_context` inside each query/mutation `run`.                                                                                                                                                         |
| Feature-local UI state (sidebar filter, dialog drafts, hover)                                                                             | **`use_state`**, co-located  | Never promoted to the station.                                                                                                                                                                                           |

**Engine integration (freya-query over the worker).** The engine stays a dedicated DataFusion thread. UI-side **reads**
are `QueryCapability`s (`fetch page(sql, page)`,
`profile(table)`, `explain(sql)`, `catalog`), **writes** are `MutationCapability`s (`run`, `register`, `drop`,
`refresh`); each `run` does `consume_context` for the engine handle and bridges to the worker via a `oneshot` reply. A
mutation's `on_settled`
invalidates the affected queries → they refetch. This **replaces** hand-rolled machinery:
pagination caching, the whole profile subsystem (`CatalogTable.profile` cache + duplicate-request dedup + spinner), and
the D10/D11 `invalidate_views_using` dance (now
`on_settled` invalidation).

*Tokio bridge:* Freya's runtime isn't Tokio, so `main` builds a Tokio runtime and holds
`let _rt = rt.enter();` for the program (the `oneshot` reply + any tokio-dependent code need it). Engine→UI **push**
that isn't request/reply (profile progress, a catalog-changed signal) uses a Tokio `watch` channel + `use_track_watcher`
(sdk feature). Never
`tokio::spawn` anything that updates UI state — Freya reactivity is single-threaded; use Freya's `spawn()` /
`use_future`.

**Write side (client model).** Strata's `Action`/`dispatch` funnel ports to Radio: direct
`radio.write()` mutator fns (mirrors today's store mutators + read-accessor convention) or a `DataReducer` if we want
the enum-dispatch shape verbatim. Palette **commands** are a separate trait-object registry (valin's `EditorCommand`),
distinct from state mutation.

**Per-window vs global.** Each project window is a *different* project, so the Radio station is **per-window** (init'd
in each project window's root). `create_global` is reserved for the app-wide singletons. (The spike used `create_global`
only because its demo state *was* global.)

---

## 5. Design system — theme Freya's components; hand-roll only the bespoke

Freya 0.4 ships a rich, fully-themeable component set (every component has a
`*ThemePreference` registered on the `Theme`). **Default to reusing + theming** it; build custom only where the
design/behaviour genuinely diverges. This *shrinks* the DS phase vs a full rebuild.

- **Colors + tokens:** one `ColorsSheet` from the existing grouped theme JSON (JSON stays the source of truth);
  Midnight/Daylight → two `Theme`s; spacing/radius/type tokens → Rust consts / theme fields. Matching the `.dc.html`
  look becomes theme config, not rebuilds.

- **Reuse + theme (most of the DS):**

  | Strata DS | Freya built-in |
    |---|---|
  | Button, IconButton | `Button` |
  | Segment | `SegmentedButton` |
  | TextInput / NumberStepper | `Input` (+ `InputValidator`) |
  | Select · DropdownMenu / ContextMenu | `Select` · `Menu` / `ContextMenu` |
  | Toggle · Checkbox | `Switch` · `Checkbox` |
  | Popup / Dialog / Window | `Popup` family + `Portal` |
  | Tooltip | `Tooltip` |
  | resizable panels (sidebar/inspector/editor/drawer) | `ResizableContainer`/`Panel`/`Handle` |
  | tabs + drag-reorder + overflow | `FloatingTab` + `DragZone`/`Draggable` + `OverflowedContent` |
  | collapsible catalog sections | `Accordion` |
  | catalog rows | `SideBarItem` |
  | partition chips | `Chip` |
  | profiling spinner | `CircularLoader` · `ProgressBar` · `Skeleton` |
  | scroll areas | `ScrollView` / `VirtualScrollView` |
  | traffic-light chrome | `TitlebarButton` |
  | icons | `SvgViewer` (+ bring the existing SVGs) |

  Notably: **resizable panels** and **tab drag/overflow** come free — both significant hand-rolled subsystems today.

- **Hand-roll (Freya has no fit / behaviour too specific):** the results **grid**
  (cell/row/col selection, drag-paint, type-coloured cells, column resize — built on
  `VirtualScrollView` + `rect`, as the spike does); the **code editor**; **status dots**; the **typography** wrappers
  (Caption/Body/Meta/… → thin `label()` presets over
  `TypographyTheme`); any pixel-exact holdouts.

- **Open eval:** whether Freya's `Input` + `InputValidator` obviate `strata-forms` on the UI side (keep the headless
  validation crate only if it still earns its place).

Preview themed components with `freya::components::gallery()` while dialling in the theme.

---

## 6. Phased sequencing (parity-checkable throughout)

**Phase 0 — Extract the core (in the Dioxus repo).** Pull `model`/`sql`/`engine`
(worker+protocol+serialize)/`persist`/`profile`/`util` into the `crates/` libs above. Re-point the Dioxus app at them;
confirm it still builds + runs on the Mac. *This is a low-risk refactor and the highest-leverage first step — both
frontends now share the core.*

**Phase 1 — Freya skeleton + engine round-trip.** `strata-freya` binary: window shell, the Radio station scaffold, the
engine bridge (spawn worker, drain events → state), and a minimal "type SQL → run → see rows" loop reusing the spike's
editor + grid. Proves the core is reusable and the state backbone works end to end.

**Phase 2 — Workbench.** Editor (CodeEditor + sql-service binding, incl. completions + diagnostics), results grid
(selection/sort/paging), tabs, run/explain, toolbar, status bar. The core UX.

**Phase 3 — Catalog + inspector + drawer.** Sidebar/catalog (collapsible, nested columns), column inspector + profiling,
bottom drawer (problems/events/history).

**Phase 4 — Multi-window.** Settings window, launcher, **export window** — all via shared state (`create_global` for
settings; export now just *reads the run*, no seed handoff). Native close handling (winit `CloseRequested`, no objc).

**Phase 5 — Design polish.** Apply the rebuilt design system across every surface; theming, spacing tokens, hover/focus
states, animations.

**Phase 6 — Platform + parity.** Keymap/hotkeys (native events — much of the Dioxus complexity evaporates), command
palette, native menu (decision: `madsmtm/menubar` vs in-app), overlays. Run both apps side by side; close parity gaps;
**then delete the Dioxus app.**

---

## 7. Survives vs rewrite (inventory)

**Survives (moves to core crates, ~as-is):** DataFusion engine worker + `Command`/`Event`
protocol; `model` data types; `sql` language service; `serialize`; config/`.strata/`
persistence; `strata-forms`; the keymap *command table* (data).

**Rewrite (Freya):** all `ui/`; the stores (dioxus-stores → Radio); overlay family; window management;
keymap/hotkeys/menu *delivery*; the editor integration (→ Freya `CodeEditor`);
`main.css` → Freya themes.

---

## 8. Known non-blockers / open items

- **Native macOS menu bar** — Freya has tray menus only. Decide `madsmtm/menubar` vs an in-app menu. Native key events
  remove much of the *reason* the menu existed (⌘A/⌘C swallowing).
- **Inline diagnostic squiggles** — `CodeEditor` exposes no decoration hook; needs the lower-level `use_editable` +
  `paragraph` path (span `.highlights()`), same as the vendored Dioxus editor. Everything else (text, cursor,
  completions overlay) is reactive off
  `CodeEditorData` today.
- **Grid at scale** — selection, column resize, virtualization tuning for large results.
- **Freya maturity** — pre-1.0, one primary maintainer, partial a11y. Accepted going in.

---

## 9. Freya 0.4 gotchas (from the spikes — save a build round-trip)

- UI helpers must return **`Element`**, not `-> impl IntoElement` (opaque return hides builder methods +
  `Into<Element>`). Convert with `.into()` *inside* the fn.
- `VirtualScrollView` needs **`.item_size()`** + an explicit height or it renders zero rows.
- Flex children need the parent to opt in with **`.content(Content::flex())`**.
- `CodeEditorData::parse()` must run **before** `CodeEditor` renders or it panics (blanks the whole window).
- Reading a `State` right after a child writes it can be **one edit stale** — compute derived values in a **
  `use_side_effect`** (post-commit) into a separate `State`.
- **Freya's runtime isn't Tokio.** `#[tokio::main]` is discouraged (breaks the event loop). Build a Tokio runtime in
  `main` and hold `let _rt = rt.enter();` for the program so
  `tokio::sync`/`tokio::time`/ecosystem crates (and the engine `oneshot` bridge) work.
  `tokio::spawn` can't update UI state (single-threaded reactivity) — use Freya's
  `spawn()`/`use_future`, or a `watch` channel + `use_track_watcher` for cross-thread push.
