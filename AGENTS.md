# Strata — engineering practices

The **how-we-work** companion to [CLAUDE.md](CLAUDE.md) (the *what/where* map: build, workspace
layout, module map, docs index, backlog). Every rule here was settled deliberately during the Freya
rewrite — most after a wrong version was built and rejected in review — so treat them as decisions,
not suggestions.

**This file is the index of rules; `docs/reference/` holds the reasoning.** Each line below is the
rule in its actionable form — act on it as written. Its full entry, with the failure it exists to
prevent, is in the linked file under **the same bolded lead sentence**, so it greps:

| § | Rules | Full text |
|---|---|---|
| 1 | The engineering bar | [reference/BAR.md](docs/reference/BAR.md) |
| 2 | Architecture invariants | [reference/INVARIANTS.md](docs/reference/INVARIANTS.md) |
| 3 | Freya component & UI conventions | [reference/FREYA_UI.md](docs/reference/FREYA_UI.md) |
| 4 | State: where things live | [reference/FREYA_UI.md](docs/reference/FREYA_UI.md) + `docs/FREYA_STATE_ARCHITECTURE.md` |
| 6–7 | The fork, git, verification | [reference/WORKFLOW.md](docs/reference/WORKFLOW.md) |

**Read the full entry before extending, arguing with, or overturning a rule** — several of these
read as over-engineering until you know what was tried first. If a design seems to conflict with
one, that is the signal to open the reference, not to route around it.

**Upkeep:** when a review settles a new convention or overturns one, update **both** the one-liner
here and the full entry in `docs/reference/` in the same change. Session memory may restate rules
from here; this repo is the authoritative copy — if they disagree, trust the repo and fix whichever
is stale.

**Scope:** the `strata-*` crates and app-level work. The Freya fork (`crates/freya`) carries its own
`AGENTS.md` with the upstream author's conventions (`just` not raw cargo, `crate::` not `super::`,
doc comments over inline comments, no em dashes, `KeyExt` on components) — follow that file when
editing fork code, and §6 here for how the fork relates to the app.

---

## 1. The engineering bar

- **Generic capability, not hardcoded subsets** — the real mechanism, not a stub that passes today's case.
- **Real end-states, not placeholders.** No TODO scaffolding as the deliverable. One sanctioned
  exception: a deliberately **inert control** whose capability another task owns (§5).
- **Native Rust tooling, not stray scripts.** Schema/codegen/tests live in the crate.
- **Verify from source before agreeing.** Check the fork or the crate before confirming an API —
  Alex's assertions included. Same bar for your own claims.
- **Framework-native idiom — never pattern-carrying.** Find the Freya/freya-query shape first. The
  Dioxus app is a *behavioural* reference only: no adapters, echo fields, parallel ids, or shims.
  Breaking `strata-dioxus`'s build is expected.
- **Model impossible states out of existence; fail loud on the rest.** Expected absences get
  defaults; unrecoverable faults are surfaced (`ProjectLoadFailed`), never a silent blank fallback.
  Never shape a production signature or add an `Option` to satisfy a test — build the test's store
  inline and pull deps from context.
- **No over-engineering.** Private app: use `pub` freely; don't hand-annotate per-field visibility.
- **A path is qualified in the `use` and nowhere else.** Import the *item*, use its bare name;
  importing a module to qualify through is the same rule broken. Exceptions: visibility modifiers,
  intra-doc links, and the `std` aliases whose module segment disambiguates (`io::Result`). On a
  genuine collision, alias with `as` — never reach through the crate root.
