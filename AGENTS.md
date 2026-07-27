# Strata — engineering practices

The **how-we-work** companion to [CLAUDE.md](CLAUDE.md) (which is the *what/where* map: build,
workspace layout, module map, docs index, backlog). Every rule here was settled deliberately during
the Freya rewrite — most after a wrong version was built and rejected in review — so treat them as
decisions, not suggestions. The *why* is kept wherever the reasoning **is** the rule.

**Upkeep:** when a review settles a new convention, or overturns one here, update this file in the
same change. Session memory may restate rules from this file; this file is the durable,
authoritative copy — if they disagree, trust the repo and fix whichever is stale.

**Scope:** the `strata-*` crates and app-level work. The Freya fork (`crates/freya`) carries its own
`AGENTS.md` with the upstream author's conventions (`just` commands not raw cargo, `crate::` imports
not `super::`, doc comments over inline comments, no em dashes, `KeyExt` on components) — follow
that file when editing fork code, and §6 here for how the fork relates to the app.

---

## 1. The engineering bar

- **Generic capability, not hardcoded subsets.** Build the real, general mechanism, not a tactical
  stub that happens to pass the current case.
- **Real end-states, not placeholders.** No TODO scaffolding left as the deliverable. (The one
  sanctioned exception is a deliberately **inert control** whose capability another task owns — §5.)
- **Native Rust tooling, not stray scripts.** Schema/codegen/tests live in the crate (e.g. the
  `schema_in_sync` test), not one-off Python.
- **Verify from source before agreeing.** If Alex asserts an API or behaviour, check it in the fork
  (`crates/freya/`) or the crate before confirming; correct it if it's wrong. Same bar for your own
  claims: don't enshrine or restate an API you haven't just looked at.
