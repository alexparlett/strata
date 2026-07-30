# P4-14 · Session persistence + autosave

**Phase:** 4 · **Status:** ✅ `[core ✓ IO]` · **DEV_TASKS:** project lifecycle · **Depends on:** P4-13

> **✅ Shipped:** `.strata/session.json` **load + debounced autosave** of the open tabs, **query
> history**, **window geometry** and the **panel layout**.
>
> **Layering (mirrors the project store).** The **serde vocabulary lives in `strata_model`**
> (`session.rs`: `TabId` / `Origin` / `ResultsView` / `TabSnapshot` / `WindowGeom` / `SessionSnapshot`;
> `history.rs`: `HistoryEntry`) — pure leaves next to `TableDef` etc. **`strata-core::project` owns
> the IO, all three families now concrete + symmetric**: `load_defs`/`save_defs`,
> `load_session`/`save_session`, `load_history`/`append_history`. The frontend `state/` holds only the
> live stores + the Freya wiring (no `persist.rs`); call sites import the vocabulary straight from
> `strata_model` (no re-exports through `state`).
>
> - **Session tabs.** `strata_core::project::{load_session, save_session}` (missing → `Ok(None)`;
>   corrupt → `Err`). Conversions are `SessionState::{snapshot, from_snapshot}` in `state/session.rs`
>   (with `SessionState`, like `ProjectState::{defs, from_defs}`). The synthetic **`Chan::Persist`**
>   fan-in (every structural / buffer / view-mode write derives it, so one debounced effect observes
>   them all; `Request` / `Diagnostics` excluded as ephemeral) drives `use_autosave` (in `state/hooks.rs`
>   with the other root wiring). Load wired into `use_init_session` (pulls the root from the
>   `ProjectState` store — a project always has a root). Cursor/scroll are intentionally out
>   (state-arch §12); a restored bound tab comes back clean (dirty-across-restart not preserved).
> - **History** (`state/history.rs`). Its own satellite (`HistoryCtx = State<History>`), **not** on
>   `SessionState` / `ProjectState`. Persisted append-only to **`.strata/history.jsonl`** (core
>   `append_history` / `load_history`, rotated to a keep-last-N window). `History::load` builds it;
>   `use_init_history` (in `hooks.rs`) provides it. Published from the results pane via
>   `use_history_recording` when a run settles `Ok(Rows)` — successful data runs only (a failed /
>   cancelled `Err` or an Explain `Plan` never records), deduped by `RunId` so a tab-switch re-mount
>   can't double-log. The **view** (FEATURES §12) is later — this is the plumbing.
> - **Window geometry.** `SessionSnapshot.window` (logical `WindowGeom`). Restored at window
>   creation (`ProjectApp::window` reads the session file, seeds `with_size` + `with_position`);
>   captured live by autosave from `Platform`. Needed a **Freya fork change**: a new
>   `Platform::window_position` signal (updated on `WindowEvent::Moved`) — position wasn't exposed
>   at runtime. **The fork submodule must be committed + pushed** (see the fork note below).
> - `project.json` def-persistence + per-tab dirty were already done (P4-13 / P2-16). Resolving the
>   project root moved to `resolve_launch_root` (called in `ProjectApp::window`, before the window
>   opens) so geometry can seed the window.
>
> **Verified** by unit tests (IO round-trips, snapshot round-trip, history dedup) **and a runtime
> smoke test**: a seeded project restored its tabs + active tab + window size/position, and autosave
> rewrote `session.json` with the live geometry + tabs.
>
> **Layout — landed with P3-01, and *not* as the satellite this task expected.** The deferral
> above assumed panel layout would arrive as its own store (state-arch §8) to be folded in later.
> It didn't: P3-01 put it on `SessionState` itself as a `Layout` field under two `Persist`-deriving
> channels (`Chan::Layout` = structure, `Chan::LayoutSize` = sizes), so it rode the autosave this
> task had already built and needed no fold at all. `SessionSnapshot.layout` carries it
> (`strata-model::session`), `SessionState::{snapshot, from_snapshot}` convert it, and
> `snapshot_round_trips_layout` / `snapshot_round_trips_an_expanded_drawer` cover it — including
> the wrinkle that an *expanded* drawer restores its restore-height rather than its expanded one.
>
> The generalisation is worth keeping: the `Chan::Persist` fan-in is what made this free. A new
> field on `SessionState` whose channel derives `Persist` is persisted the moment it exists, with
> no new writer and nothing to remember — which is why the "await its store" plan was the more
> expensive of the two and the cheaper one happened by itself.
>
> **⚠️ Fork note:** `crates/freya` (submodule) gained `Platform::window_position` +
> `WindowEvent::Moved` handling (`freya-core/src/platform.rs`, `freya-winit/src/{window,renderer}.rs`,
> `freya-testing/src/lib.rs`). Local path-dep builds pick it up, **but the submodule + gitlink must be
> committed and pushed to the fork remote** or CI / a fresh clone breaks.