- **Valin-shaped.** Follow [`marc2332/valin`](https://github.com/marc2332/valin) for module layout,
  per-window data scoping, and stateful tabs.

## 2. Architecture invariants

Things that must not regress. Full text: [docs/reference/INVARIANTS.md](docs/reference/INVARIANTS.md).

**Engine, query, results**

- **The engine is a direct-call async facade.** No UI-side runtime, channels, request ids, or
  router. DataFusion is touched **only** in `strata-core`.
- **Results are freya-query off the tab's SQL.** The store holds specs, never results. A Run
  subscription is built **only** through `QuerySpec::query`; cache-entry lifetime is subscriber
  presence, held for background tabs by the request keepers. Never manage entry lifetime imperatively.
- **An expensive, opt-in *result* is freya-query keyed by the request; the store holds the request.**
  A re-scan is a new nonce; invalidating is dropping the request. Never a results field, dedup set,
  or spinner flag. The `Query` is cache identity, so it is built in **one** place.
- **One entry point per expensive action, with the confirm in front of it** (`ProfileActions::ask`).
- **A reader that outlives one Run pins the snapshot it reads** (`Engine::pin_snapshot`, RAII).
  Never a staleness check or warning instead.
- **The snapshot is Arrow IPC, so a result's type survives it.** Parquet cannot write a union at
  all; exact null counts now come from the write pass (`query::SnapshotStats`).
- **A stopped run is not a failed one, and `engine::stopped_on_purpose` is the only thing that knows
  which is which.** Three strings, not one. Never string-match the engine's prose at a call site.
- **An engine's config is a launch value; a live change is `set_config`, and a runtime key is a
  restart** — which is the `ProjectRoot` remount, not a second path. A **removed** key goes back to
  its `ENGINE_KEYS` default; `restart_owed` measures against `built_runtime`.
- **Managed DDL policy.** The editor runs `SELECT`/`EXPLAIN`/`SHOW`/`DESCRIBE` only. Views are
  Save's artifact; typed DDL is blocked with validation pointing at the owning surface.

**Data, values, rendering cost**

- **JSON is read by our own `FileFormat`** (`engine::json_poly`), and a replaced reader inherits the
  replaced reader's **diagnostics**. A `FileSource` must handle its own projection.
- **A view of a value is bounded where the value is *encoded*, never afterwards — and it expands
  breadth-first.** Collapse the value, not its parent; fixed depth with the budget as a backstop;
  an empty container is its own summary. A `render` body never serializes a whole value — it reads
  a **synchronous** cache, not a `use_memo`.
- **An inspector reads the Arrow arrays; only a *document excerpt* goes through text.** Address by
  entry index, resolve with O(1) slices, clip **before** materialization.
- **A recursive `Debug` is not a cheap way to get a type's name.**
- **A virtualized list scrolls its cross axis already; a row that `fill`s is what stops it.** Verify
  a layout question with a `torin/tests` case before building on the answer.

**Diagnostics, logs, problems**

- **Diagnostics are a reconciliation, not an event.** Pure function of buffer revision + catalog
  epoch, stamped per tab; `stale_tabs` is the whole work list and one driver drains it. Never a
  second producer, never enumerate entry points. The catalog is a **gate** as well as an input.
- **A log is recorded by its observer; there is no producer to register with.** Whichever layer
  watched the fact calls `log_event`. Never re-derive an event, never let a log entry be the only
  copy of a live fact. A cancel is logged at the cancel; an entry carries a level, not an origin.
- **Problems holds *conditions*, at two scopes; a run failure is the results pane's.** The test is
  "is it true now, and does it retract itself" — reconciliation, remembered condition, or event.
  The rail badge must total **every** scope from the same functions the scopes use.

**Agent access**

- **An agent's tools are the app's own semantics, and the gate in front of them runs before
  dispatch.** `run` asks `Engine::policy_verdicts` and fails closed, never rewrites SQL, reports a
  stop as a status, and asks `Engine::snapshot_live` rather than reading prose. `read_page` does
  **not** pin.
- **An agent drives the app through the app's own funnels; only a *gate* may be skipped, and only
  when the gate is a question for the user** (the T2 confirm). A run's reply is parked, not awaited;
  registration is per **mount**, keyed by a minted id.
- **An agent that is not *in* the window does not write the window's state; it gets a surface of its
  own.** A surface's state belongs to whoever is looking at it.
- **Poll only what nothing on our side can observe, and name the reason where the poll is.**
  `try_read` never a wait; the timer exists only while the feature is on; staleness bounded and stated.

**Stores and state**

- **The catalog is the `ProjectState` store, not a query.** Never build a `FetchCatalog` capability.
- **Def/runtime split.** Pure serde defs in `strata-model`; `Reg<T>` rows in the store. Tables/views
  keyed by **name**, saved queries by **`Uuid`**.
- **History is a satellite** (`.strata/history.jsonl`), never a store field. Only successful data
  runs; Clear unwrites the file and keeps the `seen` guard.
- **History is a list of queries, not of presses — and dedupe comes before the cap**, keyed by the
  same `util::collapse_sql` that renders the preview.
- **Silent corruption is refused, never warned about — and the refusal is checked against read data,
  not declared metadata** (the Hive NULL-partition gate reads the footer, proceeds only on exact zero).
- **One app-global config store.** Disk is a startup input read **once** — no file watching, ever.
  `write_config` is the sole write path. Settings is a **channel**, not its own global.
- **A draft of shared state commits a per-field diff against its seed, never the whole struct**
  (`Settings::merge_onto`, exhaustive via `settings_merge!`). "Anything to apply?" is `draft != seed`.
- **The theme is pure derived state — deliberately not stored.** Copy `theme.peek().name` out before
  `theme.set(...)`.
- **An uncommitted value that must be live everywhere is a second *input* to the derivation, never a
  stored result.** Keep the slot narrow; dropping it is the revert.
- **A repeated colour is a palette slot, never a repeated `specific`.** Collapse on Midnight.
- **Panel layout lives on `SessionState`** — `Chan::Layout` (structure) + `Chan::LayoutSize` (sizes,
  unsubscribed). Keep panels keyed with fixed `.order()`.

**Windows and lifetimes**

- **A window's project subtree is keyed on the project folder; there is no reopen-in-place path.**
  Never re-point a live store at another project. Anything surviving a re-root lives on the *window*.
- **A window that belongs *to* another window is a child window, and its lifetime is ours.** It must
  not count as a workspace window, and closing-with-the-owner goes through Freya's own path.
- **A window's lifetime must be at least as short as the shortest-lived thing it holds — and for a
  child window that is a *mount* of the project subtree, not a window id.** Take a
  `platform::owner::Subtree` and call `use_owner_pin`; never grow a third copy of the rule.
- **Which window an open lands in is one decision in one place** (`platform::open::decide`, pure).
  Own project = no-op; already-windowed = focus. Both outrank the preference.
- **Every path that destroys a window's work asks on the same terms** — one `CloseTarget`, one
  dialog. The predicate is the engine's own answer, never derived from mounted UI. A question
  already answered is not re-asked (`use_engineless_close`).
- **Nothing blocking runs on the render thread, and a read the user has to wait for is an *arm*, not
  a freeze.** `task::offload`, a thread per call. Cancelling is dropping the answer, never stopping
  the work. A value needed before a window exists gets a deadline, and its consumer must be safe
  against the empty answer.
- **Window geometry** is `Platform::root_size` + `Platform::window_position`, both logical. Restore
  only at window **creation**; there is no runtime resize/move from the app.

**Settings, keymap, input**

- **A setting the user edits through more than one gesture gets one funnel, and the policy lives
  next to the resolution it has to agree with** (`keymap::propose` → `apply`, in strata-core). A
  reset is a proposal; a steal is expressed as the bindings it changes. An override is only "custom"
  if it takes effect.
- **A menubar accelerator is state, not decoration — and it must be disarmed while a chord is being
  captured.** `sync_chords` off a destructured `MenuChords`; `suspend_accelerators` for the capture.
- **An app-wide flag held to protect one window's listener is released on losing focus, not only on
  finishing.** When a flag's scope is wider than the state justifying it, its condition must include
  whatever makes that state reachable.
- **A name two surfaces have to agree on is generated from one table, not typed twice — and
  navigating to something is never editing it.** The category is never restated; the engine's
  properties are indexed off `ENGINE_KEYS` entire.
- **A free-form list setting is edited as rows and committed as a map.** Ids from a counter, never
  the name. The list lives on `SettingsCtx`, not the pane.
- **No command bus.** Distributed `on_global_key_down` per feature (`keymap::on_command`),
  precedence = document order, a modal barrier = an early-mounted consuming listener. Never a
  root-level handler registry — fix the fork limitation instead (§6).
- **The command palette is a *registry of offers*, not a dispatch layer — and it is not a function
  of the keymap.** Every command's body is one call into a funnel that already exists; where that
  logic was inline, it **moves** to the funnel rather than being copied. `CommandRoute::key` renders
  the hint and nothing else. Adding a command is one method.

## 3. Freya component & UI conventions

Full text: [docs/reference/FREYA_UI.md](docs/reference/FREYA_UI.md).

- **Reusable UI is a `Component`** — `struct` + `#[derive(PartialEq)]` + `fn render(&self)`. `mod.rs`
  builds children by **struct literal**.
- **Builder pattern**: chain; never store an element in a variable to mutate later. `.maybe()`,
  `.map()`, `.maybe_child()`.
- **Standard components first.** `Button::new().flat()`, `Select`, `Input`, `Table` — never
  hand-rolled lookalikes. Icon-button clusters are **28×28**. A missing component *state* belongs on
  the component's theme **in the fork**, never as a token on the consuming surface. But the test is
  whether the gap is in the *component*: what a table has no opinion about stays composed in the app.
  Don't restate at a call site what a variant already resolves.
- **A surface with its own component theme reads colours from that theme, not also from the sheet.**
  The sheet is reached directly only for the semantic slots (success/warning/error/info).
- **A shared theme's fields are named for the role they play, not for whoever needed one first, and
  a component's own dress never becomes one.**
- **Fonts are never hardcoded.** Typography role components; `InputTypography` around `Input`s.
  Mixed-style inline text is a `paragraph()` of spans. Theme-consuming hooks run a **fixed** number
  of times per render.
- **Event props follow `Button`'s shape**: `Option<EventHandler<Event<T>>>`. `Callback<A, R>` only
  for value-returning callbacks.
- **One handler per underlying event name.** A second registration silently **replaces** the first,
  and the sugar family shares names with the primitives (`on_secondary_down` → `on_pointer_down`).
  Check `freya-core/src/elements/extensions.rs` before adding any `on_*`; branch inside one handler.
- **A border is painted, never laid out — a bordered box whose children have backgrounds needs
  padding equal to the stroke.** Not CSS's border box.
- **A size lands on the node the parent lays out** — a component that wraps its control sizes the
  **wrapper**. Tell: a fixed width works and a relative one doesn't.
- **`Size::flex` is only divided by a parent whose `content` is `Flex`.** Check this first when a
  "push to the right" spacer misbehaves.
- **A focused `Input` owns the keyboard, so a surface built around one handles its keys in
  `on_pre_key_down`** — and that is what makes it a real modal barrier. Resolve chords through
  `keymap::resolve`. Keep a `GlobalKeyDown` barrier too, on a **different node**.
- **A disabled control gates its handlers; it does not go `interactive(false)`** (which suppresses
  `pointer_leave` and strands a hover).
- **A built-in control's press reaches its ancestors, so never wrap one in a pressable parent** —
  make the label a *sibling*.
- **A settings-style surface is built from `components::form`, never from its own rows.**
  `Form` > `Row` > control, with the register a `Variant` on the form. Where canvases genuinely
  differ, name the difference in `form/mod.rs`'s "known divergences" rather than averaging it.
  A row can be **addressed** (`Row::anchor` + `form::reveal`).
- **A field backing a draft publishes on every keystroke, and normalizes its box when it is left.**
  The change comparison belongs in **state**, never captured (`use_side_effect` builds its closure
  once; use `use_reactive`).
- **Pointer events carry NO modifiers.** Track modifiers via global key handlers; reset defensively.
- **`stop_propagation` vs `prevent_default`**: `prevent_default` in `on_pointer_down` suppresses the
  follow-up `on_press` — do double-click detection inside that same handler.
- **`VirtualScrollView` memoizes its builder closure**, so captured snapshots go stale. Each child
  reads shared state reactively.
- **Reactivity**: `state()`/`.read()` subscribe; `.peek()` does not; `.set()`/`.write()` need `let mut`.
- **Logical units everywhere.** Never multiply/divide by the scale factor in component code.
- **Naming**: plain nouns for structs, no role suffixes; DI handles end in `Ctx`.
- **User-facing text reads like a standard IDE** — terse plain sentences, single-quoted identifiers,
  no em-dashes/backticks/ellipsis/glyphs, no conversational hedges. Merge near-duplicates.

## 4. State: where things live

The decision procedure (full design: `docs/FREYA_STATE_ARCHITECTURE.md`, notes in
[docs/reference/FREYA_UI.md](docs/reference/FREYA_UI.md)):

- **State owned by one tab** → a field on `QueryTab`, under its **own granular `Chan` variant per
  concern**. Channel granularity is the leak-prevention mechanism (`request` on `Chan::Request(id)`,
  so keystrokes never wake the results pane).
- **Shared reactive state with a small, known, shallow consumer set** → **struct-field props**
  (`State<T>` is `Copy` + `PartialEq`).
- **Context** is reserved for DI handles (`EngineCtx`, theme) and deep/open-ended trees (`Selection`).
- **A second surface that needs a settled query's outcome subscribes the query again** — same
  capability, same keys, same `stale_time`. Never a mirror on a store. Caveat: `enabled` is part of
  cache identity, so there is no "watch without running".
- **Never a shared map/registry value** threading every tab's data through one value — that's the
  rejected runs-by-id store in disguise.
- **Inside the fork**, `thread_local!` for shared component state is an antipattern; use the
  lazily-initialized root-context pattern or `State::create_global`.

## 5. Cross-task ownership

Cross-cutting capabilities (clipboard/copy, export, keyboard routing…) get **one** shared
implementation owned by their backlog task in `.claude/tasks/`. When your feature touches a
capability another task owns:

- Ship the UI affordance **inert** — no handler behind it — and add a "wire into X" note to
  **both** task files. Whether it also *looks* unavailable is a design call, not a rule: a menu item
  is **parked** (`MenuButton::enabled(false)`, `catalog/menu.rs`) because a menu is a list of things
  you can do right now, while a surface's **primary call to action keeps its full dress** (the
  inspector's scan card) because greying it out misrepresents the canvas the surface is built to.
  Either way the capability arrives with the task that owns it, and nothing at the call site changes
  but the handler.
- Do **not** build the shared mechanism early, do not fold a local one-off, and leave **no
  unreferenced pre-work**. Record the intended shape in the owning task's file instead.

## 6. The Freya fork: when and how to change it

Full text — including the recovery for an unpushed gitlink:
[docs/reference/WORKFLOW.md](docs/reference/WORKFLOW.md).

- **Fix limitations in the fork, not around it.** When a design starts reaching for a workaround
  (a registry, a scale-factor correction, a duplicated token), the right move is usually a semantic
  fix in the fork. Platform-specific half in its own `freya-winit` module (`cfg`-gated, documented
  no-op elsewhere), primitive on `RendererContext`, discoverable API on `WinitPlatformExt` — so app
  code never touches objc2 or a raw winit handle.
- **Follow the fork's own `AGENTS.md`**; keep changes upstream-shaped (themed tokens, doc comments,
  examples).
