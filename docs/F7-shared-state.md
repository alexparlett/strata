# F7 — Shared-state architecture: design & plan

Status: **proposal** (design only — no code changed). Owner: Strata. Supersedes the F7 stub in `DEV_TASKS.md`.

> Note on method: this revision separates *documented framework behaviour* (Dioxus's own words — not up for debate) from
> *pattern tradeoffs* (where reasonable engineers and the literature disagree). An earlier draft of this doc asserted
> the
> pattern choice was simply "right"; that was an overreach and is corrected here. Sources are listed at the end.

## TL;DR

There are three different questions bundled in F7, and they have different *kinds* of answer:

1. **Mechanism choices** — *how* each piece of shared state is stored (per-window `GlobalStore` vs a cross-window leaked
   `Signal`). These are largely **settled by documented Dioxus behaviour**, and the codebase already aligns with it. Not
   much to decide.
2. **Pattern choice** — *how code reaches* that state: today via **global module accessors**
   (`crate::settings::row_limit()`, `crate::session::store()`, …). This is the **Service Locator** pattern, and it is a
   genuine **tradeoff**, not a settled win. The DI-vs-service-locator literature and general Rust global-state guidance
   both lean toward *dependency injection / threading when feasible*, primarily for **testability** and **explicit
   dependencies**.
3. **Shape** — *how state is grouped*. `AppState` is one `Signal` over ~25 unrelated fields, which is the Dioxus **"
   Avoid Large Groups of State" antipattern** [7] (coarse re-render, read/write-loop footguns, hard to reason about).
   Unlike (2) this isn't a tradeoff — the framework names it — so the fix (decompose, §8) is a straight win.

**Recommendation (a judgement call, laid out so you can overrule it):**

- **Keep the mechanism choices** — they're dictated by the framework (see §2).
- Treat the **pattern** question as **open and testability-driven**. Do the cheap, unambiguous wins now (document the
  model; one API-consistency fix). Then decide the service-locator-vs-DI question deliberately:
    - If you don't intend to unit-test the action layer soon → the current service-locator style is a defensible, common
      choice for an *application* (as opposed to a reusable library) and needs only documentation.
    - If testable action logic matters → the literature's answer is DI, and the **bounded-DI seam** (Phase 3, §7) is the
      principled, low-blast-radius way to get it here. I'd no longer call that "optional/rejected" — it's the path the
      field would point you to.
- **Full app-wide DI** still isn't warranted (large churn *and* it can't satisfy the cross-window constraint — see
  §4/Option B), but that's now a reasoned position rather than a decree.
- **Decompose `AppState`** (§8) regardless of the above — it's an independent, framework-endorsed cleanup, not blocked
  by the DI/tier calls. Split its ~25 fields by concern into focused signals/stores placed **as close to their consumers
  as the action layer allows**.

---

## 2. Grounding — what the framework and the field actually say

**Dioxus, on multi-window globals (the load-bearing fact).** From the official 0.7 context guide: *"GlobalSignals are
only global to one app - not the entire program. This means that in 'multitenant' environments like
server-side-rendering and multi-window desktop, every app gets its own independent global signal value."* [1] So:

- A `static GlobalStore`/`GlobalSignal` (`SESSION`, `RUNS`, `OVERLAYS`) is **per-window** — correct for per-window
  state, and it's exactly how the code uses them. (`OS_DARK` was one of these until it moved to the cross-window
  `SHARED` tier — see §4 — because it's a programme-wide value.)
- Cross-window state (settings) **cannot** be a global signal. The Dioxus-community answer for multi-window sharing is
  an out-of-band mechanism *outside* the reactive system — "shared Rust data structures with synchronization primitives
  (like `Arc` and `Mutex`)". [2] Strata's leaked-`Signal`-in-`thread_local!` is a single-threaded variant of that idea;
  because every window is on the one UI thread it needs no `Mutex`, and unlike `Arc<Mutex<T>>` it stays *reactive*.
  That's a reasonable, arguably better-fit choice — but note it's the app's invention, not a blessed Dioxus pattern.

