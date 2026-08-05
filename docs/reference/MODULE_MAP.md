# strata-freya module map

The annotated tree of the Freya app: what each module is and, where it matters, why it is
shaped that way. Read this when locating code or deciding where something new belongs.
Companion to [CLAUDE.md](../../CLAUDE.md) (workspace layout) and
[AGENTS.md](../../AGENTS.md) (the rules the shapes follow).

```
src/main.rs                      Freya launch + startup routing (reopen every project that had a
                                 window at the last quit, else the launcher; a folder argument
                                 wins); discovers ThemesCtx + creates the two app-globals — the
                                 reactive AppConfig store and the live window registry. Each
                                 window's theme is pure derived state (`use_strata_theme`).
                                 Also the **`strata mcp <project>` branch** (AA-05): `cli()` is a
                                 pure parser taken first, ahead of everything app-global, and
                                 `headless()` points logging at stderr (stdout is the MCP
                                 transport's) and hands the resolved root to
                                 `strata_agent::serve_stdio`
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
                                 the generation back; `ProjectLoaded` provides it, so no opener can
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
src/state/mod.rs                 `AppCtx` — the seven app-globals `main` creates once (themes ·
                                 config · window registry · theme preview · menubar handles · the
                                 focused window's open path · agent access), handed to every
                                 window root as one value rather than seven parameters
src/agent/                       AA-03 / AA-03b — agent access, the half that outlives any one
                                 window. `AgentCtx` is the pair `main` creates: the **directory**
                                 (lives for the process; windows join and leave it) and the
                                 **server slot** (what is listening now, or nothing — dropping it
                                 *is* stop). The window's half lives with the window, because that
                                 is what it is made of (`apps/project/state/agent.rs` beside the
                                 diagnostics driver, `state/agents.rs` beside the log satellite,
                                 `views/sidebar/agents/` beside the catalog pane)
  directory.rs                   the cross-thread service registry **and** the app's `Host` impl:
                                 each mount of `ProjectRoot` lends its `Arc<Engine>` (the data
                                 plane) and two senders (asks, bounded and answered; notices,
                                 unbounded and one-way). Keyed by a minted `RegId`, not by the
                                 project root: a restart remounts at the *same* root. **AA-03b
                                 dispatches an agent's run here**, straight at the engine on the
                                 query session's own `WsId` — so no tab is opened, nothing steals
                                 focus and the diagnostics driver has nothing extra to validate;
                                 the window only brackets it (ownership check + record, then the
                                 outcome)
  ask.rs                         `AgentAsk` — one variant per `Host` method that touches window
                                 state, each carrying its own reply channel — plus `AgentNotice`,
                                 the facts that carry none. Two channels because one producer
                                 cannot wait: a connection ending is sent from a `Drop`, with
                                 nothing to await on. `RunOutcome` lives here too, since it is
                                 what a notice carries and what the satellite stores
  server.rs                      `use_agent_server`: start / stop / restart off the whole
                                 `agent_access` setting, mounted by the two **workspace** windows
                                 (there is always one alive) and idempotent, the theme
                                 derivation's shape. Mints the token on first use and persists it
  status.rs                      the header's dot — the app's one *polled* fact, and the module
                                 doc says why: the count is rmcp's, created below our own seam
src/state/theme_preview.rs       the Settings window's **live theme preview** — the one half of
                                 its uncommitted draft every other window reads, so a pick
                                 repaints them all before it is saved. A second, higher-priority
                                 input to the same pure derivation; dropping it is the revert
src/state/config.rs              THE app-global store: one `RadioStation<AppConfig, ConfigChan>`
                                 (settings · recents · open-project set) created once in main and
                                 shared into every window (`use_share_config`). Channels keep a
                                 project open from waking theme readers; `write_config` is the only
                                 write path (mutate + notify + persist — nothing re-reads the file).
                                 `use_claim_open` ties a window to the open-set for its lifetime;
                                 `use_promote_recent` is the half a project earns by loading
src/task.rs                      `offload` — **the** way blocking work leaves the render thread.
                                 Freya is one event loop drawing every window and `spawn` polls on
                                 it, so an `async` block around a `std::fs` read moves nothing and
                                 a quiet network mount freezes the app. A thread per call (a pool
                                 would let one wedged mount hold up the next open), and cancelling
                                 means dropping the answer — a blocking syscall cannot be stopped
src/theme/                       Freya theme application. `mod.rs`: RoleColors + `use_roles()`,
                                 StrataPalette + `bridge_sheet` (feeds the fork's 27-slot
                                 ColorsSheet from the roles), syntax registration, ThemesCtx (the
                                 shared ThemeRegistry handle, discovered once in main; every
                                 window root *derives* its theme through it, but only the roots
                                 whose subtree reads the registry itself — project, settings —
                                 also `provide` it, so a new consumer must check its window
                                 does), schema-sync test. `components.rs`: the static mapping
                                 table — every component field fixed onto a role; built-ins as
                                 partial retunes, Strata components whole-cloth. The Role
                                 vocabulary + data model + loader + ThemeRegistry + schema gen
                                 live in `strata-core::theme`; the theme files in root `themes/`
src/components/                  shared component library
  divider, dot, icon, run_button, segmented_toggle, toggle_button, typography
  badge.rs                       tinted label pill (PART · HOTSPOT · ANALYZE · dtype).
                                 NOT Freya's `Chip` — that's a selectable, focusable control
  keycap.rs                      a **key cap** (`"keycap"` token group): Settings ▸ Keymap's
                                 chords and the palette's shortcut hints have to look like the
                                 same kind of object, and the colours were the `settings` theme's
                                 own until P6-01. Two *named* shapes rather than an average —
                                 `key` (raised, heavier bottom edge: the row is about the chord)
                                 and `chip` (flat: the chord is a hint). One cap per call; a
                                 chord is composed by the caller
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
                                 one is a flex child of the row, `.masked()` for a secret —
                                 `Input`'s own `InputMode`, so the state keeps the real value and
                                 revealing is a prop flip) + `NumberField` (bounded,
                                 `.unit("px")`, reports per keystroke and normalizes its text on
                                 blur) + `DirectoryField` (a path box + the native folder picker:
                                 one buffer, both write it — the picker sets the box and the box
                                 is what reports)
src/apps/launcher/               the launcher / welcome window (P4-02, `Launcher.dc.html`)
  mod.rs                         root + window config + the `launcher` component theme
  model.rs                       ProjectList: the filter + PINNED/RECENT split (unit-tested)
  views/                         title_bar · rail (SidebarRow) · projects · row · open (rfd pick)
src/apps/settings/               the settings window (P4-03, `Settings.dc.html`) — one app-wide,
                                 pinned above its opener. All six categories are built
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
                                 own authored roles (`authored_role`), so a user theme previews
                                 with nothing authored per theme; the tick follows
                                 `ThemeSel::effective`, not the stored id
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
    agent_access.rs              AA-04 — the Agent-access pane: enable · port · token, the control
                                 for the MCP server AA-03 ships dark. Three rows and no more: a
                                 client-setup line (one client's incantation; the README's job)
                                 and a live server status (the header's dot already says it) were
                                 both sketched and both descoped. Applying is all the wiring there
                                 is — every workspace window's `agent::use_agent_server` reconciles
                                 the server off `ConfigChan::Settings`. Regenerate edits the
                                 **draft** rather than committing at once: `agent_access` is one
                                 merge field, so a token written behind the draft's back would be
                                 overwritten by the next Apply — and Cancel is the undo a
                                 credential wants
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
src/apps/configure/              the Configure-table window (P4-11 — `Configure.dc.html`; P4-12
                                 folded in, because the format dropdown is what selects the
                                 import-option set and both halves reach the engine through one
                                 `TableSpec`). Register a new table or edit an existing def:
                                 mod.rs (root · window config · `ConfigureCtx` · `Status`),
                                 model.rs (the draft + its option groups), views/ (title_bar ·
                                 identity · paths · options · hive · status · footer). It is a
                                 child of the project window that asked, so it writes that
                                 window's store through the shared `persisted_defs` funnel and
                                 asks *its* one scan driver for the registration pass — rather
                                 than holding an engine or a second "make the engine match the
                                 defs" of its own
src/apps/project/                the project window (Valin-shaped)
  project.rs                     two layers: `ProjectApp` = the **window** (theme, app-globals,
                                 close bridge, menubar, OpenCtx) and `ProjectRoot` = the **open
                                 project**, whose `render_key` is the project folder — so "open
                                 in this window" is a `State` write and the remount *is* the
                                 reopen path. `ProjectRoot` runs the fallible load (defs +
                                 session) once per mount — **off the render thread**, driven by
                                 `use_future` — and is one of three arms: `ProjectLoading` while
                                 the read is out, then `ProjectLoaded` (engine, stores, autosave,
                                 catalog, views — built from the loaded values) or
                                 `ProjectLoadFailed` (P4-01: the fault dialog that closes the
                                 window). It also claims the open-set for every arm alike
                                 (`use_claim_open`), since all three are a window on that project;
                                 only the loaded arm promotes the recent. `window_geometry` lives
                                 here too: a window's size and position can only be set as it is
                                 created, so they are a launch input the *caller* resolves —
                                 offloaded, and given up on after 250ms, because that read is on
                                 the same folder and used to be what froze the app first
  commands.rs                    P6-01 — the **command registry** the palette's ACTIONS group
                                 offers: nine methods, each carrying its own metadata, over
                                 `strata-command-macro`'s `#[command_router]`. rmcp's declaration
                                 shape (id from the method name, subtext from the doc comment)
                                 without its `HashMap<name, Arc<dyn Fn>>` — the macro generates
                                 the `Action` enum, so every variant came from a body and there
                                 is no unrunnable command to test for. **Every body is one call
                                 into a funnel that already exists** (`actions::run_query`,
                                 `close::close_project`, the catalog's `view_row`…); a palette row
                                 is a second way to ask, never a second implementation. `key` is
                                 the chord a command *also* answers to, used for the row's hint
                                 and never to run it — synthesizing it (menu.rs's trick, right
                                 there because a muda handler has no stores) would make an
                                 unbound command unreachable from the surface that exists so you
                                 needn't know the chord
  contexts/engine_ctx.rs         EngineCtx = Arc<Engine>, provided via use_provide_context, built
                                 with the app's `datafusion.*` overrides — a launch value, since
                                 the RuntimeEnv is fixed when the SessionContext is
  query/                         the freya-query capabilities over the engine facade — run_query
                                 (RunQuery · FetchSnapshotPage), validate, profile (P3-09: the
                                 scan, keyed by `ProfileSpec { owner, scan }`, with `use_profile`
                                 the one place that Query is built), chart (Rz2: `FetchChart`
                                 keyed by `ChartSpec { snapshot, query, display }` — the display
                                 config is in the key because axis labels render through it)
  state/                         per-window state (Radio): channel, hooks, session
                                 agent.rs = the **window driver** (AA-03, re-pointed by AA-03b):
                                 one serial loop over both channels. It never waits for a query —
                                 the run is the directory's, on the engine — so all it does is
                                 check the agent holds the session, record what ran, and record
                                 what it came to. Holds the pure projections it answers with too
                                 (catalog from the store, never introspection), unit-tested with
                                 no renderer
                                 agents.rs = AA-03b's satellite: per connected agent, its query
                                 sessions and each one's run trail. Ephemeral and capped both
                                 ways — **never** `SessionState` (so nothing reaches
                                 `session.json`) and **never** history (which stays the user's).
                                 Unit-tested with no renderer
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
                                 and each row's profile *request* (never its numbers). W7's
                                 connections are a fourth section here, keyed by bucket on
                                 `ProjChan::Connections` — a `ConnRow`'s `Reg<()>` carries no
                                 payload because connecting *registers* an object store rather
                                 than inferring anything, so the three states are the whole value
                                 catalog.rs = CatalogSelection, the inspected column (context)
                                 log.rs = P3-13's **event log** satellite (`LogCtx`, ephemeral,
                                 capped): the record behind the drawer's Events tab. No producer
                                 hook — whichever layer observed the fact appends it (the scan
                                 pass, Save, the drop confirm, the keeper's settle, `cancel_run`)
                                 persist.rs = P4-15's **write funnel**: the one place a `.strata`
                                 write failure is reported. `persisted(log, ProjectFile, write)`
                                 → an event row + a `bool` the caller uses to decline claiming a
                                 success; `ProjectFile` is the only copy of each file's wording.
                                 Here rather than beside a caller because every writer that was
                                 added *away* from the old home grew its own silent
                                 `tracing::error!` instead of finding it
  model/                         window-local view models
  views/
    dialogs/                     the window's modal dialogs, mounted early so their key barrier
                                 precedes every feature listener: close_confirm (T2) ·
                                 drop_confirm (P3-05) · open_prompt (the This/New question) ·
                                 profile_confirm (P3-10 — and `ProfileActions`, the one entry
                                 point every "profile this" trigger calls) · load_failed
                                 (P4-01 — not a barrier over features but the whole fault arm:
                                 what `ProjectRoot` *is* when the project could not load. Try
                                 again re-runs the load via a generation bump; Close window
                                 goes through `close_this_window`; non-modal, so ⌘O and ⌘,
                                 keep working)
    loading.rs                     the load's **third** arm: what `ProjectRoot` is while the read
                                 is off on its own thread. Silent for the first 600ms — a spinner
                                 that flashes on every open is worse than none — then a loader,
                                 "Opening '<name>'" and **Close window**, which is the honest
                                 wording because a blocking syscall cannot be cancelled. No
                                 engine, no store, no `Subtree`. Shares the fault arm's
                                 once-only close + confirm-slot drain (`use_engineless_close`)
    header/
      mod.rs                     the header bar — and the window's macOS title bar: brand ·
                                 switcher · ⌘K/⌘, cluster, drag + double-press-to-fill
                                 (`window_drag`), traffic-light gutter
      project_menu.rs            the project switcher: trigger + Open… / open set / recents
                                 dropdown; every row opens through the window's `OpenCtx`
    sidebar/
      mod.rs                     sidebar shell — pane-specific header (the catalog's filter +
                                 refresh row) over the active pane
      agents/                    AA-03b — what each connected agent is doing: mod (pane + theme +
                                 the header's ⓘ, agent group over session group), run (the run
                                 card). Built to the canvas out of vocabulary the app already
                                 has (Freya's `TreeItem` with our own chevrons, the History
                                 drawer's card). **Only connected agents appear**, so no row
                                 wears a connected mark. A press opens a run's SQL in a **new**
                                 tab (`actions::open_sql`) — never the active one, which is the
                                 harm the pane exists to prevent — and there is no double-press
                                 to run. That press is the *only* way an agent's work reaches the
                                 tab strip
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
      problems/                  P3-12 + P4-15 — **two scopes under one header strip**, the
                                 IntelliJ arrangement: the drawer's title bar carries the tabs,
                                 each with its own count, so this is the one drawer tab whose
                                 header shows no separate tally. mod.rs = the strip (`ScopeStrip`,
                                 mounted by the drawer *header*) + the body that dispatches on
                                 `Layout::problems_tab`, which rides the session file like every
                                 other panel decision. queries.rs = P3-12's every-open-tab
                                 diagnostics, grouped by tab, rows pressable to switch to the
                                 owning tab — a pure view over `problem_groups()`. project.rs =
                                 P4-15's conditions about the *project*: defs the engine refused
                                 (re-derived from `Reg::Failed`) and `.strata` files a failed
                                 write left behind (`PersistFaults`, which cannot be re-derived —
                                 hence a **remembered condition**, the third kind of state beside
                                 a reconciliation and an event). The rail badge totals both scopes
                                 (`error_count()` + `project_error_count()`). Run failures are
                                 deliberately NOT here — a failure belongs to a run, and the
                                 results pane renders it in full
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
    palette/                     P6-01 — the **command palette** (⌘K), the window's one discovery
                                 surface. mod.rs is the `command_palette` theme, the
                                 always-mounted node (drawing nothing but its ⌘K listener) and
                                 the overlay card; model.rs the index — five groups in fixed
                                 order, all-words matching, a per-group cap, and COLUMNS hidden
                                 until you type (unit-tested off a store built inline); row.rs
                                 the 42px row and its heading. Its **keyboard lives in
                                 `Input::on_pre_key_down`**: the field consumes and
                                 `prevent_default`s every key, which cancels the derived
                                 `GlobalKeyDown` — that is what makes the palette a real modal
                                 barrier, and why ↑↓ / ↵ / Esc / ⌘K are handled before the field
                                 sees them. The overlay's own barrier sits on a *different* node
                                 from the ⌘K listener (one handler per event name)
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
        record_view.rs           the whole-row modal (P2-10): its nested blocks are
                                 `cell_preview_json`'s **bounded, sampled** text (P2-24), built once
                                 per row through a synchronous `PreviewMemo` rather than per render
        cell_view.rs             the nested-value modal (P2-12) — a **lazy tree** since P2-25, not
        value_tree.rs            text. cell_view is the card; value_tree is its model (the
                                 expanded-path set, the flat row projection Freya's `Tree` is
                                 virtualized over, and `PAGE`-at-a-time widening with a `… N more`
                                 tail). It carries the `RecordBatch`, which *keeps* P2-12's snapshot
                                 rule: the arrays it reads are the ones it opened with
        copy.rs                  the shared results-copy path (P2-11) — the *unbounded*
                                 serializers, off the render thread, where the whole value is asked
                                 for
        selection.rs             cell/row/column selection model + SelCtl controller
        find.rs                  find-in-results (P2-09): FindState + the page-local filter
        chart/                   the Chart body (Rz2): mod.rs is the surface — the
                                 `ChartSpec` subscription and the notice states; config.rs
                                 the column roles, the per-mark option sets, and the one
                                 place a `ChartConfig` + a schema resolve into a `ChartQuery`
                                 (`resolve` → `encode`); sort.rs the strip's order, a view
                                 transform over the settled data; strip.rs the mark picker,
                                 the X/Y/Series encoders, the sort toggle and the legend;
                                 paint.rs the frame + the `canvas` (slot-peeked, redraw
                                 requested); axis.rs the plotters `Ranged` category coord +
                                 nice max + abbreviated tick; marks.rs a render fn per mark
        toolbar.rs, status_bar.rs, running.rs, explain_plan.rs, empty.rs, error.rs
```