- **After changing the fork, push it** — the committed gitlink must exist on the fork remote or
  fresh clones and CI can't init the submodule. This has broken a worktree outright before.
- **Worktree traps — use the `freya-submodule` skill.** `git worktree add` does not update
  submodules. Every worktree has its **own** `crates/freya`: when editing fork files by absolute
  path, confirm the path goes through *your* worktree.

## 7. Git, worktrees, and verification

Full text: [docs/reference/WORKFLOW.md](docs/reference/WORKFLOW.md).

- **Formatting is the `fmt` skill, never `cargo fmt --all`** — `--all` includes local path deps, so
  it reformats the fork (measured once: 344 files, 4006 deletions, none intended, and invisible in
  `git submodule status`).
- **Build + `schema_in_sync` is the check.** After any theme change:
  `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`.
- **CI runs that same check on every PR** — `cargo test --workspace --locked` on **macOS**, with
  `submodules: true`, asserting the gitlink **before** compiling.
- **The release path is a script CI calls, never a pipeline written in YAML.** Signing degrades
  honestly and says which rung it took; the tag is created **after** the build.
- **The version lives in one file and is reached through one script** (`scripts/version.sh`, which
  writes as well as reads). A bump is refused without the release box; the commit is pushed after
  the build and never rebased.
- **The app bundle is self-contained**, and that is a claim each new asset has to keep — naming a
  new font family or weight in a theme means embedding it in the same change.
