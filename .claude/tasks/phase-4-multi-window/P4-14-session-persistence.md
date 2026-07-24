# P4-14 · Session persistence + autosave

**Phase:** 4 · **Status:** 🟡 `[core ✓ IO]` **session tabs + history + window geometry done; layout awaits its store** · **DEV_TASKS:** project lifecycle · **Depends on:** P4-13

> **🟡 Shipped:** `.strata/session.json` **load + debounced autosave** of the open tabs, **query
> history**, and **window geometry**.
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
> **Deferred:** persisting **layout** — that satellite store (state-arch §8) doesn't exist in the
> Freya app yet; fold it into `SessionSnapshot` when it lands.
>
> **⚠️ Fork note:** `crates/freya` (submodule) gained `Platform::window_position` +
> `WindowEvent::Moved` handling (`freya-core/src/platform.rs`, `freya-winit/src/{window,renderer}.rs`,
> `freya-testing/src/lib.rs`). Local path-dep builds pick it up, **but the submodule + gitlink must be
> committed and pushed to the fork remote** or CI / a fresh clone breaks.

## Goal
Keep `.strata/session.json` (and the `project.json` defs) in sync as the user works.

## Current state
Not built (`session.rs`: "Persistence — a serde snapshot — is a later slice"). `SessionState` holds
live `QueryTab`s whose `CodeEditorData` **isn't serde**, so persistence goes through a snapshot.

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
5. ⚠️ **Known bug:** editing a view's SQL + ⌘S must **update the view** (route by `Origin`), not save
   a new saved-query — pairs with P2-16.

## Acceptance
- [x] Edits / tabs / history / window geometry persist (debounced) and restore on reopen. *(Layout
      pending its satellite store — see the Shipped note.)*
- [x] Catalog def changes persist to `project.json`; dirty state tracks per tab. *(P4-13 / P2-16.)*

## Freya / references
- state-arch §4 (durable client model), §5 (persistence). Core `.strata/` IO. Memory
  `project-persistence`. DEV_TASKS Known bugs (the ⌘S-on-a-view bug).