## Goal
Keep `.strata/session.json` (and the `project.json` defs) in sync as the user works.

## Current state *(as this task was picked up)*
Not built (`session.rs`: "Persistence — a serde snapshot — is a later slice"). `SessionState` holds
live `QueryTab`s whose `CodeEditorData` **isn't serde**, so persistence goes through a snapshot —
which is why `SessionSnapshot` exists at all, and still the reason it does.

> **Constraint (agreed 2026-07-23): history gets its own satellite store.** The Dioxus app
> kept run history *on the Project store* but persisted it *in `session.json`* — don't copy
> that straddle. History is a small satellite (state-arch §8): its own per-window store,
> persisted with the session file (local, gitignored), never on `ProjectState`.
>
> Also inherited from the P4-13-internals refactor: the model types are now **pure defs**
> (`TableDef`/`ViewDef`/`SavedQuery` — no `#[serde(skip)]` runtime fields), so the session
> snapshot serializes defs and *only* defs; registration state (`Reg<T>` on the store rows)
> is never persisted. `SavedQuery` identity is its `id: Uuid` (`Origin::SavedQuery(Uuid)`);
> view identity is its name.

## Build (state-arch §4/§5)
1. **`SessionSnapshot`** — a serde view of `SessionState`: each tab's **text + origin + language**,
   the order / active / closed stack, layout, inspector selection, per-tab view intent, and history.
2. **Autosave** — a debounced `use_side_effect` writes `session.json` on change (tabs, layout,
   history, window). Local-only (gitignored).
3. **project.json** — written on catalog/def changes (view create/drop, saved-query, register/
   deregister): the durable, shareable **defs**, separate from the ephemeral session.
4. **Dirty tracking** — a tab is dirty via `Origin` + content hash (`is_dirty = editor.is_edited()`).
5. ✅ **Known bug fixed:** editing a view's SQL + ⌘S updates the view. `actions::save_tab` routes by
   `Origin` — `Origin::View(name)` goes to `save_view` (re-issuing `CREATE OR REPLACE VIEW`),
   `Origin::SavedQuery(uuid)` to `save_query` — so neither can produce the other's artifact.

## Acceptance
- [x] Edits / tabs / history / window geometry **and layout** persist (debounced) and restore on
      reopen.
- [x] Catalog def changes persist to `project.json`; dirty state tracks per tab. *(P4-13 / P2-16.)*

> **One thing this task deliberately did not make resilient: the failure path.** Both session
> writers (the debounced autosave and the final save on close / re-root) report a failed write
> through `tracing` alone, so a `session.json` that can't be written loses the session silently —
> the final save worst, since there is no later write to make up for it and the window is already
> going away. That is **P4-15**'s, listed in its table; don't fix it here.

## Freya / references
- state-arch §4 (durable client model), §5 (persistence). Core `.strata/` IO. Memory
  `project-persistence`. DEV_TASKS Known bugs (the ⌘S-on-a-view bug).