- **One Strata window across every session — enforced** by `.claude/hooks/block-second-strata.sh`.
  A refusal, not a kill.
- **No destructive git — enforced, not merely agreed.** `git checkout`/`restore`/`reset`/`clean` are
  blocked by a `PreToolUse` hook that reads the whole command string, so chaining behind `&&`, `;`
  or `$(…)` does not get past it. Use `git switch`, `git stash`, `git diff`, or ask. Any other
  delete/overwrite of work you didn't just create: standalone, explicitly described, and not at all
  with substantial uncommitted work in the tree unless you have asked.
- **Task files are the working contract.** Keep the `.claude/tasks/` file true — corrections, wiring
  notes and ownership seams — as part of the change. The same goes for `docs/reference/`.

## 8. High-risk areas

- **The editor's hover/pointer stack** (`hover`/`update_hover`, per-line pointer handlers, the
  diagnostics squiggle popup) is delicate; generalising it has broken diagnostics before, and every
  hover-docs/signature-popup variant was reverted. If function/symbol docs come up again, extend the
  **autocomplete** surface (signatures already render as the completion row's dim detail); treat any
  change to the hover model as prototype-behind-a-flag, and re-verify diagnostics.
- **Modifier tracking** (§3) and **global listener order** (§2, no-command-bus) both depend on fork
  semantics — if either misbehaves, suspect a fork-level fix before an app-level patch.
