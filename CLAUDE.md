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

To build something you can hand to a tester — a universal `.app` + DMG in `target/dist/`:

```bash
./scripts/bundle-macos.sh              # universal; --arch arm64 for a quick local check
```

The same script is what the **Release** workflow runs (Actions → Release → Run workflow), which
can also bump the crate version, tag the commit and publish a release page in the same press.
`scripts/version.sh` owns the version number (read it, resolve a bump, write it + `Cargo.lock`).
Signing degrades honestly: ad-hoc today, notarized the moment the secrets exist. See
**`docs/RELEASING.md`**.

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
- **`strata-model`** — leaf data vocabulary, serde-only (schema, results, catalog, form, history,
  session, query_error). No logic. (The event log is *not* here: it is ephemeral app state —
  `strata-freya::apps::project::state::log`.)
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
                                 project folder / settings), `open_project` / `open_launcher`
                                 (focus-if-open), `close_this_window` (the launcher takes over
                                 when it was the last — settings doesn't count as a window you
                                 work in), and quit (closes every window + keeps the persisted
                                 open-set, so a restart reopens them)
src/platform/settings.rs         the Settings window's single instance + its **pin**: one app-wide,
                                 opened above whichever window asked (a native child window via
                                 the fork's `set_window_parent`), re-pointed when another window
                                 asks. Closing with the owner is ours, not AppKit's — the owner
                                 leaving the registry closes it through Freya's own path
src/platform/open.rs             **where** an open lands, vs windows.rs's *how*: `OpenCtx` (the
                                 window's project root + its pending This/New question) resolves
                                 `OpenPref` for every project-window surface — ⌘O, Open…, Open
                                 Recent, the switcher. `decide` is a pure, unit-tested rule (own
                                 project = no-op; already-windowed = focus, outranking the pref);
                                 acting is split off (`OpenTarget`) because the menubar handler
                                 has a RendererContext and no Platform
src/platform/export.rs           where an Export window goes: a native child of the project
                                 window that asked — and pointedly **not** single-instance.
                                 Settings shows app-wide state so a second ⌘, can only mean focus;
                                 an export window is opened *on a result* and carries that run's
                                 snapshot, so focusing an open one would show the wrong run. One
                                 per press of Download, each closing itself when its write lands
src/platform/configure.rs        where a Configure window goes: the same native child, keyed
                                 **one per target** — it is opened on a *def*, which is shared
                                 mutable state, so two windows on one def would both write it and
                                 the second would revert the first. Between the other two rules:
                                 Settings is app-wide (a second ask means focus), Export has no
                                 rule at all
src/platform/owner.rs            P4-16 — **how long a child window may live**: not as long as the
                                 window it sits above, but as long as the *mount* of `ProjectRoot`
                                 whose handles it borrowed. `Subtree` is that mount's own diff key
                                 (folder + engine generation) plus the live `EngineRestart` to read
                                 the generation back; `ProjectRoot` provides it, so no opener can
                                 assemble a mismatched trio, and `use_owner_pin` is the one
                                 predicate both Export and Configure close on. A re-root changes
                                 the folder, a restart changes neither it nor the window id — and
                                 an owner that has closed shows nothing, so it fails the same
                                 comparison rather than needing a clause
src/menu.rs                      the macOS menubar: App · **File** (Open… · Open Recent ·
                                 Close Project) · Edit. Not static — `app_menu` hands back
                                 `MenuHandles`, and the *focused* window keeps the File menu
                                 pointed at itself (`use_file_menu`): its recents, Close Project
                                 only when it has a project to close, and the `OpenCtx` Open
                                 Recent resolves through (the one item carrying data, not a chord).
                                 The **accelerators** follow the keymap live (P4-08,
                                 `sync_chords`), and are held off entirely while Settings ▸ Keymap
                                 is capturing a chord (`suspend_accelerators`) — the OS resolves an
                                 accelerator before the window sees the key, so an armed menubar
                                 would copy on ⌘C instead of letting it be bound
src/state/mod.rs                 `AppCtx` — the six app-globals `main` creates once (themes ·
                                 config · window registry · theme preview · menubar handles · the
                                 focused window's open path), handed to every window root as one
                                 value rather than six parameters
src/state/theme_preview.rs       the Settings window's **live theme preview** — the one half of
                                 its uncommitted draft every other window reads, so a pick
                                 repaints them all before it is saved. A second, higher-priority
                                 input to the same pure derivation; dropping it is the revert
src/state/config.rs              THE app-global store: one `RadioStation<AppConfig, ConfigChan>`
                                 (settings · recents · open-project set) created once in main and
                                 shared into every window (`use_share_config`). Channels keep a
                                 project open from waking theme readers; `write_config` is the only
                                 write path (mutate + notify + persist — nothing re-reads the file).
                                 `use_open_project` ties a window's project to recents + the
                                 open-set for its lifetime
src/theme.rs                     Freya theme application: `theme_registry!` / `strata_components!`
                                 macros, Pref→Preference coercion, ThemesCtx (the shared
                                 ThemeRegistry handle, discovered once in main; every window root
                                 *derives* its theme through it, but only the roots whose subtree
                                 reads the registry itself — project, settings — also `provide` it,
                                 so a new consumer must check its window does), schema-sync test.
                                 The theme data model + loader +
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
  form/                          **the form vocabulary** every settings-style surface is built
                                 from — export options, the config modal, the Settings panes —
                                 under one `form` component theme. mod.rs carries the theme, the
                                 shared metrics and a "known divergences" list (the canvases
                                 differ; the differences are named, not averaged)
                                 `Form` > `Row` > control, composed: a control is a `Row`'s
                                 *child*, so a row wraps a field, a Switch, a pill or a Note
                                 without knowing which. mod.rs is the `Form` (container + theme
                                 + the `Variant` it provides through context)
    row.rs                       `Row` — **one** row, not one per window. Its register comes from
                                 the form's `Variant`: `Fields` (eyebrow label + ⓘ tooltip + gaps
                                 — export, config modal) or `Preferences` (title + inline subtext
                                 + rules — the Settings panes). `.trailing()` puts the control at
                                 the row's end, `.on_press()` makes the label activate it,
                                 `.anchor()` names it so a `Reveal` can jump to it. Plus `Note`,
                                 a statement where a control would go
    reveal.rs                    P4-09 — **a row you can jump to**: `Reveal`, the one-anchor slot
                                 a surface asks through (window-lived: it is written before the
                                 page holding the row exists), and `RevealScroll`, the frame the
                                 row scrolls itself into (page-lived — whatever owns the
                                 `ScrollView` provides it). Both optional; a form with neither is
                                 a form of ordinary rows
    field.rs                     `ValueField` (the mono box: stated height, length cap enforced
                                 on the state, the caller's width on the *wrapper* so a relative
                                 one is a flex child of the row) + `NumberField` (bounded,
                                 `.unit("px")`, reports per keystroke and normalizes its text on
                                 blur) + `DirectoryField` (a path box + the native folder picker:
                                 one buffer, both write it — the picker sets the box and the box
                                 is what reports)
src/apps/launcher/               the launcher / welcome window (P4-02, `Launcher.dc.html`)
  mod.rs                         root + window config + the `launcher` component theme
  model.rs                       ProjectList: the filter + PINNED/RECENT split (unit-tested)
  views/                         title_bar · rail (SidebarRow) · projects · row · open (rfd pick)
src/apps/settings/               the settings window (P4-03, `Settings.dc.html`) — one app-wide,
                                 pinned above its opener. All five categories are built
  mod.rs                         root + window config + the `settings` component theme, the
                                 **freya-router** `Route` per category, and `SettingsCtx` (the
                                 draft · its **seed** · Apply · the live-theme mirror). Apply
                                 commits a per-field diff of draft-vs-seed
                                 (`Settings::merge_onto`), so a setting another window wrote
                                 meanwhile survives it; `dirty()` is the same comparison, which
                                 is why it reads no config state
  model.rs                       the nav tree: CATEGORIES + their groups + breadcrumbs
                                 (unit-tested — one category per route, groups contiguous)
  search.rs                      P4-09 — **the settings index**: what the nav's search box filters.
                                 One table generates the `Anchor` enum, every anchor, and each
                                 setting's route/label/subtext/keywords — and the panes build their
                                 rows from it (`Anchor::row()`), so a setting has one name and a
                                 typo in an anchor is a build error rather than a jump that lands
                                 nowhere. A category is never spelled out here (it resolves through
                                 `model`'s `category`), and the engine's properties are indexed off
                                 `ENGINE_KEYS` entire rather than a chosen few. Unit-tested
  views/                         chrome (the router layout) · title_bar · nav · pane · footer
                                 (the panes' rows are `components::form` — P4-05 moved the row
                                 vocabulary there rather than keeping a settings-only copy; P4-09
                                 gave `Pane` the scroll controller a revealed row reveals into) ·
                                 row_note (the full-width note *between* two table rows, shared
                                 by the window's two grids — one tone for wash, edge, glyph and
                                 text, so it reads as one statement)
    nav.rs                       the category rail — and P4-09's **search box** above it, which
                                 *replaces* the tree while it has a query (a hit can be a property
                                 on a page the tree only names). `follow` is the one place a jump
                                 happens, for the pressed row and for Enter, and it only ever
                                 navigates: a property with no override gets no row made for it
    theme.rs                     P4-04 — the Appearance pane: Sync-with-OS + the theme grid, both
                                 `Setting`s. Each card's thumbnail is painted from **that** theme's
                                 own sheet slots, so a user theme previews with nothing authored
                                 per theme; the tick follows `ThemeSel::effective`, not the stored
                                 id
    data_display.rs              P4-05 — the Data-display pane: row density · zebra · default
                                 column width · default row limit. All four already had their
                                 consumer (the grid reads three off the config store, the catalog's
                                 View-table action the fourth), so this is the control, not the
                                 wiring. Its bounds are `strata_core::config`'s COL_WIDTH_MIN/MAX,
                                 which the grid clamps to — a field offering a width the grid then
                                 corrects would be a field that lies
    system.rs                    P4-06 — the System pane: reopen-on-startup · default project
                                 directory · **Opening a project** · confirm-on-running-close ·
                                 query-history limit. All five already had their reader, so this
                                 is the control; the open-pref pill is the one worth naming — the
                                 This/New prompt's "Remember" was the only writer, and it is
                                 one-way, so nothing put the answer back to Ask. The history
                                 floor is `strata_core::config::HISTORY_MIN`, the same floor
                                 `history_cap` applies (a `0` would rotate `history.jsonl` away)
    engine/                      P4-07 — the Engine pane: the DataFusion properties editor, and the
                                 one category that is a *surface* rather than a list of settings
                                 (hence `Pane::filled` + `maybe_trailing`). mod.rs is the frame +
                                 toolbar + Revert; model.rs the row list (`PropRows` — rows are the
                                 editing model, the `BTreeMap` is what commits, ids are a counter
                                 because the *name* is the thing you retype; unit-tested);
                                 table.rs the grid on Freya's builtin `Table` (five fork additions
                                 rather than a lookalike — see the task file); inspector.rs the
                                 selected key's catalogue entry. Nothing here reaches an engine:
                                 Apply writes the config, and each project window picks it up
    keymap/                      P4-08 — the Keymap pane: every command and the chord it answers
                                 to, on the **same** builtin `Table` the engine grid uses (the
                                 canvas was redrawn from a card list into an Action/Shortcut grid
                                 after the last handoff). mod.rs is the frame + Reset all + the
                                 capture listener + `ask`, the **one** funnel every change goes
                                 through (`keymap::propose` then `apply`, so a reset is
                                 conflict-checked exactly like a capture); model.rs the row
                                 projection + `Editing` (one value, so listening and blocked
                                 cannot both be true; unit-tested); table.rs the grid, the key
                                 caps and the four states of the Shortcut column. The rebind
                                 policy itself is `strata_core::keymap`'s, beside the resolution a
                                 hand-edited config meets. While a row is listening the menubar's
                                 accelerators are **suspended** — see src/menu.rs
                                 `Strata.exportGroups()` for the options) — opened from the
                                 results toolbar, pinned above the project window that asked
  mod.rs                         root + window config + the `export` component theme, and
                                 `ExportCtx`: the draft, the launch values, and **the one write
                                 path** (`edit`, idempotent, so every control writes the same
                                 way). **It exports a result, not a tab** — the window carries
                                 that run's `ExportTarget` (snapshot handle · schema · rows ·
                                 the grid's sort · the page in hand) and **pins** the snapshot
                                 for its whole life, so a re-run in the tab behind can neither
                                 truncate a running `COPY` nor make the export report no results
  model.rs                       **options are data**: `ExportDraft::groups` returns a
                                 `Vec<Group>` the view renders blind, so a new option is a row
                                 in a table, not a branch in a component (D6's actual ask).
                                 Every option carries the `Edit` it performs, so a control
                                 cannot write the wrong field; the draft keeps every format's
                                 options side by side. Unit-tested without a renderer
  preview.rs                     what the chosen options will produce — P3-08's "only real
                                 facts" again: real rows from the page in hand, real types from
                                 the run's schema, no `estSize()` compression guesswork
  views/                         title_bar · formats (four cards, not five — the canvas dropped
                                 Clipboard once the grid grew its own copy controls) · options
                                 (one component per control *shape*, so the hook count per
                                 render is fixed) · partition (the Hive `key=value` transfer
                                 panes) · footer (the only thing here that writes: pick a
                                 destination, build the spec, call the engine, log both arms)
src/apps/project/                the project window (Valin-shaped)
  project.rs                     two layers: `ProjectApp` = the **window** (theme, app-globals,
                                 close bridge, menubar, OpenCtx) and `ProjectRoot` = the **open
                                 project** (engine, stores, autosave, catalog, views), whose
                                 `render_key` is the project folder — so "open in this window"
                                 is a `State` write and the remount *is* the reopen path
  contexts/engine_ctx.rs         EngineCtx = Arc<Engine>, provided via use_provide_context, built
                                 with the app's `datafusion.*` overrides — a launch value, since
                                 the RuntimeEnv is fixed when the SessionContext is
  query/                         the freya-query capabilities over the engine facade — run_query
                                 (RunQuery · FetchSnapshotPage), validate, profile (P3-09: the
                                 scan, keyed by `ProfileSpec { owner, scan }`, with `use_profile`
                                 the one place that Query is built)
  state/                         per-window state (Radio): channel, hooks, session
                                 engine_config.rs = P4-07's driver: `Engine::set_config` off
                                 `ConfigChan::Settings`, and `EngineRestart` — a runtime key can
                                 only be applied by a new engine, so the restart is a bump of
                                 ProjectRoot's diff key (the re-root mechanism), asked through the
                                 one T2 confirm (`CloseTarget::Restart`)
                                 diagnostics.rs = **the window's one validation driver**: every
                                 open tab's diagnostics kept in step with its text and the
                                 catalog. Each tab carries `validated: Option<Stamp>` (buffer
                                 revision + catalog epoch), so `stale_tabs` is the whole work
                                 list and no entry point needs enumerating. One hook, three
                                 fixed subscriptions (`Chan::Text` fan-in · `Chan::Tabs` · the
                                 catalog), one serial task, active tab first
                                 catalog.rs = `CatalogState { Scanning, Settled(epoch) }` — the
                                 scan claim *and* the validation gate. Seeded `Settled(0)`:
                                 settled so the project-open pass can claim it, epoch 0 so
                                 nothing validates before that pass completes
                                 session.rs = SessionState + stateful QueryTab (each tab owns its
                                 CodeEditorData, keyed on Chan::{Tabs, Tab(id)}); Layout too
                                 project.rs = ProjectState — **the catalog**: persisted defs +
                                 `Reg<T>` (Loading/Ready/Failed), per-section ProjChan channels,
                                 and each row's profile *request* (never its numbers)
                                 catalog.rs = CatalogSelection, the inspected column (context)
                                 log.rs = P3-13's **event log** satellite (`LogCtx`, ephemeral,
                                 capped): the record behind the drawer's Events tab. No producer
                                 hook — whichever layer observed the fact appends it (the scan
                                 pass, Save, the drop confirm, the keeper's settle, `cancel_run`)
  model/                         window-local view models
  views/
    dialogs/                     the window's modal dialogs, mounted early so their key barrier
                                 precedes every feature listener: close_confirm (T2) ·
                                 drop_confirm (P3-05) · open_prompt (the This/New question) ·
                                 profile_confirm (P3-10 — and `ProfileActions`, the one entry
                                 point every "profile this" trigger calls)
    header/
      mod.rs                     the header bar — and the window's macOS title bar: brand ·
                                 switcher · ⌘K/⌘, cluster, drag + double-press-to-fill
                                 (`window_drag`), traffic-light gutter
      project_menu.rs            the project switcher: trigger + Open… / open set / recents
                                 dropdown; every row opens through the window's `OpenCtx`
    sidebar/
      mod.rs                     sidebar shell — pane-specific header (the catalog's filter +
                                 refresh row) over the active pane
      catalog/                   P3-02: mod (pane + sections), section, entry (entry/column/
                                 saved-query rows), columns (flatten + tests), interaction (tests)
    inspector/                   P3-08/P3-09 — the selected column, and **only what was actually
                                 read or counted**: mod (frame + theme + the not-a-column
                                 states), model (resolve the ColRef path · the dynamic fact list ·
                                 completeness · `with_scan`, which folds a scan's facts into that
                                 same list — all unit-tested off a store), column (title ·
                                 nested-fields box · the STATISTICS zone's four states), tests
    drawer/
      mod.rs                     the bottom drawer frame + its `drawer` component theme: header
                                 (title · count · Clear · expand/restore · collapse) over the body
                                 the **rail's bottom group** chose — there is no in-drawer tab
                                 strip. The count is a `DrawerCount` (`State<usize>`) the shell
                                 owns and the mounted body resolves (the `running` mirror's
                                 pattern); Clear is Events/History-only and parked until P3-13/14
      frame.rs                   the frame the three bodies share (P3-11 → P3-12): `DrawerBody`
                                 (scroll container) + `DrawerEmpty` (centred glyph + one line)
      problems/                  P3-12 — **every** open tab's diagnostics, grouped by tab, rows
                                 pressable to switch to the owning tab. A pure view over
                                 `problem_groups()`; the header tally and the rail badge are both
                                 `error_count()`. Run failures are deliberately NOT here — a
                                 failure belongs to a run, and the results pane renders it in
                                 full
      events.rs                  P3-13 — the window's event log, newest first: flat dot · message
                                 · time rows over the shared frame. A view over `state::log`, and
                                 the tab that owns the drawer's first working **Clear**
      history.rs                 P3-14 — the project's past runs, newest first: pressable cards
                                 (figures · line-count pill · age over a two-line SQL preview)
                                 over the `state::history` satellite. Press loads into the active
                                 tab, double-press loads and runs — both through the editor's own
                                 `actions`, so a re-run is an ordinary press. Its **Clear**
                                 unwrites `history.jsonl` as well as emptying the satellite. No
                                 status dot: only successful data runs are ever recorded
    keeper.rs                    request keepers, mounted by ProjectRoot: one invisible
                                 query subscriber per open tab's current press, so a
                                 backgrounded run keeps its cache entry (lifetime =
                                 subscriber presence) and records history at settle time
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
- **`FREYA_THEME_SPEC.md`** — the native JSON theme format (sheet + palette + components + fonts).

Shipping:

- **`RELEASING.md`** — how a build reaches a tester: `scripts/bundle-macos.sh` (the whole
  pipeline — universal binary, `.app`, icon, signing, notarization, DMG) and the **Release**
  workflow that runs it on demand and can bump + tag + publish in the same press. Also
  `scripts/version.sh` (the one place the version number lives), the Claude-written release notes
  and their fallback, the Gatekeeper bypass testers need while builds are unsigned, and the secrets
  that switch notarization on.

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

**P3-12 (Problems drawer)** is ✅, and carries the phase's third standing rule: **diagnostics are
a reconciliation, not an event.** Every open tab's diagnostics are a pure function of two inputs —
its buffer revision and the catalog epoch — and each tab stamps the pair its rows describe, so
`SessionState::stale_tabs` is the whole work list and the window's **one** driver
(`state::diagnostics`) drains it. Never add a second producer and never enumerate entry points: a
tab restored at open, reopened, opened from a view or saved query, duplicated, edited, or left
behind by a pass a tab switch cancelled are all the same thing — the stamp does not match. The
catalog is a **gate** as well as an input (`Engine::register` deregisters before it re-infers, so
nothing validates mid-scan and no false "not found" is ever produced). Problems is the
**SQL-validation** surface across every tab; a run failure belongs to a run, not to the text, and
stays the results pane's.

**P3-13 (Events drawer)** is ✅, and is the standing rule above read backwards: **a log is
recorded by its observer.** An event can't be re-derived from anything — it describes something
already finished — so `state::log` has **no producer hook**; the scan pass records each def the
engine answered for, Save and the drop confirm record their mutations, the request keeper records
each run's settle, and `cancel_run` records a cancel (the `Err("cancelled")` settle lands
unsubscribed, because clearing the trigger unmounts the keeper in the same pass). It also builds
the store state-arch §8 only sketched — deleting the dead `strata-model::log` vocabulary — and
wires the drawer's first working **Clear**. An entry carries a level, not an origin; see its task
file.

**P3-14 (History drawer)** is ✅, and completes the drawer. Query history is the *persisted* log
next to P3-13's ephemeral one, so its **Clear** has to unwrite `.strata/history.jsonl`
(`strata_core::project::clear_history`) and not just empty the satellite — while keeping the
satellite's `seen` dedup guard, because the pin holding a cleared run is still mounted and would
otherwise re-record it. The rows are *actionable*, which is the one thing that shapes the surface:
a press loads into the active tab, a double-press loads and runs, both through the editor's own
`actions::load_sql` / `actions::press_query`, so a re-run from the drawer is an ordinary press with
its own nonce and cache entry. And there is deliberately **no status dot** — the satellite records
only successful data runs, so the canvas's ok/cancelled/failed mark would have exactly one value
(P3-08's "only real facts", applied to a glyph).

History is a list of **queries, not presses**: a re-run moves its entry to the top with the newest
figures, keyed by `util::collapse_sql` — the same function that renders the row's preview, so no
two rows can read identically and none a reader can tell apart are merged. **Dedupe runs before
the cap** at every layer, which is the point rather than a detail: one query pressed 150 times has
to occupy one slot of `max_history`, not all of them. The log on disk holds to the same rule
without giving up its cheap write — a new query is one `O_APPEND` line, and only a run that
*replaced* an entry rewrites the file (`project::save_history`), because an append can add a line
but not move one.

**P3-08 (column inspector)** is ✅, and carries the phase's other standing rule: **only real
facts.** Every number in the inspector was *read* from the source (footer statistics via
`ColumnInfo.stats`, the row count via `TableMeta.rows`) — never derived from the rows on screen,
which is what the Dioxus panel used to do. So the facts box is a dynamic list rather than a grid
of blanks, inexact footer values render `~value`, and the completeness bar needs a real exact null
count or it doesn't appear.

**P3-09 (profiling) + P3-10 (cost confirm)** are ✅, and land the scan tier in that same list
(matched on `StatKey`, so a fact can never appear twice; free wins a tie *unless* it is an inexact
bound). The scan is a **freya-query action keyed by its request**: the store row keeps
`Option<ScanId>` and the numbers live only in the cache entry that key names (AGENTS.md §2) — so
invalidating a profile is dropping the request, which `table_registered` does for the table *and*
for the views that read it. `Engine::profile` / `cancel_profile` own the engine side; a running
scan counts as work in flight for the window-close confirm and not for the per-tab probe. The
canvas's **distribution bars are deliberately not built** — the scan has no distribution data and
an honest histogram needs a second full pass (P3-09's file has the reasoning).

**P4-07 (Settings ▸ Engine)** is ✅, and is where Freya's builtin `Table` earned its first use.
The investigation it started with is the transferable part: the table gave the bordered box, the
shared column widths and the per-row rule, and the five things it could not do were all **fork
gaps rather than design limits** — `TableRow` had a `pub theme` field with no builder (so a row
could not carry a selection fill, nor decline the hover a selectable table doesn't want), only
`TableCell` had `on_press`, `TableCell` hardcoded `main_align(End)`, `Table`'s rect had no flex
content so a stated height could not reach a scrolling body, and one `divider_fill` painted both
the box and the row rules so a theme could never author them apart. Five small upstream additions,
not a hand-rolled grid; what a table has *no opinion* about (which row is selected, what goes
between two rows) stayed composed in the app. It also wired the setting for the first time — the
engine was being built with `Default::default()` — and settled how an engine config change lands:
`Engine::set_config` writes the `ConfigOptions` half live (a **removed** key going back to its
catalogue default, not skipped), and a changed `datafusion.runtime.*` is a **restart**, which is a
bump of `ProjectRoot`'s diff key through the one T2 confirm rather than a second way to configure
a live engine. See its task file; AGENTS.md §2 carries the rules.

**P4-08 (Settings ▸ Keymap)** is ✅, and completes the Settings window. It is the second use of
that same builtin `Table` — the canvas was **redrawn** between the last handoff and the task, from
a list of two-line cards into an Action/Shortcut grid with the description in a tooltip and a
double-click to rebind, so the local handoff bundle's `Settings.dc.html` is stale for this pane
(read the design project). Three things it settled. **One funnel**: capture, the per-row reset and
Reassign all go through `keymap::propose` then `keymap::apply` over a `Rebind`, with the policy in
**strata-core** beside the `validate_bind` a hand-edited config already meets — which is what makes
a *reset* conflict-checked like a capture (a command's default chord can have been taken while it
was away) and a *steal* two bindings rather than one write. **A menubar accelerator is state**: it
now follows the keymap live (`sync_chords`, the list a destructure so a new menu command can't
forget it), and — the load-bearing half — it is **suspended** for the life of a capture, because the
OS resolves an accelerator before the window sees the key, so an armed menubar makes ⌘C copy instead
of bind, and ⌘Z ⌘X ⌘C ⌘V ⌘A ⌘O ⌘Q ⌘, are most of what anyone reaches for. And **dashed borders are a
fork addition** (`BorderStyle::Dashed`, `Button::border_style`): torin fills the region between two
rounded rects and a fill cannot carry a pattern, so a dashed edge strokes the centreline instead —
worth the addition because the dash is the message, distinguishing an open slot from a bound
control. The one thing deliberately **not** built is a direct unbind control: the state is supported
end to end and reachable via Reassign, but the canvas has no such affordance and inventing one is
the designer's call. See its task file.

**P4-10 (export window)** is ✅ — rebuilt from the canvas rather than ported, because the Dioxus
modal had drifted from the design and reached its screen through hardcoded `match` arms per
format. Four things it settled. **Options are data**: `ExportDraft::groups` hands the view a
`Vec<Group>` and each option carries the `Edit` it performs, so adding one is a table row and no
control can write the wrong field. **A window opened on a result pins what it reads** — the
snapshot pin (AGENTS.md §2) exists because of this window: a re-run in the tab behind used to
retire the table mid-`COPY`. **NULL partition values are refused, not warned about**: DataFusion
54 misfiles them into a neighbouring value's directory, which is silent corruption, so
`partition_columns_have_no_nulls` reads the parquet footer and proceeds **only on an exact zero**
(schema nullability is useless here — every column reports nullable), which is also why
`snapshot_writer_props` sets `EnabledStatistics::Chunk` explicitly. And the form vocabulary it
grew — the labelled row, the mono value box, the bounded number — went to
`components::form` with P4-05 rather than staying the export's own.

**P4-09 (Settings search)** is ✅, and settles that **a setting's name has one home**. The index
(`apps/settings/search.rs`) is one table generating the `Anchor` enum, the list of every anchor, and
each setting's route / label / subtext / keywords — and the panes build their rows from it
(`Anchor::row()`), because the failure it rules out is invisible: an anchor spelled one way in the
index and another in the pane is a jump that navigates and then singles nothing out, and only a
person trying it would ever know. The category is never restated either — a hit resolves its page
through `model`'s `category`. Three more things it settled. The engine's properties are indexed off
**`ENGINE_KEYS` entire** rather than the canvas's hand-picked eleven, so a subset can't drift from
the catalogue. **Following a hit is navigation and never an edit** — the first cut added a pre-filled
row for a property with no override (the canvas's "search doubles as add a known property") and was
rejected: a named row with an empty value still projects into the draft, so merely following a result
left Apply live for an override nobody asked for. And a **revealed row belongs to the form, not to
this window**: `Row::anchor` names it, `components::form::reveal` carries the ask (a window-lived
slot, because it is written before the target's page has mounted, plus the page-lived scroll frame),
and the row scrolls itself in and flashes once — the app's first use of `freya::animation` and of
`ScrollController::scroll_to_item` outside the tab strip.

**P4-16 (child-window lifetimes)** is ✅, and settles what a child window is actually pinned to:
**not the window it sits above, but the mount of `ProjectRoot` whose handles it borrowed.** Export
and Configure carry that subtree's store, log, catalog and scan counter as launch values — all
`GenerationalBox`-backed — and both things that remount the subtree free them while leaving the
owner window open under the same id. A re-root changes the folder, which the Configure pin happened
to catch; an engine restart (P4-07) changes neither, and nothing caught that, so the next repaint
panicked on a reclaimed box and a Save wrote into a store nothing was left to serve. The fix is one
value and one predicate (`platform/owner.rs`): `Subtree` is the subtree's own diff key plus the live
`EngineRestart`, **provided by `ProjectRoot`** so no opener can assemble a mismatched trio, and
`use_owner_pin` replaces the two near-verbatim pins. Three things it settled. An owner that has
closed *shows nothing*, so it fails the same comparison — one predicate, not three clauses. The
generation is the one handle safe to hold across a remount, for exactly the reason it exists (owned
by `ProjectApp`, above the subtree). And `WindowKind` now carries **less**: `Configure`'s `project`
and `Export`'s `owner` were the old pins' inputs, so once the pin read its owner from the launch
value they were unread second copies of a fact that could go stale.

---

## Engine model

The engine (`strata_core::engine::Engine`) is a **direct-call async facade**: it owns a private
multi-thread Tokio runtime (DataFusion's operators need a Tokio context; query CPU never touches
the render thread), spawns each call onto it, and the caller awaits the `JoinHandle` — which is
executor-agnostic, so Freya's non-Tokio UI executor awaits engine methods like any async fn. No
UI-side runtime, no channels, no request ids. freya-query capabilities call the facade directly
(`engine.query(…)`, `engine.fetch_page(…)`); snapshot lifecycle (supersede / cancel / retire) is
the facade's own bookkeeping — see **`docs/SNAPSHOT_SPEC.md`**. Snapshots are **Arrow IPC**, not
parquet, so a result's type survives the round trip (parquet cannot write a union or a zero-field
struct at all); compressed they are the same size on disk. The export null-gate's exact counts come
from the write pass (`query::SnapshotStats`), not a footer. In Freya the handle is `EngineCtx`
(an `Arc<Engine>` + Deref) held in context — not stored in any god-object `AppState`. Managed DDL
policy: the editor runs `SELECT`/`EXPLAIN`/`SHOW`/`DESCRIBE` **only**. Views are Save's artifact,
never typed DDL — ⌘S / Save-as-view wraps the buffer's *plain query* in `CREATE OR REPLACE VIEW`
itself (`Engine::create_view`), so typed `CREATE`/`DROP VIEW` is blocked (validation points at
Save / the catalog), like `CREATE EXTERNAL TABLE` / CTAS / `INSERT` (use Table Config) and the
hard-blocked `CREATE DATABASE`/`SCHEMA`.

**The SQL function set is the live registry, not a list we keep.** `build_context` registers
`datafusion-functions-json`'s Postgres-style accessors (`json_get` / `->` / `->>`; **not** `?`,
which sqlparser reads as a placeholder before the crate's planner sees it — `json_contains` is the
spelling that works) over Utf8
columns holding JSON text, and that call is the whole integration: `engine::functions::snapshot`
walks `ctx.udfs()`, so anything registered reaches autocomplete, signature help and the docs panel
with no per-function table and no way for the completion pool and the engine to disagree. Adding a
UDF family means one `register_*` call in `build_context` and nothing else.
(`.claude/tasks/workstream-json-polymorphic/` — WJ-01, and WJ-02 for the union-tolerant JSON
reader that makes the accessors pay off.)

> The Dioxus-era `Command`/`Event` channel protocol + worker loop was **deleted from
> `strata-core`** with P2-01. `crates/strata-dioxus` still references it and therefore **no longer
> builds** — it is kept as *reference code only* for porting features to Freya. Don't try to fix
> its build.