**Dioxus, on when globals are OK.** The state tutorial: global state "can be very ergonomic if your state is truly
global, but you shouldn't use it if you need state to be different for different instances of your component.
**Libraries should generally avoid this to make components more reusable.**" [3] Strata is an application and settings
are genuinely global, so the "libraries should avoid" caveat doesn't bite — but this is an endorsement of *global
signals for app state*, not of the service-locator *access* pattern specifically.

**The field, on Service Locator vs DI.** What Strata's `crate::module::reader()` accessors are, in pattern terms, is
Service Locator / ambient global. The documented tradeoffs [4][5]:

- **Against (service locator):** dependencies are *hidden* (a function's signature doesn't reveal it reads settings);
  tests must configure a global registry before each test and tear it down after, creating **test-order coupling** and
  heavier setup; resolution is at runtime, so wiring mistakes surface late.
- **For (DI):** explicit dependencies; trivial mocking (pass a fake); compile-time checking; looser coupling. "If
  performance and testability are priorities, Dependency Injection is often the better choice."

**Rust, on global state.** General guidance leans the same way: *"prefer dependency injection when feasible... thread
state manually throughout your program when feasible."* And on the specific tool Strata uses: `thread_local!` "can
provide a safe way to handle global state without the need for synchronization" for single-threaded apps, with the
caveat that its **drop semantics are platform-dependent**. [6] (Strata leaks the signals, so drop never runs — the
caveat is sidestepped, but worth knowing.)

**Net.** The framework facts *support* Strata's mechanism choices. The general architecture literature does **not**
bless the service-locator access pattern as "right" — it frames it as an ergonomics-vs-testability tradeoff and leans
DI-when-feasible. My earlier "reject DI" framing didn't reflect that; this version does.

---

## 3. The Dioxus 0.7 state model (primitives in play)

- **`Signal<T>`** — `Copy` handle to reactive state, owned by its creating scope, dropped with it (unless leaked).
  `.read()` in a reactive scope subscribes; `.peek()` reads without subscribing; `.write()` mutates + notifies.
- **`use_context_provider` / `use_context`** — DI *within one VirtualDom's component tree*; `use_context` only resolves
  **inside a component** (it walks the provider chain). Does not cross VirtualDoms. [1]
