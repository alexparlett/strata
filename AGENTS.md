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
- **The catalog is the `ProjectState` store, not a query.** Never build a `FetchCatalog`
  capability: introspecting DataFusion would surface the `__snap_*` result snapshots and hide defs
  whose registration failed — precisely the rows the catalog exists to show. Mutations call the
  engine, then the store's own method on the matching `ProjChan`; nothing refetches.
- **Def/runtime split.** `strata-model` holds pure serde defs only (exactly what
  `.strata/project.json` stores — no runtime caches, no UI flags). The Freya store wraps defs in
  rows with `Reg<T> = Loading | Ready(T) | Failed(String)`, making invalid combos unrepresentable;
  `defs()` is a pure projection for saving. **Identity:** tables/views are keyed by **name** (their
  engine/SQL identity, one shared namespace, case-insensitive compare); saved queries by a stable
  **`Uuid`**. Renames route through the store (a view rename rewrites tab `Origin::View` keys).
- **History is a satellite**, persisted append-only to `.strata/history.jsonl` — never a field on
  `ProjectState`/`SessionState`.
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
- **The theme is pure derived state — deliberately not stored.** Every window root mounts
  `use_strata_theme(themes, config)`, which derives the effective theme id from the settings global
  (+ `Platform.preferred_theme` while `sync_os`) and resolves through the shared `ThemesCtx`.
  Don't add a stored applied-theme-id global back, and don't store other derivable settings
  projections — subscribe to the channel and compute. Gotcha: copy `theme.peek().name` out before
  `theme.set(...)` — an if-condition temporary holds the read borrow across the write (runtime
  borrow panic on the same GenerationalBox).
- **Panel layout lives on `SessionState`** (not a peer store), so it rides `SessionSnapshot` +
  autosave and survives restart. Two channels, both `Persist`: `Chan::Layout` = structure,
  `Chan::LayoutSize` = sizes (nobody subscribes; a resize drag persists without re-rendering the
  shell). `ResizableContainer` owns live resizing — we persist only the last size. Keep panels
  **keyed** with fixed `.order()` so the `Workbench` subtree survives a sibling collapsing.
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
  on the consuming surface's theme.
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

- Ship the UI affordance **inert** (a no-op control), and add a "wire into X" note to **both** task
  files.
- Do **not** build the shared mechanism early, do not fold a local one-off, and leave **no
  unreferenced pre-work** (pre-built helpers were removed for exactly this — "let the next task
  redefine that how it likes"). Record the intended shape in the owning task's file instead.

## 6. The Freya fork: when and how to change it

`crates/freya` is a git submodule of `github.com:alexparlett/freya`, resolved by **local checkout
path** — edits are picked up on the next `cargo build`, no push needed locally.

- **Fix limitations in the fork, not around it.** When an app design starts reaching for a
  workaround (a registry, a scale-factor correction, a duplicated theme token), the right move is
  usually a semantic fix in the fork — deterministic listener ordering, logical `root_size`,
  `SelectPlacement`, disabled colors on `ButtonColors` all landed this way.
- Follow the fork's own `AGENTS.md` conventions when editing it; keep changes upstream-shaped
  (themed tokens, doc comments, examples).
- **After changing the fork, push it** — the committed gitlink must exist on the fork remote or
  fresh clones/CI can't init the submodule.
- **Worktree traps:** `git worktree add` does not update submodules — in any new worktree run
  `git submodule update --checkout` before the first build, then `git submodule status` (no `+`
  prefix). And every worktree has its **own** `crates/freya` checkout: when editing fork files by
  absolute path, confirm the path goes through *your* worktree, not the main repo's copy.

## 7. Git, worktrees, and verification

- **Build + `schema_in_sync` is the check.** After any theme change:
  `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync` (the committed
  `themes/theme.schema.json` must match `theme.rs`'s `REGISTRY`). Sandboxes that can't build verify
  against fork source and hand off to a Mac build (see CLAUDE.md's environment note).
- **No compound destructive git.** `git checkout`/`restore`/`reset`/`clean` — and any
  delete/overwrite of work you didn't just create — run **standalone** with an explicit
  description, never chained into a compound command; with substantial uncommitted work in the
  tree, not without asking. Cleaning up a failed script means removing the exact files it created.
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