- **Framework-native idiom — never pattern-carrying.** When porting a feature, find the
  Freya/freya-query native shape first (fork examples, Valin) and build to that. The Dioxus app is
  a *behavioural* reference only, never an architecture to bridge to: no adapters, echo fields,
  parallel ids, or compatibility shims to keep old shapes alive ("I don't want to keep a pattern
  that worked with dioxus for the sake of it"). Prefer widening a native id over introducing a
  mapping. Breaking `strata-dioxus`'s build is expected — it is already broken on purpose.
- **Model impossible states out of existence; fail loud on the rest.**
  - A project can't exist without a folder, so `ProjectState.root` is `PathBuf` (not `Option`), has
    no `Default`, and is only built full from load/scaffold. Don't thread `Option`s or blank
    fallbacks to paper over failures.
  - Expected absences get defaults (missing session file → one blank tab; missing `.strata/` →
    scaffold). Unrecoverable faults (unopenable project dir, unparseable defs/session) are
    **surfaced** — interim `panic!` until P4-01's close-window handling — never a silent
    blank/rootless fallback.
  - Never shape a production signature (or add an `Option`) to satisfy a test — build the test's
    store inline instead. Pull deps like the project root from context
    (`use_radio_station::<ProjectState>`), not params-for-tests.
- **No over-engineering.** Private/internal app: use `pub` freely, don't hand-annotate visibility
  per field on struct-literal-built components (the linter widens them back anyway).
- **Valin-shaped.** Follow [`marc2332/valin`](https://github.com/marc2332/valin) (the Freya
  author's own IDE) for module layout, per-window data scoping, and stateful tabs.

## 2. Architecture invariants

Things that must not regress. Each was fought for once already.

- **The engine is a direct-call async facade** (`strata_core::engine::Engine`): private multi-thread
  Tokio runtime, each call spawned onto it, caller awaits the `JoinHandle`. No UI-side runtime, no
  channels, no request ids, no router/demux — the Dioxus-era `Command`/`Event` protocol was deleted
  with P2-01 and must not be rebuilt. DataFusion is touched **only** in `strata-core`.
- **Results are freya-query off the tab's SQL.** Each `QueryTab` owns its Run trigger
  (`QueryTab::request: Option<QuerySpec>` on `Chan::Request(id)`). The store holds **specs, never
  results** — rows live only in the freya-query cache keyed by `QuerySpec`. No runs-by-id store.
  Two lifetime rules keep that cache honest: a Run subscription is built **only** through
  `QuerySpec::query` (a `Query`'s settings are cache identity — a hand-built variant is a
  different entry, i.e. a duplicate execution), and cache-entry lifetime is **subscriber
  presence**, held for background tabs by the window's request keepers (`views::keeper`, mounted at ProjectRoot —
  one invisible pin per open tab's current press, which also owns history recording). Never
  manage entry lifetime imperatively; mount or unmount a subscriber. Fork-side, freya-query
  never cleans an entry whose execution is in flight and never cancels one on unmount — a
  remounting subscriber attaches to it (`RunningGuard`).
- **Diagnostics are a reconciliation, not an event.** Every open tab's diagnostics are a pure
  function of two inputs — its buffer revision and the catalog epoch — and each tab records a
  `Stamp` of the pair its current rows describe. `SessionState::stale_tabs` is the whole work
  list, and the window's **one** driver (`state::diagnostics::use_diagnostics`, a hook in the
  window root) drains it. Never add a second producer, and never enumerate entry points: a tab
  restored at open, reopened, opened from a view or saved query, duplicated, edited, or left
  behind by a pass a tab switch cancelled are all the same thing — the stamp does not match. It
  is one hook rather than a component per tab because `Chan::Text` is a fan-in (like
  `Chan::Persist`) that lets one subscription watch every tab's buffer. **The catalog is a gate,
  not just an input**: `Engine::register` deregisters before it re-infers, so nothing validates
  mid-scan and no false "not found" is ever produced.
- **A log is recorded by its observer; there is no producer to register with.** The event log
  (`state::log`, the drawer's Events tab) is the mirror image of the rule above, and the contrast
  is the reasoning: a diagnostic is a pure function of two live inputs, so one driver can
  re-derive it and no entry point needs enumerating — an **event** can be re-derived from nothing,
  because it describes something already finished that may no longer exist to be re-read. So
  whichever layer watched the fact records it (the scan pass per def, Save and the drop confirm
  per mutation, the request keeper per settle, `cancel_run` per cancel), by capturing the `LogCtx`
  at render time and calling `log_event`. Never add a producer hook, never re-derive an event from
  live state, and never let a *log* entry be the only copy of a live fact — a registration failure
  belongs on its catalog row, a run failure in the run's own query entry. Two corollaries that
  cost time to rediscover: a cancel is logged at the **cancel**, since clearing the tab's trigger
  unmounts the press's keeper in the same pass and the `Err("cancelled")` settle lands
  unsubscribed; and an entry carries a **level** (the sheet's four semantic slots) but no
  `origin`, because the message already names its subject and a structured copy of that is a
  second copy that can disagree with it.
- **A stopped run is not a failed one, and `engine::stopped_on_purpose` is the only thing that
  knows which is which.** The engine settles **three** such strings, not one — `cancelled` (an
  abort), `superseded by a newer run` (a press that finished after a newer one replaced it) and
  `superseded by a newer scan` (the profile equivalent) — each behind a named const beside the code
  that produces it. Never string-match the engine's prose at a call site: the event log tested
  `e == "cancelled"` and so logged a *supersede* as a red error reading "superseded by a newer run",
  while the inspector's scan zone kept a second copy of the rule (`== "cancelled" ||
  starts_with("superseded")`) that happened to be right. Two copies, one already drifted; both now
  call the predicate. A surface showing a settled `Err` must map every one of them to something the
  user reads as "you stopped this", never as a fault — and none of them may reach Problems.
- **Problems is the SQL-validation surface; a run failure is the results pane's.** A failure
  belongs to a run, not to the text — it can describe SQL the buffer no longer holds, it can't
  self-clear by typing, and `cancel`/supersede settle `Err("cancelled")`/`Err("superseded")`
  that no user should ever read as a problem. Putting it in a cross-tab view costs either a copy
  on the store that outlives the run, or one freya-query subscription per tab in the drawer
  *and* in the rail badge. The results pane already renders it in full.
- **The catalog is the `ProjectState` store, not a query.** Never build a `FetchCatalog`
  capability: introspecting DataFusion would surface the `__snap_*` result snapshots and hide defs
  whose registration failed — precisely the rows the catalog exists to show. Mutations call the
  engine, then the store's own method on the matching `ProjChan`; nothing refetches.
- **An expensive, opt-in *result* is freya-query keyed by the request; the store holds the
  request.** Profiling (P3-09) is the shape: the row keeps `Option<ScanId>` — a nonce minted per
  ask — and the numbers live only in the cache entry that key names, with `stale_time(MAX)` (a
  settled scan must never re-execute itself) and `clean_time(MAX)` ("cached until the entry
  changes"). A re-scan is a *new* nonce, so it is a new execution; invalidating is dropping the
  request. Never a `profile` field holding results on the store, never a dedup set, never a
  spinner flag — the cache key is the dedup and `query.read().state()` is the spinner. And the
  `Query` (stale/clean times included) is the identity, so it is **built in one place**: two call
  sites spelling it differently are two entries, i.e. the same table scanned twice.
- **One entry point per expensive action, with the confirm in front of it.** Every trigger for a
  scan calls `ProfileActions::ask`, which raises P3-10's confirm on a first scan and goes straight
  through on a re-scan; confirming calls the same `start` the ↻ calls. Adding a surface means
  calling `ask`, never reaching for the store directly — the same rule the drop confirm holds.
- **Def/runtime split.** `strata-model` holds pure serde defs only (exactly what
  `.strata/project.json` stores — no runtime caches, no UI flags). The Freya store wraps defs in
  rows with `Reg<T> = Loading | Ready(T) | Failed(String)`, making invalid combos unrepresentable;
  `defs()` is a pure projection for saving. **Identity:** tables/views are keyed by **name** (their
  engine/SQL identity, one shared namespace, case-insensitive compare); saved queries by a stable
  **`Uuid`**. Renames route through the store (a view rename rewrites tab `Origin::View` keys).
- **A reader that outlives one Run pins the snapshot it reads.** A snapshot belongs to its
  workspace and is retired the moment that workspace dispatches another run (SNAPSHOT_SPEC §4),
  which is right for the grid and wrong for anything longer-lived. `Engine::pin_snapshot` hands
  back an RAII `SnapshotPin` that **defers** the retire to the last release — so the export
  window (P4-10) writes the result it was opened on even if the user re-runs the query behind
  it. RAII rather than a pin/unpin pair for the same reason cache entries are held by a mounted
  subscriber: lifetime is a held handle, never imperative bookkeeping. Never answer this with a
  warning or a staleness check instead — "your results moved" is a worse product than results
  that don't move, and a check races the very dispatch it is checking for.
- **History is a satellite**, persisted to `.strata/history.jsonl` — never a field on
  `ProjectState`/`SessionState`. It records **only successful data runs**, which is a claim the
  surface has to keep: the History drawer shows no status mark, because the canvas's
  ok/cancelled/failed dot would have exactly one value. Its **Clear** unwrites the file as well as
  emptying the satellite (an emptied list that refills on the next open is a surface disagreeing
  with its store), and keeps the `seen` dedup guard — that guard is about *runs*, and the pin
  holding a cleared run is still mounted, so forgetting it would put the entry straight back.
- **History is a list of queries, not of presses — and dedupe comes before the cap.** A re-run
  moves its entry to the top with the newest figures instead of stacking a row, keyed by
  `util::collapse_sql`, which is the *same* function that renders the drawer's preview (a key
  looser than the preview merges rows a reader can tell apart; a tighter one lets two identical
  rows sit in the list). The ordering is the load-bearing part: a query hammered 150 times must
  occupy one slot of `max_history`, not all of them, so a keep-last-N over the raw log is wrong —
  collapse, *then* cap. The persisted log follows the same rule without losing the cheap write: a
  new query is one `O_APPEND` line, and only a run that *replaced* an entry rewrites the file
  (`project::save_history`), because an append can add a line but not move one.
- **A window's project subtree is keyed on the project folder; there is no reopen-in-place path.**
  `ProjectApp` is the *window* (theme, app-globals, close bridge, menubar, the `OpenCtx` open
  path); `ProjectRoot` is the *open project* (engine, stores, autosave, catalog, views) and its
  `render_key` is that folder. So "open in this window" (`OpenPref::This`) is a plain `State`
  write: Freya drops the old subtree — flushing its session, dropping its engine, leaving the
  open-set — and mounts the new project through the very hooks that run at launch. Never add a
  second path that re-points a live store at another project: two ways to open one project drift,
  and the mutating one is how relative sources and partition columns get mangled. Anything that
  must survive a re-root (window fill state, the close-confirm slot, the registry entry) belongs
  on the **window** layer, and anything reading "which project" must read it reactively.
- **Which window an open lands in is one decision in one place** (`platform::open`). `decide` is
  pure over plain values and is the *whole* rule; acting on it is split off (`OpenTarget`) because
  a window holds a `Platform` and the menubar handler holds a `RendererContext`. Two rules outrank
  the preference and are not among its outcomes: the project a window already shows is a no-op,
  and a project another window already has is focused — two windows on one project would both
  autosave over its `session.json`.
- **Every path that destroys a window's work asks on the same terms.** The T2 confirm is not the
  close button's — it is the gate for *any* action that aborts running queries, and re-rooting
  (`OpenPref::This`) is one, since the remount drops the engine. Adding such an action means
  adding a `CloseTarget` variant and routing through the one dialog, never a second confirm and
  never a silent abort. The predicate is always the engine's own answer (`guard.running` /
  `Engine::is_running`) plus `confirm_close_running` — never derived from mounted UI, which goes
  false the moment the user switches tabs.
- **Managed DDL policy.** The editor runs `SELECT`/`EXPLAIN`/`SHOW`/`DESCRIBE` only. Views are
  Save's artifact: ⌘S wraps the buffer's plain query in `CREATE OR REPLACE VIEW`
  (`Engine::create_view`); typed DDL is blocked with validation pointing at the owning surface
  (Save / the catalog / Table Config).
- **One app-global config store.** `RadioStation<AppConfig, ConfigChan>` created once in `main`,
  shared into every window (`use_share_config`). Disk is a startup input, read **once** — no file
  watching, ever; after launch only the UI writes. `write_config` (src/state/config.rs) is the
  **sole** write path: mutate + notify + persist; nothing re-reads the file to answer a question.
  Settings is the `ConfigChan::Settings` **channel**, not its own global — one struct = one load,
  one write, no field clobbered by a partial save. `use_config(chan)` to subscribe;
  `use_config_station()` when a handler must only `peek` (key chords, close guard).
- **A draft of shared state commits a per-field diff against its seed, never the whole struct.**
  The Settings window's draft is a snapshot taken when it opened, and another window can commit a
  setting of its own before Apply is pressed — the close confirm's "Don't ask again" writes
  `confirm_close_running` from a window that never shows it. Writing the draft wholesale carries
  its stale copy of that field back over the top: a change the user made, undone by a window that
  never displayed it. So `SettingsCtx` keeps a `seed` beside the `draft` and commits through
  `Settings::merge_onto` (strata-core), which only writes fields the draft actually changed. Two
  consequences. The field list is generated by `settings_merge!` and made **exhaustive by the
  compiler** (`let Settings { … } = self` names every field, so a new setting that isn't merged is
  a build error, not a control that silently never commits) — hand-writing the merge is the
  failure mode, not the macro. And "is there anything to apply?" is `draft != seed`, never
  `draft != committed`: the latter enables Apply for someone else's change, which the merge then
  correctly commits nothing for.
- **The theme is pure derived state — deliberately not stored.** Every window root mounts
  `use_strata_theme(themes, config, preview)`, which derives the effective theme id from the
  settings global (+ `Platform.preferred_theme` while `sync_os`) and resolves through the shared
  `ThemesCtx`. Don't add a stored applied-theme-id global back, and don't store other derivable
  settings projections — subscribe to the channel and compute. Gotcha: copy `theme.peek().name` out
  before `theme.set(...)` — an if-condition temporary holds the read borrow across the write
  (runtime borrow panic on the same GenerationalBox).
- **An uncommitted value that must be live everywhere is a second *input* to the derivation, never
  a stored result.** The Settings window's theme pick has to repaint every window while it is still
  uncommitted, and `write_config` persists — so it rides one narrow app-global slot
  (`state/theme_preview.rs`: `State<Option<ThemeSel>>`, theme id + `sync_os`) that
  `use_strata_theme` resolves *ahead of* the settings. Two rules keep it honest. It stays
  **narrow**: the rest of the draft is the Settings window's own `State`, because putting the whole
  draft in the slot would wake every window's theme derivation on a keystroke in a text field
  (mirror with `set_if_modified` for the same reason). And **dropping it is the revert** — Cancel,
  Esc and the red button all just clear the slot, so there is no restore path to keep in step with
  the commit path.
- **A repeated colour is a palette slot, never a repeated `specific`.** A theme file's colour
  source is the 27-slot `sheet` **plus** its own `palette` of app-named slots, together forming the
  `Palette` a `Theme` resolves references against (fork-side: `Theme.palette: Box<dyn Palette>`,
  `sheet()` required so a custom palette can never break a built-in, `color()` open and consulted
  only for non-core names). Authoring the same hex in two fields is the smell the palette exists to
  remove — name it once and reference it. Two consequences to hold: `reference` is an **open**
  namespace, so the schema can't enumerate targets — an unresolvable name paints magenta and warns
  via `unresolved_references` (`references_resolve` pins the built-ins); and a colour is only one
  token if it is one *per theme*, so collapse on the design source of truth (Midnight) and let the
  others normalize onto it rather than freezing each theme's drift into separate specifics.
- **Panel layout lives on `SessionState`** (not a peer store), so it rides `SessionSnapshot` +
  autosave and survives restart. Two channels, both `Persist`: `Chan::Layout` = structure,
  `Chan::LayoutSize` = sizes (nobody subscribes; a resize drag persists without re-rendering the
  shell). `ResizableContainer` owns live resizing — we persist only the last size. Keep panels
  **keyed** with fixed `.order()` so the `Workbench` subtree survives a sibling collapsing.
- **A window that belongs *to* another window is a child window, and its lifetime is ours.** The
  Settings window is one app-wide, pinned above whichever window opened it (the fork's
  `set_window_parent`), re-pointed when another window asks — with one entry point
  (`platform::settings::open_settings`) so "already open" can only mean focus + re-pin. Two things
  don't come free with the AppKit relationship. It must **not** count as a workspace window
  (`Windows::is_last()` skips it, or the last project closes onto an empty app), and **closing with
  the owner has to be the app's rule, not AppKit's**: AppKit closes a child behind winit's back and
  Freya only removes a window on a close it was asked for, so it would keep a live scope for a
  window that is no longer on screen. Express it in the registry's terms — the owner leaving closes
  the child through Freya's own path — which also covers the platforms where the child relationship
  is a no-op.
- **Window geometry** is read via `Platform::root_size` and the fork-added
  `Platform::window_position` (both logical); never reach for the raw winit handle. There is no
  runtime resize/move from the app — restore geometry only at window **creation**
  (`WindowConfig::with_size` + `with_window_attributes(with_position(..))`), which is why launch
  inputs (project root, geometry) are resolved *before* the window opens.
- **No command bus.** App-level shortcuts are distributed `on_global_key_down` listeners per
  feature (helper: `strata-freya::keymap::on_command`), resolving through the central
  `strata-core::keymap` table. Precedence = document order; a modal barrier = an early-mounted
  consuming listener. Never a root-level handler registry — registries/buses are god-objects that
  centralize what the tree already expresses, and when a design reaches for one to work around a
  Freya limitation, fix the limitation in the fork instead (§6).

## 3. Freya component & UI conventions

- **Reusable UI is a `Component`**: `struct` + `#[derive(PartialEq)]` +
  `impl Component { fn render(&self) -> impl IntoElement }`. Plain functions only for the app root
  and stateless helpers. `mod.rs` builds children by **struct literal**, so fields stay visible.
- **Builder pattern**: chain methods; never store an element in a variable to mutate later. Use
  `.maybe(bool, |el| …)`, `.map(Option, |el, v| …)`, `.maybe_child(Option)`.
- **Standard components first.** Ghost icon buttons are `Button::new().flat()`, input-shell
  dropdowns are `Select`, text fields are `Input` — never hand-rolled lookalikes. The design comps'
  `data-hv` vocabulary maps 1:1 onto existing component themes; duplicating it drifts. Icon-button
  clusters are **28×28**. A missing component *state* (e.g. disabled) belongs on the component's
  own theme **in the fork** (`ButtonColors` grew `disabled_*` for exactly this) — never as a token
  on the consuming surface's theme. And don't restate at a call site what a variant already
  resolves: `Button::new().filled()` *is* accent-over-inverse-text, so a `theme_colors` override
  naming those same two slots is a second copy of them. Override only for a genuinely different
  tone (the destructive action reading `cancel_button`).
- **A surface with its own component theme reads colours from that theme, not also from the
  sheet.** Once a component has a `define_theme!`, every colour it paints — surfaces, borders,
  hairlines, tints — is one of its own fields, authored as a `reference` to a sheet slot where it
  should track one. The sheet is reached for directly only where the value is **semantic**
  (`success` / `warning` / `error` / `info` — the status bar's state dot), because those must
  follow the app-wide ramp wherever they appear. Mixing the two sources in one component is how
  `colors.border` ends up beside a `border_fill` that already holds the same value.
- **A shared theme's fields are named for the role they play, not for whoever needed one first,
  and a component's own dress never becomes one.** The `drawer` theme dresses three bodies, so a
  field called `stats_color` is one the other two can never use — it is `value_color`, "a row's
  secondary fact", and History is merely the first row wanting all three text tones at once. The
  same question kills a field outright when the colour belongs to a *component*: the line-count
  pill's outline was briefly `badge_border_fill` on the drawer, but an outline is the badge's own
  dress, so `Badge::outlined()` derives it from its foreground exactly as the tint derives from
  `TINT_ALPHA` — and every surface that ever uses an outlined badge pays nothing. Before adding a
  field, ask which of the surface's other users could name it too; if the answer is none, it is
  either misnamed or it belongs to the component.
- **Fonts are never hardcoded.** Text goes through the typography role components
  (src/components/typography.rs); `Input`s are wrapped in `InputTypography::body(..)`/`::mono(..)`;
  `CodeEditor` pulls from the theme's code scale. Mixed-style inline text (one sentence changing
  style mid-run) is a `paragraph()` of spans dressed from the typography scale — not adjacent
  labels, which can't wrap or truncate as one line. Hooks that consume theme context must be called
  a **fixed number of times** per render — a variable number of calls breaks hook order.
- **Event props follow `Button`'s shape**: field `Option<EventHandler<Event<T>>>`, builder takes
  `impl Into<EventHandler<Event<T>>>`, and the handler is called with the triggering event even if
  callers ignore it. Never bespoke unit-payload shapes like `Option<EventHandler<()>>`.
  `Callback<A, R>` is a different tool, only for value-returning callbacks
  (e.g. `on_pre_key_down: Callback<Event<KeyboardEventData>, bool>`).
- **One handler per underlying event name.** A second registration silently **replaces** the first,
  and the sugar family shares names with the primitives: `on_secondary_down` is sugar over
  `on_pointer_down` (fork `freya-core/src/elements/extensions.rs`), so chaining it onto a node that
  already has `.on_pointer_down(..)` kills the first handler. Before adding any `on_*`, check which
  event name it registers under; if the node already handles that name, branch inside the one
  handler (match `e.data().button()` for right-click). Diagnostic fingerprint of a replaced
  handler: sibling events (hover) still fire, the press reaches ancestors, the node's own handler
  is dead.
- **A border is painted, never laid out — a bordered box whose children have backgrounds needs
  padding equal to the stroke.** torin has no notion of `border` at all (`BorderAlignment` exists
  only in `style/border.rs` and `elements/rect.rs`), so the default `Inner` alignment draws the
  stroke *inside* bounds the children already occupy, and children paint after the parent's
  background and border. A child at `width(fill)` with its own background therefore erases the
  border behind it. This is **not** CSS's border box, and the failure is partial and so reads as a
  rendering bug rather than a layout one: the export window's transfer panes kept their outline
  around the body (a wrapper with no background) and lost it across the header strip (which has
  one). Pad the bordered rect by the stroke width and subtract it from any child sized from the
  outer edge. Reach for `BorderAlignment::Outer` only when the box may genuinely overflow its
  slot.
- **A disabled control gates its handlers; it does not go `interactive(false)`.** Wrap only the
  action handlers in `.maybe(enabled, …)` and leave `on_pointer_enter` / `on_pointer_leave`
  registered unconditionally, then decline to *dress* the hover while disabled — that is what
  Freya's own components do (`Switch`, `Card`). `interactive(false)` suppresses **every** pointer
  event including `pointer_leave`, so a node disabled while hovered keeps `hovering == true`
  forever and paints a stale hover the moment it is enabled again. Reach for it only for a
  genuinely pass-through overlay, which is the fork's own only use of it (tooltip, drag ghost,
  docking). Clearing the stuck flag in an effect afterwards is treating the symptom.
- **A built-in control's press reaches its ancestors, so never wrap one in a pressable parent.**
  `Switch`'s `on_press` does not `stop_propagation`, so a "click the whole row to toggle" ancestor
  takes the same click and toggles **twice** — back to where it started, which reads as a dead
  control. Make the row's label block a *sibling* of the control instead (Settings ▸ Appearance's
  Sync-with-OS row): the label takes the press, and the control keeps its own focus and keyboard
  operation. Check the component's source before assuming it consumes its press.
- **Pointer events carry NO modifiers.** `MouseEventData` is location + button only. Track
  shift/⌘/ctrl via `on_global_key_down`/`on_global_key_up` into shared state — and beware desync (a
  keyup lost while the window is unfocused leaves a modifier stuck). Reset defensively.
- **`stop_propagation` vs `prevent_default`**: `prevent_default()` in `on_pointer_down` suppresses
  the follow-up `on_press`/`on_global_pointer_press`. If a handler calls `prevent_default`, do
  double-click/press detection *inside* that same handler
  (`EventsCombos::pressed(loc).is_double()`), not via `on_press`.
- **`VirtualScrollView` memoizes its builder closure**, so snapshots captured in the closure go
  stale. Each child reads shared state reactively (`state.read()`) and computes its own view.
- **Reactivity**: `state()`/`.read()` subscribe (re-render on change); `.peek()` does not (use in
  event handlers/actions); `.set()`/`.write()` need `let mut`.
- **Logical units everywhere.** `on_sized` areas, authored offsets/positions/margins, and (since
  our fork fix) `Platform.root_size` are all logical. Never multiply/divide by the scale factor in
  component code — unit mixing here produced dropdowns that were only wrong on retina, and the
  wrong "fix" (dividing measured areas) halves correct values.
- **Naming**: plain nouns for structs (`CloseConfirm`, `Workbench`) — no role suffixes (`…Ui`,
  `…Manager`). DI handles end in `Ctx` (`EngineCtx`, `ThemesCtx`).
- **User-facing text reads like a standard IDE**, matching DataFusion's/JetBrains' register: terse
  plain sentences, single-quoted identifiers, no em-dashes/backticks/ellipsis/glyphs, no
  conversational hedges. ("Table or view 'nope' not found", "CREATE TABLE is not supported in the
  editor. Register tables in Table Config".) Merge or drop near-duplicate messages rather than
  stacking them.

## 4. State: where things live

The decision procedure (full design: `docs/FREYA_STATE_ARCHITECTURE.md`):

- **State owned by one tab** → a field on `QueryTab` in the session store, under its **own granular
  `Chan` variant per concern**. Channel granularity is the leak-prevention mechanism: `request` sits
  on `Chan::Request(id)`, split from `Chan::Tab(id)`, so keystrokes never wake the results pane and
  one tab's press/cancel never touches another tab's results.
- **Shared reactive state with a small, known, shallow consumer set** → **struct-field props**
  (`State<T>` is `Copy` + `PartialEq`), e.g. the workbench's `running` mirror.
- **Context** (`use_provide_context`/`use_consume`) is reserved for DI handles (`EngineCtx`, theme)
  and deep/open-ended consumer trees (`Selection` across the datagrid layers).
- **A second surface that needs a settled query's outcome subscribes the query again** — same
  capability, same keys, same `stale_time`, which *is* a freya-query cache entry's identity — never
  a mirror of the result on a store or a prop threaded across the tree. A settled entry with
  `stale_time(MAX)` is never stale so it can't re-execute, and an execution in flight is attached
  to (our fork counts them). The Problems drawer reads a run's error exactly this way, off the same
  entry the results pane renders. Caveat: `enabled` is part of that identity, so `.enable(false)`
  reads a *different*, never-running entry — there is no "watch without running", and a surface
  that only sometimes has a key mounts its subscriber in a child that only exists when it does.
- **Never a shared map/registry value** (`State<HashMap<TabId, …>>`, a context registry) that
  threads every tab's data through one value into every consumer — that's the rejected
  "runs-by-id store" in every disguise.
- **Inside the fork**, `thread_local!` for shared component state is an antipattern. Use the
  lazily-initialized root-context pattern (`try_consume_root_context::<T>()` → on miss
  `provide_root_context`), as `Http` and `ContextMenu` do, or `State::create_global` for app-level
  multi-window state.

## 5. Cross-task ownership

Cross-cutting capabilities (clipboard/copy, export, keyboard routing…) get **one** shared
implementation owned by their backlog task in `.claude/tasks/`. When your feature touches a
capability another task owns:

- Ship the UI affordance **inert** — no handler behind it — and add a "wire into X" note to
  **both** task files. Whether it also *looks* unavailable is a design call, not a rule: a menu
  item is **parked** (`MenuButton::enabled(false)`, `catalog/menu.rs`) because a menu is a list of
  things you can do right now, while a surface's **primary call to action keeps its full dress**
  (the inspector's scan card) because greying it out misrepresents the canvas the surface is built
  to. Either way the capability arrives with the task that owns it, and nothing at the call site
  changes but the handler.
- Do **not** build the shared mechanism early, do not fold a local one-off, and leave **no
  unreferenced pre-work** (pre-built helpers were removed for exactly this — "let the next task
  redefine that how it likes"). Record the intended shape in the owning task's file instead.

## 6. The Freya fork: when and how to change it

`crates/freya` is a git submodule of `github.com:alexparlett/freya`, resolved by **local checkout
path** — edits are picked up on the next `cargo build`, no push needed locally.

- **Fix limitations in the fork, not around it.** When an app design starts reaching for a
  workaround (a registry, a scale-factor correction, a duplicated theme token), the right move is
  usually a semantic fix in the fork — deterministic listener ordering, logical `root_size`,
  `SelectPlacement`, disabled colors on `ButtonColors`, `set_window_parent` all landed this way.
  The platform-specific half goes in its own `freya-winit` module beside `traffic_light.rs`
  (`cfg`-gated, a documented no-op elsewhere), the primitive on `RendererContext` (the only place
  that holds every window at once), and the discoverable API on `WinitPlatformExt` hopping to it —
  so app code never touches objc2 or a raw winit handle.
- Follow the fork's own `AGENTS.md` conventions when editing it; keep changes upstream-shaped
  (themed tokens, doc comments, examples).
- **After changing the fork, push it** — the committed gitlink must exist on the fork remote or
  fresh clones/CI can't init the submodule. This is not a formality: P4-03's `set_window_parent`
  commit was never pushed, so P4-04's worktree could not build the app at all (`no method named
  set_window_parent`), and no amount of `git submodule update` fixes it — the object isn't on the
  remote to fetch. If you hit that, the commit is in the **main repo's** `crates/freya` checkout:
  `git -C crates/freya fetch --no-tags /abs/path/to/main/repo/crates/freya <sha>` then
  `git merge --ff-only <sha>` (additive, and it keeps your own uncommitted fork edits as long as
  that commit touches different files — check with `git show --stat` first). Then push it.
- **Worktree traps:** `git worktree add` does not update submodules — in any new worktree run
  `git submodule update --checkout` before the first build, then `git submodule status` (no `+`
  prefix). A `+` means the checkout is not the commit the superproject recorded; compare
  `git ls-files -s crates/freya` (the gitlink the index wants) against `git -C crates/freya log -1`
  before concluding anything about a build error in fork API. And every worktree has its **own**
  `crates/freya` checkout: when editing fork files by absolute path, confirm the path goes through
  *your* worktree, not the main repo's copy.

## 7. Git, worktrees, and verification

- **Build + `schema_in_sync` is the check.** After any theme change:
  `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync` (the committed
  `themes/theme.schema.json` must match `theme.rs`'s `REGISTRY`). Sandboxes that can't build verify
  against fork source and hand off to a Mac build (see CLAUDE.md's environment note).
- **CI runs that same check on every PR** (`.github/workflows/ci.yml`): `cargo test --workspace
  --locked` on **macOS** (the platform we ship — a green Linux build proves nothing about the muda
  menubar or the traffic-light gutter), with `submodules: true`, because the build resolves Freya by
  local path and without the fork checkout nothing compiles. `--workspace` and not a bare
  `cargo test`, which `default-members` would narrow to `strata-freya` alone. It asserts the
  submodule sits at the recorded gitlink **before** compiling, so §6's unpushed-fork-commit trap
  fails in seconds with that named as the cause instead of as a missing method 40 minutes in.
- **One Strata window across every session — enforced.** Several sessions can be live in several
  worktrees, and each can build its own binary; a second instance clobbers the shared app config
  (read once at startup, last writer wins for recents / settings / the open-project set). So
  `.claude/hooks/block-second-strata.sh` refuses `cargo run` while any Strata is alive anywhere,
  naming the worktree that owns it. A **refusal, not a kill**: the running window may be what the
  user is looking at. This is a convention between agent sessions, *not* an app-level single-
  instance lock — that is a real feature (one process, N windows, a second launch focuses) and
  belongs to P4-01.
- **No destructive git — now enforced, not merely agreed.** `git checkout`/`restore`/`reset`/
  `clean` are **blocked outright** for agents by a `PreToolUse` hook
  (`.claude/hooks/block-destructive-git.sh`, wired in `.claude/settings.json`). It reads the whole
  command string, so chaining one behind `&&`, `;` or `$(…)` does not get past it — which is
  exactly how the rule was broken while it was only written down. Both hooks bound the verb with
  "not an identifier character" on **each** side: the git one originally required whitespace-or-end
  *after* the verb, so `git reset;`, `git clean|cat` and `$(git clean)` slipped through the very
  chaining forms it claimed to catch (found while building the Strata hook, which had copied the
  pattern). If you add a third hook, copy the fixed pattern and test the terminator forms. Ask the user to run it, or reach
  for something that destroys nothing: `git switch` to change branch, `git stash` to park work,
  `git diff` to inspect. Any other delete/overwrite of work you didn't just create still follows
  the original rule: **standalone**, with an explicit description, and not at all when there is
  substantial uncommitted work in the tree unless you have asked. Cleaning up a failed script means
  removing the exact files it created.
- **Task files are the working contract.** Each `.claude/tasks/` file is self-contained; keep it
  true — record corrections, wiring notes, and ownership seams there as part of the change (the
  `FetchCatalog` correction and the P4-01 fail-loud seam both live in task files because sessions
  read them cold).

## 8. High-risk areas

- **The editor's hover/pointer stack** (`hover`/`update_hover`, per-line pointer handlers, the
  diagnostics squiggle popup) is delicate; generalising it has broken diagnostics before, and every
  hover-docs/signature-popup variant was reverted. If function/symbol docs come up again, extend
  the **autocomplete** surface (signatures already render as the completion row's dim detail);
  treat any change to the hover model as prototype-behind-a-flag, and re-verify diagnostics.
- **Modifier tracking** (§3) and **global listener order** (§2, no-command-bus) both depend on
  fork semantics — if either misbehaves, suspect a fork-level fix before an app-level patch.