- **`GlobalSignal`/`GlobalStore`** (`Signal::global` / `Global::new`) — a `static` Dioxus resolves **per-VirtualDom**
  (`.resolve()` = this window's instance). Per-window, reachable from non-component code inside a VirtualDom's runtime,
  **not** process-global. [1]
- **`Signal::leak` / `leak_with_caller`** — a `Signal` with no owner scope: never dropped, not bound to a runtime,
  shareable across VirtualDoms and readable without one. The only escape hatch for true cross-window sharing.
- **`dioxus-stores` `Store<T>`** — `#[derive(Store)]` → per-field lenses for fine-grained reactivity. **Write through
  lenses**, never a coarse `.write()` (a coarse write leaves lens subscribers stale — the historical tab-switch bug).

**Environment:** each window is its own VirtualDom; all windows run on the one macOS UI thread (so a `thread_local!` is
process-global and single-threaded); the action layer (`dispatch(state, action)` → `action::{query,tab,…}`) is plain
functions, not components, so it **cannot** use `use_context`.

**Two hard constraints any design must meet:**

- **C1** — the non-component action/engine layer must reach shared state without a component scope.
- **C2** — settings must be *the same value across every window*, updating everywhere at once (a per-VirtualDom global
  can't do this).

---

## 4. Current architecture (inventory)

| State                                                                                                                 | Mechanism                                                                                              | Scope                           | Reached by action layer via | Forced by                                                                                                                                           |
|-----------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------|---------------------------------|-----------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------|
| `AppState` — a **god-struct** of ~25 fields (engine `cmd_tx`, `project`, layout, drags, status, log, results bits, …) | `Signal<AppState>` (`use_context_provider` per `ProjectRoot`) **and threaded** as `dispatch(state, …)` | Per-window                      | explicit param (DI'd)       | ⚠️ **"Avoid Large Groups of State" antipattern [7] — decompose, see §8**                                                                            |
| `SESSION` — durable workspaces/tabs                                                                                   | `static GlobalStore<Session>` + `session::*`                                                           | Per-window (per-VirtualDom) [1] | `session::store()` global   | C1 + per-entry lens reactivity                                                                                                                      |
| `RUNS` — heavy ephemeral query output by id                                                                           | `static GlobalStore<HashMap>` + `runs::*`                                                              | Per-window                      | `runs::*` globals           | C1 + cheap persistence                                                                                                                              |
| `OVERLAYS` — overlay visibility                                                                                       | `static GlobalStore` + `overlays::*`                                                                   | Per-window                      | `overlays::*` globals       | C1 + UI-only concern                                                                                                                                |
| `settings::SHARED` — user `Settings` (`applied` + live `theme`)                                                       | leaked `Signal`s in `thread_local!` + `settings::*`                                                    | **Cross-window**                | `settings::*` globals       | **C2** (no framework global can cross windows) [1][2]                                                                                               |
| `OS_DARK` — OS appearance                                                                                             | leaked `Signal<bool>` in `settings::SHARED`, behind `os_dark()`                                        | **Cross-window**                | `settings::os_dark()`       | ✅ **moved** — programme-wide runtime fact feeding the same `effective_theme` as `theme`, so it shares that tier (was a per-window `GlobalSignal`). |

**Observation:** at the *public-API* level this is already ~90% one consistent pattern — module reader/mutator functions
hiding the storage. The storage *varies* only along the scope axis (per-window vs cross-window), which is
framework-forced. (`OS_DARK` was the lone API inconsistency — a bare `pub static` — now wrapped behind
`settings::os_dark()` **and** promoted into the cross-window `SHARED` tier, since it's a programme-wide value feeding
the same `effective_theme` as `theme`. ✅)

**Options for the pattern question:**

- **A — status quo, undocumented.** The *why* isn't written down, so the next contributor may "fix" the thread-local
  into a `GlobalStore` and silently break C2. Reject (do at least the docs).
- **B — full app-wide DI** (thread an `AppCtx`/`Services` through the action layer *and* replace component reads with an
  injected context). Gets DI's benefits, but: large churn, **and it does not solve C2** — an injected settings still has
  to wrap the same leaked signal underneath (a per-VirtualDom context can't be cross-window). Cost up, core constraint
  unmoved. **Reject.**
- **C — per-window settings context for components** (`use_context::<SettingsHandle>()` instead of
  `settings::row_limit()`). The context can only wrap the same leaked signal, and the **action layer still can't use
  `use_context`** (C1), so the globals stay anyway → *more* layers, not fewer. **Reject.**
- **D — document + consistency + a narrow test seam** (Phase 1–2). Near-zero churn; keeps the framework-correct
  mechanisms; buys the docs and (cheaply) testability. **Adopt as the floor.**
- **E — bounded DI at the dispatch seam** (Phase 3). Thread a small `Cx { state, settings, engine }` through `dispatch`/
  `run` only; components untouched; test impl of `settings` is a fixed `Settings`. This is the literature's DI answer
  scoped to exactly the layer that benefits, sidestepping B's churn and C's redundancy, and it *doesn't* fight C2 (prod
  impl wraps the leaked signal). **Adopt if/when testable action logic is a priority** — this is the principled upgrade,
  not a rejected afterthought.

---

## 5. Best-practice conventions (make these the written rules)

- **State lives in one owner module, behind reader/mutator functions.** Callers never touch storage. (Now true
  everywhere, incl. `OS_DARK`.)
- **Never put cross-cutting/overlay state in `AppState`.** settings→`settings`, overlays→`overlays`, heavy output→
  `runs`, durable tabs→`session`.
- **Pick storage from the scope axis** (framework-forced): cross-window ⇒ leaked `Signal` in `thread_local`; per-window
  reactive ⇒ `GlobalStore`/`GlobalSignal` behind module fns.
- **Don't group unrelated state in one signal** ("Avoid Large Groups of State" [7]): a `Signal<T>` is right only for a
  *small, cohesive* struct. Split by concern into focused signals/stores, each placed at the **tightest scope its
  consumers share** — component-local if nothing outside the component (and the action layer) touches it; a focused
  per-window module store otherwise. See §8.
- **Stores: write through lenses**, never coarse `.write()`.
- **Read discipline:** components `.read()` (subscribe); action layer `.peek()`/plain reads. Never hold a read guard
  across a write to the same signal (copy out first) — the `AlreadyBorrowed` panic.
- **Mutators own persistence** (only the owning module writes to disk, only where documented).
- **Cross-window fields follow the `applied`/live split** (immediate un-persisted preview vs committed-on-Save).
- **Pre-warm** any lazy cross-window handle from each root's mount (`settings::init()`), so a non-component read never
  races cold init.

---

## 6. "Where does new state go?" (decision tree)

```
Same value across ALL windows (one user-global value)?
├─ YES → cross-window: leaked Signal(s) in thread_local, behind module fns.
│         immediate vs preview-then-commit?
│         ├─ immediate           → one shared signal
│         └─ preview-then-commit → split live + applied (theme/settings)
└─ NO  → per-window. Durable/persisted session data?
          ├─ YES → `session` store (serde; lens writes) → session.json
          └─ NO  → heavy/ephemeral view output (result pages, running flags)?
                   ├─ YES → `runs` store (keyed by WorkspaceId)
                   └─ NO  → pure overlay visibility?
                            ├─ YES → `overlays` store
                            └─ NO  → AppState (threaded) or a focused GlobalSignal
                                     behind module fns
```

---

## 7. Implementation plan (phased, each independently shippable)

**Phase 1 — formalise (docs + one fix). Low risk.**

- Write the shared-state model up (§2–§6): taxonomy table, the two constraints, the decision tree, the rules — as
  `docs/architecture/shared-state.md`, cross-referenced from the owner modules' banners.
- ✅ **Done:** `OS_DARK` wrapped behind `settings::os_dark()` **and** re-tiered — moved into the cross-window `SHARED`
  context as a third leaked signal (runtime-only, never persisted), because it's a programme-wide value feeding the same
  `effective_theme` as `theme` (not a per-window fact). One window's `ThemeChanged` now updates it for all.
- Make the single-thread + init-order + leak invariants explicit in `settings.rs` docs.

**Phase 2 — test seam. Low risk, high leverage.** *(This is the cheap answer to the literature's #1 gripe with service
locators — testability — without adopting DI.)*

- Add `#[cfg(test)] settings::set_shared_for_test(Settings)` (+ reset) that seeds `applied` with a fixed value. No prod
  behaviour change.
- Characterise the trickiest ambient reads: `projects::open_dir` honouring `open_pref`; `tab::close` honouring
  `confirm_close_running` — locking behaviour before any future refactor.

**Phase 3 — bounded DI at the dispatch seam. Do when testable action logic is a priority.**

- Introduce `Cx { state: Signal<AppState>, settings: SettingsReader, engine: EngineTx }`; change `dispatch`/`run`
  /handlers to take `cx`. Components unchanged.
- `SettingsReader` prod impl wraps the leaked signal (C2 preserved); test impl is a fixed `Settings`. `EngineTx`
  replaces reaching `state.cmd_tx` for sends.
- Contained to `action::*`. This converts hidden ambient reads into explicit, mockable dependencies — the DI benefit,
  scoped to the layer that wants it.

**Out of scope / non-goals:** replacing per-window `GlobalStore`s with threaded handles; a component-facing settings
context (Option C); any locking (`Arc<Mutex>`) given the single UI thread; multi-thread UI.

---

## 8. `AppState` decomposition (the "Avoid Large Groups of State" fix)

`AppState` is one `Signal` over ~25 unrelated fields, which is the antipattern by name [7]: any `.write()` re-renders
every reader, hot fields (resize/tab-drag on mousemove) re-render the whole window, and the `.peek()`-vs-`.read()` care
already sprinkled through the action layer is the "easy to accidentally read+write the whole object" footgun. A single
`Store<AppState>` would *not* fix it — it keeps the god-struct shape. The fix is **decomposition by concern**.

**Placement rule (grounded per field by who touches it):** *put each piece at the tightest scope its consumers share —
but no tighter than the action layer can reach.* Constraint C1 (the action layer can't use `use_context`) means anything
the `action::*` / engine layer mutates cannot be pure component-local; it needs a focused per-window module store/signal
(the `overlays`/`runs`/`session` pattern). Only state nothing outside one component *and* the action layer touches can
hoist all the way down.

| Concern                        | Fields                                                                                                                       | Consumers (verified)                                                                                                                 | Proposed home                                                                                                                                                              | Scope / note                                                                                                                                   |
|--------------------------------|------------------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------|
| Catalog filter                 | `filter`                                                                                                                     | `ui::sidebar` (read); `action::catalog::set_filter` (only writer)                                                                    | **`ui::sidebar`-local `use_signal`** — drop the `SetFilter` action                                                                                                         | **Component-local.** The one true hoist-down (nothing else reads it).                                                                          |
| Column-inspector selection     | `selected_col`                                                                                                               | `ui::inspector`, `ui::sidebar` (read); `action::catalog` (clears on drop)                                                            | **`crate::inspector` per-window `Signal`**                                                                                                                                 | Focused signal — *not* a subtree context (action layer touches it → C1).                                                                       |
| Pager UI                       | `page_size_open`                                                                                                             | pager component; `action::overlay` (close-all); `action::query::paging`                                                              | **fold into `crate::runs`** (or a small results-UI signal)                                                                                                                 | Focused signal — action-layer-touched (C1).                                                                                                    |
| Layout / panels                | `sidebar_open`, `inspector_open`, `sidebar_w`, `inspector_w`, `editor_h`, `log_h`, `resizing`                                | shell panels (read); resize handles + `action::panel` (write); `app.rs` root (`resizing`→cursor/class); grid cells (read `resizing`) | **`crate::layout` per-window `Store<Layout>`** (lens writes)                                                                                                               | **Biggest reactivity win.** `resizing`/sizes are HOT (mousemove) — isolating layout stops editor/grid/sidebar re-rendering on a resize.        |
| Tab drag + reopen              | `tab_drag` (HOT), `closed_tabs`                                                                                              | `ui::workbench::tabs`; `app.rs` pointer driver; `action::tab`                                                                        | **fold into `crate::session`** as transient companions                                                                                                                     | Isolate `tab_drag` so a reorder mousemove doesn't re-render the workbench.                                                                     |
| Status bar                     | `status_text`, `status_kind`                                                                                                 | status bar (read); many writers via `set_status` (action + engine)                                                                   | **`crate::status` per-window `Signal<{text,kind}>`** behind `set_status`                                                                                                   | Keep the pair in one struct so they can't drift.                                                                                               |
| Event log / drawer             | `log`, `log_open`, `log_tab`, `next_log`                                                                                     | drawer UI; action + engine (append)                                                                                                  | **`crate::log` (events) per-window store**                                                                                                                                 | Distinct from `crate::diagnostics` (Problems).                                                                                                 |
| Engine session                 | `cmd_tx` (handle, non-reactive), `functions` (reactive; `crate::sql` reads), `next_req` (counter; only `action::query::run`) | action layer (send/alloc); sql language service (functions)                                                                          | **`crate::engine` per-window session**: `cmd_tx` a stored sender, `functions` a `Signal<FunctionCatalog>`, `next_req` a `Cell<u64>`                                        | Pairs with the DI `Cx.engine` (§7 Phase 3) — do them together.                                                                                 |
| Project domain                 | `project`, `project_path`                                                                                                    | sidebar catalog, history drawer, action layer (register/save/run), persistence                                                       | **`crate::project` per-window `Store<Project>`** (mirrors `crate::session`)                                                                                                | **Largest extraction; do last, on its own.** Lenses give per-collection reactivity (catalog vs history vs saved-queries update independently). |
| Reassess (not a straight move) | `type_color_cells`; `recent_projects`                                                                                        | grid only, set once; recents mirror `crate::config`                                                                                  | `type_color_cells` → **Settings tier** (a display preference like zebra/density); `recent_projects` → **consolidate with `crate::config`** (config is the source of truth) | Neither belongs in per-window `AppState`.                                                                                                      |

**Migration order** (value-first, low-blast-radius-first; each step shrinks `AppState` and is independently shippable):

1. **`crate::layout` store** — biggest reactivity win, self-contained (shell + `action::panel`).
2. **Isolate `tab_drag`** — hot, self-contained.
3. **`filter` → sidebar-local** — trivial, and deletes an action.
4. **`status`, `log`, `inspector` selection, pager** — small focused modules; mechanical.
5. **`crate::engine` session** (`cmd_tx`/`functions`/`next_req`) — do with the DI `Cx.engine` (Phase 3).
6. **`project` → `Store<Project>`** — largest; its own branch.
7. **Reassess** `type_color_cells`→Settings, `recent_projects`→config.

**End state:** `AppState` shrinks toward empty. What (if anything) remains is a thin bundle of genuinely cross-cutting
per-window handles — exactly the `Cx` the DI seam threads (§7). So decomposition and the DI direction converge on the
same place.

## 9. Risks & open questions

- **Cross-window fields will multiply** (Connections/config, handoffs 25–27). The `applied`/live split + `settings::`
  function pattern is the template; the decision tree exists to stop someone reaching for a `GlobalStore` for
  cross-window data (it will silently be per-window — the exact trap Dioxus's docs warn about [1]).
- **The service-locator-vs-DI call is genuinely yours.** The evidence says: framework-correct mechanisms either way; DI
  is the field's default *when testability matters*; service locator is fine for app-global config with no test
  pressure. Phase 3 is the hedge — you can defer it and adopt it later without redoing Phase 1/2.
- **`OS_DARK` re-tier** (now in `SHARED`, cross-window) — verify a live OS light/dark switch still re-themes *every*
  open window: one window's `ThemeChanged` writes the shared value and all subscribers re-derive `effective_theme`.
- **Per-project settings override** (if ever wanted) would break the "settings are process-global" assumption and would
  itself justify revisiting DI — not on the roadmap today.

---

## Sources

1. Dioxus 0.7 — Global Context (context is per-tree; GlobalSignals are per-app/per-window in multi-window
   desktop): https://dioxuslabs.com/learn/0.7/essentials/basics/context/
2. Dioxus multi-window state sharing needs an out-of-band mechanism (Arc/Mutex) — Dioxus docs/community +
   `dioxus-desktop` `new_window`: https://docs.rs/dioxus-desktop/
3. Dioxus 0.7 — Adding State / state tutorial (globals ergonomic for truly-global state; libraries should
   avoid): https://dioxuslabs.com/learn/0.7/tutorial/state/
4. Dependency Injection vs. Service Locator — Baeldung on Computer Science (hidden deps, test-order coupling, compile-
   vs run-time): https://www.baeldung.com/cs/dependency-injection-vs-service-locator
5. Service Locator Pattern — DevIQ: https://deviq.com/design-patterns/service-locator-pattern/
6. Idiomatic global variables in Rust (prefer threading/DI when feasible; `thread_local!` for single-threaded,
   platform-dependent drop): https://www.sitepoint.com/rust-global-variables/
7. Dioxus 0.7 — Antipatterns, "Avoid Large Groups of State" (split a big state struct into smaller signals/stores;
   god-structs cause re-render, loops, poor reasoning): https://dioxuslabs.com/learn/0.7/guides/tips/antipatterns
