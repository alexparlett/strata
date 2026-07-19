# Freya port — Phase 0 overnight handoff

Blind session (no compiler). Everything below is **uncommitted working changes** — build,
fix the inevitable shakedown errors, then commit. Plan: `docs/FREYA_PORT_PLAN.md`.

## Done

1. **Plan revised** to `strata-model` + `strata-core` (two core crates), not one-per-module.
   Module ≠ crate; crates serve reuse, not a mirror of the module tree. (§2 of the plan.)

2. **`strata-model` extended** with the shared data types lifted out of the Dioxus store
   modules (they're vocabulary the `sql` service needs):
   - `Diagnostic` / `DiagSource` / `Severity` (from `diagnostics.rs`) — new `diagnostics` module.
   - `CatalogProfile` (from `profile.rs`) — new `profile` module (the *result*; the scan
     *logic* stayed in the app).
   - `RegStatus` / `CatalogTable` / `CatalogView` + the `de_partition_cols` deserializer
     (from `project.rs`) — appended to the `catalog` module.
   - Added a `serde` dependency (the catalog defs persist). Still no datafusion.

3. **Re-export shims** so the app's paths resolve unchanged (no call-site churn):
   - `diagnostics.rs`: `pub use crate::model::{Diagnostic, DiagSource, Severity};` (kept the store)
   - `profile.rs`: `pub use crate::model::CatalogProfile;` (kept `Slot`/`aggregates`/`decode`)
   - `project.rs`: `pub use crate::model::{CatalogTable, CatalogView, RegStatus};` (kept the store + persistence)

4. **`strata-core` created**, with `sql/` moved in (`git mv`) as its first module.
   - deps: `strata-model` + `datafusion` 54.
   - Repointed sql's imports at `strata_model` directly (`validate.rs` diagnostics;
     `symbols.rs` ColumnInfo + CatalogTable/View). Internal `crate::sql::*` refs are
     unchanged (they resolve within strata-core).
   - App: `mod sql;` → `pub use strata_core::sql;` so all 11 `crate::sql::*` refs
     (incl. `FunctionCatalog`) resolve unchanged. Added the workspace member + path dep.

Boundary types line up: the app passes `crate::project::CatalogTable`
(= `strata_model::CatalogTable` via the shim) into `strata_core::sql`, which expects exactly
that type. Same for `Diagnostic`.

5. **The rest of the framework-agnostic core moved into `strata-core`** (`git mv`, so history
   follows) — `util`, `plan/`, `config`, `profile`, `theme`, and the whole `engine/` family.
   Each is a **module** of `strata-core`, not its own crate. Repoints were minimal: only
   `crate::model::*` → `strata_model::*` (sql/engine data types) needed changing; every other
   moved-file `crate::…` ref (`crate::plan`, `crate::profile`, `crate::util`, `crate::sql`,
   `crate::engine::config`, `crate::theme`) already resolves **within** strata-core unchanged.
   - `theme` moved too (its only "coupling" was one stray autocomplete import,
     `use dioxus::html::completions::…::base;` — deleted; the file is otherwise pure serde +
     std). This resolves `config`'s `crate::theme::DEFAULT_THEME` default in-crate. The CSS
     generation (`css_for`) rides along for now — pure string work the Freya frontend just
     won't call (it'll build a Freya theme from the same JSON model instead).
   - `config`'s `Settings` **dropped its vestigial `#[derive(Store)]`** — the app holds
     settings as a whole-value `Signal<Settings>` (+ leaked signals), never a `Store`/lens,
     and never compares whole `Settings`, so no `Store`/`PartialEq` is needed.
   - App aliases (in `main.rs`): `pub use strata_core::{config, plan, profile, sql, theme, util};`
     so every `crate::<mod>::*` call site resolves unchanged.

6. **Engine handle split (all engine → core, incl. the handle).** `strata_core::engine::Engine`
   is now a **plain** connection object — `cmd_tx`/`evt_rx`/`next_req`/`functions` with
   *instance* methods `spawn(overrides)` / `send(&self)` / `next_req(&self)` / `functions(&self)`
   / `set_functions(&mut self)` / `take_evt_rx(&mut self)`. No Dioxus, no `Store`, no `Global`.
   `spawn` now **takes** the `datafusion.*` overrides as a param (was reading
   `crate::settings::engine_overrides()`, which is app state).
   - The app keeps a **thin reactive shim** at `src/engine.rs` (~70 lines, the only Dioxus
     part): a per-window `GlobalSignal<CoreEngine>` + a unit `struct Engine` whose static
     methods delegate to it — `send`/`next_req` via `.peek()` (no notify on the query hot
     path), `functions` via `.read()` (reactive), `set_functions`/`take_evt_rx` via `.write()`.
     The `command!` macro moved here too. So the app's `crate::engine::Engine::*` call sites,
     the macro, and `crate::engine::{config, serialize, Command, Event, TableSpec, TableMeta,
     purge_snapshot_root}` all resolve **unchanged** (the shim re-exports the two submodules +
     the protocol). The Freya app will hold the same `CoreEngine` via context/Radio.

## Likely first-build shakedown (all easy)

- **Unused-import / unused-dep warnings** in the shimmed modules (`diagnostics.rs`,
  `project.rs`) and possibly a now-unused `preferences`/`arboard` in the *app* Cargo.toml
  (that logic lives in core now). Warnings, not errors.
- **Intra-doc links** in the moved files still point at `crate::…` of the *app* (e.g.
  `[crate::runs]`, `[crate::project]`, `[crate::keymap]`, `[crate::model]` in the moved
  engine/config/theme files). Confirmed by grep to be **comment-only** — harmless to
  `cargo build`; only `cargo doc` warns. Neutralize later.
- The engine split touches behavioural wiring grep can't check — the `command!` macro
  expansion and the `apply_event` drain in `app.rs`. The shim preserves the exact call
  shapes, but this is the place to watch on first run.
- Anywhere the app *constructs* a moved type directly or reaches a submodule internal I
  didn't spot — fields/items are all `pub`, so at worst a path tweak.

## Follow-ups (not blockers)

- **`functions` as frontend state.** It sits on `CoreEngine` only so the language service can
  read it reactively (the shim bridges that). Cleaner: make `functions` pure frontend state
  fed by `Event::Functions`, leaving the core handle pure I/O. Deferred to keep this pass
  mechanical.
- **Clipboard in core.** `engine::serialize` carries `arboard` (`ClipboardWriter`). Pure and
  cross-platform, so it builds, but clipboard is arguably a frontend concern — revisit when
  wiring the Freya copy path.
- **Project persistence stays app-side** on purpose — it's a reactive `GlobalStore<Project>`
  (session.json autosave). Only its serde *definitions* (`CatalogTable`/`View`) moved (to
  `strata-model`). The *app-config* persistence (recents/prefs via `preferences`) did move,
  inside `strata_core::config`.
- Then delete the doc-link cruft and commit once green.
