# strata-freya module map

The annotated tree of the Freya app — plus short maps of its two satellite crates,
`strata-agent` and `strata-command-macro`, at the end: what each module is and, where it
matters, why it is shaped that way. Read this when locating code or deciding where something
new belongs.
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
src/platform/connection.rs       where the **connection editor** goes (W7 · 03) —
                                 configure.rs's rules verbatim, one window per `ConnectionTarget`
                                 keyed by owner *and* target, because it too is opened on a def
                                 that two windows would both write
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
src/menu.rs                      the macOS menubar: **App** (About · Check for Updates… ·
                                 Settings…) · **File** (Open… · Open Recent ·
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
src/state/mod.rs                 `AppCtx` — the app-globals `main` creates once (themes · config ·
                                 window registry · theme preview · menubar handles · the focused
                                 window's open path · agent access · model listings · provider
                                 probes · the assistant runtime · the update status · the focused
                                 window's restart-question slot), handed to every window root as
                                 one value rather than as parameters
src/agent/                       AA-03 / AA-03b — agent access, the half that outlives any one
                                 window. `AgentCtx` is the pair `main` creates: the **directory**
                                 (lives for the process; windows join and leave it) and the
                                 **server slot** (what is listening now, or nothing — dropping it
                                 *is* stop). The window's half lives with the window, because that
                                 is what it is made of (`apps/project/state/agent.rs` beside the
                                 diagnostics driver, `state/agents.rs` beside the log satellite).
                                 **No surface shows any of it**: the Agents pane and the header's
                                 status dot were removed, so the server runs unshown
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
src/state/listings.rs            AS-06 — the app-global **model listings** slot: what each
                                 provider last reported serving, loaded from its own file in
                                 `main` (no dial-out there) and written by `write_listings`, the
                                 one path that persists. App-global because two surfaces pick a
                                 model (Settings ▸ AI ▸ Chat, the composer footer) and neither
                                 owns the list; persisted because a picker fed only by a live
                                 call is empty at every launch. Distinct from `Probes`, which is
                                 the *outcome* of a request and must not survive a restart
src/state/updates.rs             UP-02 — the app-global **update status**: what the updater last
                                 learned, plus the check / download / install actions and the one
                                 startup check (`use_updates`, mounted by both workspace windows
                                 like `use_agent_server`). App-global because there is one running
                                 app to update; **not** persisted, on `Probes`' reasoning. A
                                 worker outlives the window that started it, so its settled status
                                 is parked in a process-global the next mount adopts, and the
                                 install intent is another one — the swap happens in `main` after
                                 `launch` returns. The mechanism itself is `strata_core::update`;
                                 the surfaces are `src/updater.rs`
src/updater.rs                   UP-03 — the updater's **surfaces**, the half that belongs to no
                                 single window (so not an `apps/` folder, which is one per OS
                                 window). `Affordance::of(status, site)` is the one answer to "what
                                 does the app offer right now" — a pure, unit-tested function, so
                                 the launcher rail's label, App ▸ Check for Updates… and the
                                 dialog cannot each restate the rules: a dev build offers
                                 nothing, a release with no archive (or a bundle that cannot be
                                 replaced) degrades to the release page, a staged update is a
                                 restart. `press` is the one gesture behind all four, each arm a
                                 call into `state::updates`. `UpdateConfirm` is the restart
                                 question — one component, mounted at both workspace window roots,
                                 over a **per-window** `AskSlot` (two project windows must not both
                                 raise it). `UpdateRequest` is the app-global App ▸ Check for
                                 Updates… records its press in: that item has no chord to
                                 synthesize *and* no Freya scope (the menu handler runs on the
                                 renderer thread, where `spawn_forever` panics), so the focused
                                 window drains it from `use_file_menu`'s effect
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
src/keymap.rs                    Freya-side keymap glue: the event→chord fold, `on_command` (the
                                 distributed-dispatch handler builder — no registry: each feature
                                 attaches its own global listener, precedence is document order,
                                 and a handled press consumes via `prevent_default`) and reactive
                                 shortcut hints. Policy + resolution live in `strata-core::keymap`
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
  avatar.rs                      the initials tile leading a project row (the header's switcher,
                                 the launcher's lists). The caller passes the *name* — deriving
                                 the initials is the component's job, so every list spells a
                                 project the same way
  badge.rs                       tinted label pill (PART · HOTSPOT · ANALYZE · dtype).
                                 NOT Freya's `Chip` — that's a selectable, focusable control
  dialog.rs                      the confirm dialog shell — every centred confirm is header ·
                                 body · footer on this one 420px card; callers supply only what
                                 differs (the header's icon, tone and title run, the body, the
                                 buttons). Enter-confirm lives on the card, in the slot the
                                 modal base leaves open
  modal.rs                       the **modal base** (Chart 09): open/closed and nothing else —
                                 overlay layer, backdrop, Esc and outside-press as a close
                                 request, the key barrier (Enter deliberately left to the
                                 surface's own card). `Dialog` wraps its confirm card in this;
                                 the Shape panel wraps its own working card in the same base
  keycap.rs                      a **key cap** (`"keycap"` token group): Settings ▸ Keymap's
                                 chords and the palette's shortcut hints have to look like the
                                 same kind of object, and the colours were the `settings` theme's
                                 own until P6-01. Two *named* shapes rather than an average —
                                 `key` (raised, heavier bottom edge: the row is about the chord)
                                 and `chip` (flat: the chord is a hint). One cap per call; a
                                 chord is composed by the caller
  metrics.rs                     the design's spacing + radius scale (`SP_1…9` / `R_XS…4`,
                                 `Design.dc.html` §03) as constants — not theme fields, since a
                                 step doesn't vary by theme — plus `pill()`, `HAIRLINE`, and the
                                 fixed sizes more than one surface agrees on (tool button, title
                                 bar, panel headers, table rows). P5-03's shared durations land
                                 here beside `PROGRESS_HOLD`
  sidebar_row.rs                 the left pane's row shell: a preset over Freya's `SideBarItem`
                                 (+ `Activable` for selection), so hover/selected dress and a11y
                                 are shared by the catalog and, later, connections (W7)
  tones.rs                       the four semantic tones (success · info · warning · error) read
                                 off the roles as **one** shared hook — the only place that reads
                                 them; three surfaces had grown three copies of the four-slot read
  tool_button.rs                 the 28×28 list-toolbar icon button (add / remove / duplicate /
                                 paste / browse) — the *action* carries the tone, and the label is
                                 a **required** tooltip, because an icon-only button has no
                                 accessible name of its own
  toolbar.rs                     P5-06 — the chrome row that degrades instead of spilling:
                                 [ leading run (ellipsizes) ][ items (fold tail-first into ⋯) ]
                                 [ pinned tail (never folds) ]. One fold policy for every row,
                                 arithmetic over the item list
  type_palette.rs                the seven per-`Kind` hues (`"type_palette"` theme group) +
                                 `kind_color`. Named for Kind, not Arrow; the EXPLAIN plan
                                 borrows the same ramp for operator kinds
  window.rs                      **window chrome** — the tones every window's body, recessed
                                 insets, rules and status blocks are built out of. One theme for
                                 all windows, not one per window; a field is added when a surface
                                 reads it, never in anticipation
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
                                 blur) + `PathField` (a path box + the native picker, over a
                                 folder or one file: one buffer, both write it — the picker sets
                                 the box and the box is what reports. One component for both
                                 kinds, because they differ in the picker call and nothing else)
    options.rs                   **options as data** — a surface hands over a `Vec<Group<E>>` and
                                 this renders it blind, one component per control *shape*, so a
                                 new option is a row in a table rather than a branch in a
                                 component (P4-10 / D6). Every option carries the `Edit` it
                                 performs, so a control cannot write the wrong field
src/apps/launcher/               the launcher / welcome window (P4-02, `Launcher.dc.html`)
  mod.rs                         root + window config + the `launcher` component theme
  model.rs                       ProjectList: the filter + PINNED/RECENT split (unit-tested)
  views/                         title_bar · rail (SidebarRow — and the version line, which is
                                 UP-03's update affordance) · projects · row · open (rfd pick)
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
                                 check-for-updates (UP-03) · query-history limit. All six already
                                 had their reader, so this is the control; the open-pref pill is the one worth naming — the
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
src/apps/export/                 the Export window (P4-10 — `Export.dc.html` for the layout,
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
                                 model.rs (the draft + its option groups), interaction.rs (the
                                 body and footer driven as a user drives them), views/ (title_bar ·
                                 location · identity · paths · options · hive · status · footer).
                                 It is a child of the project window that asked, so it writes that
                                 window's store through the shared `persisted_defs` funnel and
                                 asks *its* one scan driver for the registration pass — rather
                                 than holding an engine or a second "make the engine match the
                                 defs" of its own. **location.rs** is the LOCATION toggle and,
                                 behind its object-store arm, the TYPE / CONNECTION pair (W7 ·
                                 04): the draft records the connection's `url()`, the source list
                                 goes single-path under the bucket as a prefix, and
                                 `register::table_spec` composes the two — so the window needs no
                                 engine call and the picker's *New connection…* sets the project
                                 window's own `ConnectionRequest` rather than opening an editor
src/apps/connection/             the **connection editor** window (W7 · 03 —
                                 `Connections.dc.html`): add or edit one remote object store.
                                 mod.rs (root · window config · `ConnectionCtx` · `Status`),
                                 model.rs (`ConnectionTarget` + the draft, which holds *every*
                                 provider's fields flat so flipping the picker forgets nothing,
                                 and projects the one in play, plus `ConfigRows` — the client
                                 options edited as identified rows and committed as a map),
                                 interaction.rs (the editor driven as a user drives it: which
                                 controls a provider has, and a Save that writes one def and
                                 deregisters the URL it moved off — tests), views/ (title_bar ·
                                 form · status · footer). The form's rows are
                                 **all keyed by the provider**, so switching one is a clean
                                 remove-and-add: a row that merely *moves* index is recorded by
                                 the differ as moved and then unwraps a scope it left behind. Configure's shape throughout: a child of the
                                 project window that asked, writing that window's store through
                                 `persisted_defs` and asking *its* scan driver for the pass.
                                 Two things are its own — Save deregisters the old URL when an
                                 edit moves the connection's identity (`engine::store::connect`
                                 never sees the def it replaced), and the pass it asks for is
                                 whole-catalog, because `ScanScope` has no connection width and
                                 every table over the bucket was registered against the store
                                 being replaced
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
  close.rs                       the window's **close bridge**: the close-while-running confirm
                                 (T2) and the last-window-becomes-the-launcher rule, one
                                 mechanism because both need the OS close held off long enough
                                 for the UI to act — `on_close` runs on the winit thread and must
                                 be `Send`, so a guard (`CloseGuard`, atomics) answers it
                                 synchronously and a declined close sends a `Veto` that wakes the
                                 UI to do what the hook couldn't, then re-close programmatically
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
                                 the RuntimeEnv is fixed when the SessionContext is. The forwarders
                                 (pin_snapshot, chart, trend, export) are the methods taking
                                 `&Arc<Engine>`, which Deref cannot reach
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
                                 connections are a fourth section here, on
                                 `ProjChan::Connections` and keyed by `ConnectionDef::url()`
                                 (scheme + authority — never the bucket, which two providers
                                 can share) — a `ConnRow`'s `Reg<()>` carries no payload
                                 because connecting *registers* an object store rather than
                                 inferring anything, so the three states are the whole value
                                 catalog.rs = CatalogSelection, the inspected column (context)
                                 log.rs = P3-13's **event log** satellite (`LogCtx`, ephemeral,
                                 capped): the record behind the drawer's Events tab. No producer
                                 hook — whichever layer observed the fact appends it (the scan
                                 pass, Save, the drop confirm, the keeper's settle, `cancel_run`)
                                 history.rs = P4-14's **query-history** satellite
                                 (`.strata/history.jsonl` — never a store field): a list of
                                 queries, not presses — a re-run moves its entry up with the
                                 newest figures, keyed by `collapse_sql`, dedupe before the cap.
                                 The drawer's History tab reads it; Clear unwrites the file
                                 persist.rs = P4-15's **write funnel**: the one place a `.strata`
                                 write failure is reported. `persisted(log, ProjectFile, write)`
                                 → an event row + a `bool` the caller uses to decline claiming a
                                 success; `ProjectFile` is the only copy of each file's wording.
                                 Here rather than beside a caller because every writer that was
                                 added *away* from the old home grew its own silent
                                 `tracing::error!` instead of finding it
                                 chat.rs = AS-04's **transcript satellite**: `Chats` (several
                                 conversations, both lists capped), each with its per-conversation
                                 `Pick`, its pinned `Anchor`s, its `Turn`s of ordered `Block`s
                                 (prose · a step card's citation · an `offer_sql` statement), the
                                 model's own `Conversation`, and the task driving its turn —
                                 whose *drop* is the cancel. Ephemeral: nothing reaches
                                 `session.json` (AS-07 is what makes a transcript survive), and
                                 nothing reaches history, which stays the user's. Unit-tested
                                 with no renderer
                                 chat_send.rs = AS-04's **send funnel**: `AssistantCtx` (the
                                 app's runtime, one `StrataTools::in_app` per mount, and the
                                 project scope), `seed_pick`, `blocked` (what is missing, named
                                 before a press rather than reported after one) and `send` — which
                                 resolves the store-answerable anchors on the render thread,
                                 records the question, then spawns the one task that describes the
                                 rest, reads the key **off** the render thread and folds every
                                 `TurnEvent` into the transcript
                                 statement.rs = ED-02's **statement settle**: one fold applying an
                                 intercepted statement's `StoreEffect` — store channel →
                                 `persisted_defs` → `catalog_settled` → the event log. Driven
                                 from the request keeper, and it owns the log row because only
                                 the fold knows whether the def was written
  model/                         window-local view models
  views/
    dialogs/                     the window's modal dialogs, mounted early so their key barrier
                                 precedes every feature listener: close_confirm (T2) ·
                                 drop_confirm (P3-05 — every removal that destroys project work,
                                 the Connections pane's Forget included: one card, one
                                 `DropTarget`, one event) · open_prompt (the This/New question) ·
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
    shell.rs                     P3-01 — the body shell: rail | (sidebar · workbench · inspector
                                 over the drawer), the collapsibles as `ResizableContainer`
                                 panels — present only when the layout has them open, keyed with
                                 fixed `.order()` so the workbench survives a sibling collapse
    right_rail.rs                AS-04 — the **right** 48px rail: `rail.rs`'s mechanism on the
                                 other edge, picking which assistive surface the right pane shows
                                 (inspector · chat). One slot rather than two panels, which is
                                 what keeps a 1180px window readable with both rails, a sidebar
                                 and the drawer up. No badge on either button: the inspector has
                                 nothing to count, and an unread count is a notion for a surface
                                 somebody else writes into
    chat/                        AS-04 — the **chat pane**: mod.rs (the frame, the `chat` theme,
                                 `ask_about` — the one funnel every friction entry opens through
                                 — and `result_anchor`), header.rs (the chat switcher, over
                                 `Menu`), transcript.rs (turns in arrival order; prose through
                                 the fork's `MarkdownViewer`), card.rs (the **step** card, a
                                 citation whose every figure is the engine's own, and the
                                 **offer** card, executable because `offer_sql` checked it — both
                                 promoting through `actions::open_sql`, never into the user's
                                 buffer), composer.rs (chips · input · the per-conversation
                                 model + effort pick · send-becomes-stop), mention.rs (the `@`
                                 picker over the catalog **store**)
    rail.rs                      the 48px activity rail: two `ToggleButton` groups — the top
                                 picks the sidebar pane (Catalog · Connections), the
                                 bottom the drawer tab (Problems · Events · History). `on` is
                                 *derived* from the layout, the single source of truth; a press
                                 routes through the layout store's toggle
    configure_launch.rs          P4-11 — the slot a "Configure…" trigger sets and the one place
                                 that acts on it: a row's ⋮ menu is built inside an event handler
                                 where no hook may run, so the trigger sets `ConfigureTarget` and
                                 does nothing else, and this root-mounted watcher — where the
                                 window's handles actually live — opens the window
    connection_launch.rs         W7 · 03 — configure_launch's shape verbatim over
                                 `ConnectionTarget`: a sidebar trigger sets the slot, the
                                 root-mounted watcher opens the connection editor
    header/
      mod.rs                     the header bar — and the window's macOS title bar: brand ·
                                 switcher · ⌘K/⌘, cluster, drag + double-press-to-fill
                                 (`window_drag`), traffic-light gutter
      project_menu.rs            the project switcher: trigger + Open… / open set / recents
                                 dropdown; every row opens through the window's `OpenCtx`
    sidebar/
      mod.rs                     sidebar shell — pane-specific header (the catalog's filter +
                                 refresh row, Connections' label + ⓘ + `+`) over the active pane
      catalog/                   P3-02: mod (pane + sections), section, entry (entry/column/
                                 saved-query rows), columns (flatten + tests), menu (P3-06: one
                                 item list per row kind, shared by right-click and the ⋮ so the
                                 two triggers can't drift; Drop opens the confirm, never drops),
                                 interaction (tests)
      connections/               W7 — the project's object stores: mod (pane + theme + the
                                 header's ⓘ and `+`, one row per `ConnRow`), interaction (tests).
                                 **The catalog entry row's shape**: badge, bucket, one trailing
                                 status slot, ⋮. A row that registered is clean; a refused one
                                 wears the warning triangle with the engine's reason on its
                                 popover, in full — a two-line row spelling that reason under
                                 the bucket ellipsized it to four useless words. What the slot
                                 reports is the registration outcome, never a probe of its own:
                                 `connect` resolves the chain once and throws the answer away so
                                 this slot can mean something. `Loading` states nothing until the
                                 wait outlasts `PROGRESS_HOLD`, then spins, holding the last
                                 settled verdict across the gap.
                                 The row is **not** clickable (the doc's "The Connections pane" section); Edit and Forget are the
                                 ⋮ / right-click menu, and Forget sets the shared remove
                                 confirm's `DropTarget::Connection(url)` — the dialog owns the
                                 store mutation, the persist and `Engine::disconnect`. Add and
                                 Edit set `ConnectionRequest` and stop, on the same terms: the
                                 editor window is `ConnectionLauncher`'s at the project root
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
                                 status dot: only successful runs are ever recorded
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
      editor/                    SQL editor: tab, toolbar, actions (P2-16 — Format/Clear + the
                                 Save dispatch on the tab's `Origin`: a view-bound tab re-issues
                                 `CREATE OR REPLACE VIEW` on *its* view, a saved-query tab
                                 overwrites by id, a scratch tab Save-As-es; free functions over
                                 the window's stores, so the toolbar and ⌘S share one
                                 implementation)
      tab_bar/                   bar, tab, controls (new/navigate/overflow), drag, menu (context menu)
      results/
        mod.rs                   results panel — freya-query-driven states (empty / running /
                                 grid / explain / error) off the workbench's `request` slot
        datagrid/                mod, header, cell, row, model, interaction  (sticky typed
                                 header, cells virtualized in **both** directions — a
                                 VirtualScrollView over the rows × a scroll-derived column
                                 window, spacers standing in for the off-window extent —
                                 per-column resize + on-demand double-click autofit; row.rs is
                                 one virtualized body row — everything reactive read *inside*
                                 the memoized builder — owning its cells' handlers: record
                                 view, value modal, right-click copy menu; interaction.rs the
                                 focus-routed copy-chord + copy-menu tests)
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
        sort.rs                  column sort (P2-13): the per-run sort intent the header
                                 chevrons cycle, owned by the results body (so it resets with
                                 every press, like the page number) and folded into the snapshot
                                 read — `ORDER BY` over the whole snapshot at page-read,
                                 `PageSpec.sort` part of the cache key
        chart/                   the Chart body (Rz2): mod.rs is the surface — the
                                 `ChartSpec` subscription and the notice states; config.rs
                                 the column roles, the per-mark option sets, and the one
                                 place a `ChartConfig` + a schema resolve into a `ChartQuery`
                                 (`resolve` → `encode`); sort.rs the strip's order and hide.rs
                                 the legend's hidden set, both view transforms over the settled
                                 data (sorted **then** hidden); strip.rs the mark picker, the
                                 X/Y/Series encoders, the bin count, the sort + scale toggles
                                 and the pressable legend; paint.rs the frame + the `canvas`
                                 (slot-peeked, redraw requested) + the crosshair, ruled through
                                 the hovered mark so it costs no repaint; axis.rs the plotters
                                 `Ranged`s — the category coord and the linear-or-log
                                 `ValueCoord` — plus nice max, decade span and abbreviated tick;
                                 marks.rs a render fn per mark plus the **one** draw body (a
                                 canvas + a `FontCollection`, returning its hit regions and the
                                 plot frame); capture.rs Copy Image — the same frame through
                                 that same body onto an offscreen surface, then the clipboard;
                                 preview.rs the headless PNG harness (the plan view's), because
                                 a chart's correctness is *visual*
        shape/                   the **Shape panel** (Chart 09): compose.rs the pure
                                 form-to-SQL composer (UI-local `SqlAgg`/`Stride` vocabulary,
                                 subquery form, ordinal `GROUP BY`, an always-emitted
                                 `ORDER BY`, idents through the engine's `quote_col`); mod.rs
                                 the `ShapeTarget` slot vocabulary and the working card on the
                                 shared `Modal` base — group rows (a stride `Select` per time
                                 column, sub-day only for a clock), measure rows, the row-count
                                 toggle and the order pill, confirmed into a **new unrun tab**
                                 via `open_named`. Triggered from the results toolbar on both
                                 bodies; the chart body seeds it from the resolved encoding
        explain_plan/            the EXPLAIN plan view (P2-05, EXPLAIN_PLAN_SPEC v3): mod.rs the
                                 `explain_plan` theme + the shell (Physical/Logical segments ·
                                 ANALYZE badge · Raw/Tree toggle over the tree or the raw text —
                                 all values arrive pre-typed from `engine::plan`, no unit math);
                                 node.rs one railed operator card + the three-tier ANALYZE
                                 metrics block; palette.rs kind / metric / group / tone onto the
                                 theme's colour fields; preview.rs the headless render harness
                                 (`--ignored`, writes `target/plan-preview.png`)
        toolbar.rs, status_bar.rs, running.rs, empty.rs, error.rs
        statement.rs             ED-02's **statement state**: an intercepted statement's report,
                                 in the empty-state layout in success dress. No grid, no pager,
                                 no snapshot handle — the tab keeps the one it had, because DDL
                                 retires nothing
```

## crates/strata-agent — agent access (Freya-free)

Everything frontend-agnostic about agent access (`docs/AGENT_ACCESS_SPEC.md`), **and** the
assistant's loop (AS-02). The crate has **no Freya dependency**, which is what lets one
`StrataTools` serve HTTP (AA-03), stdio headless (AA-05) and the in-process chat loop, and lets
that loop be tested against a mock host and a stub endpoint with no window and no vendor.

```
src/lib.rs                       the crate charter + the seam diagram: rmcp server / stdio host /
                                 chat loop → `StrataTools` (the ten tools) → `Host`
src/tools.rs                     the **vocabulary** — the ten read-only tools (the doc's "The ten tools" section) as the
                                 rmcp `ServerHandler`, deliberately transport-free. The policy
                                 gate runs here, *before* dispatch; `run` never rewrites SQL and
                                 reports a stop as a status, not a fault
src/host.rs                      the **`Host` seam** — the union of the vocabulary's questions
                                 and nothing else. Methods return `impl Future + Send` rather
                                 than `async fn` (rmcp polls on its own runtime and needs `Send`,
                                 which an `async fn` in a trait doesn't promise)
src/wire.rs                      the **wire shapes** — flat JSON in/out, projected from the host
                                 types by the `from_*` fns so no tool assembles a response by
                                 hand. A cell is `null` or a string (the engine's own
                                 `CellFormat` text, the same the grid shows)
src/error.rs                     the error taxonomy (the doc's "Error taxonomy" section) — every fault typed once, rendered
                                 once, as `isError` tool results. No `Stopped` variant (a stop
                                 is an outcome shape, not a fault) and no `Unauthorized` (a bad
                                 token is HTTP 401 before any tool runs)
src/server.rs                    the MCP server: Streamable HTTP on loopback + bearer token,
                                 stop-on-drop — the Engine pattern (a small private Tokio
                                 runtime behind a plain handle), because rmcp needs a reactor
                                 and the UI thread is not one
src/headless.rs                  AA-05 — the **headless host**: `strata mcp <project>` over
                                 stdio (the client owns the process, so process ownership *is*
                                 the auth). A plain `Engine` with the registration pass replayed
                                 on it; the pass's own outcomes are the catalog
src/mock.rs                      `MockHost` — a `Host` over plain values and a **real** engine:
                                 the vocabulary's test rig and the executable statement of what
                                 a host owes. Public, not `#[cfg(test)]`, because the MCP
                                 integration test lives in `tests/`
src/assistant/mod.rs             AS-02 — the **assistant**: `Assistant` (its own small Tokio
                                 runtime, deliberately not `AgentServer`'s, whose lifetime is a
                                 setting) and `Running`, a turn in flight. Dropping one cancels
                                 it
src/assistant/provider.rs        the **provider seam**: `PROVIDERS`, the one table every surface
                                 reads a kind's label, URL policy, key policy and **effort
                                 ladder** from; `Selection` (a pick, per send); `Brain::resolve`,
                                 the single site a `genai` client is built, which either builds
                                 one or names the missing field and the pane it is set in
src/assistant/turn.rs            the **loop** and its `TurnEvent` stream, `Conversation` (the
                                 *model's* memory, opaque outside the crate — the pane's
                                 transcript is a different list), and cancel, which is a drop
                                 because a drop is already the engine's abort
src/assistant/dispatch.rs        **name → method**: the one match binding a model's tool call to
                                 the ten, with a bad-arguments message written for a model to
                                 read, plus `Scope` and the step card's `Facts`
src/assistant/offer.rs           `offer_sql` — the assistant's own eleventh tool, **never on the
                                 router**: how it hands the user a statement to execute,
                                 validated before the card exists
src/assistant/system.md          the system prompt, `include_str!`'d — prose, in a file, because
                                 it is edited as prose
```

## crates/strata-command-macro

```
src/lib.rs                       the workspace's one proc macro: `#[command_router]` /
                                 `#[command]` (P6-01) — rmcp's `#[tool_router]` declaration
                                 shape (id from the method name, subtext from the doc comment)
                                 but generating an **enum**, so dispatch is total by
                                 construction and no unrunnable command exists. It knows nothing
                                 of Strata's types — a registration mechanism, not a vocabulary
```
