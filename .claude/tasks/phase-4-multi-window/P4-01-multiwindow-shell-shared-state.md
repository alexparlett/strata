# P4-01 · Multi-window shell + shared state + native close

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** W1 / A8 · **Depends on:** — · **Unblocks:** P4-02, P4-03, P4-10

## Goal
The plumbing for more than one OS window: app-wide shared state, window spawn/focus, and native
close handling.

## Current state
Only the project window exists. `main.rs` launches it. No other window roots.

**Shared state is done** (build item 1): `crate::state::config` holds the app-global `AppConfig`
store — a global `RadioStation<AppConfig, ConfigChan>` (settings · recents · open-set) created in
`main` and shared into each window root by `use_share_config`; `write_config` is the single
mutate-and-persist path; `use_open_project` keeps a window's project in recents + the open-set for
its lifetime. Theme is pure derived state off the `Settings` channel (`use_strata_theme`).

## Build (plan §4/§6)
1. ✅ **Shared singletons** — one app-global store (see Current state) rather than three separate
   `create_global` signals: `AppConfig` is what the file holds, so one struct means one load, one
   write, and no field clobbered by a partial save; `ConfigChan` supplies the granularity.
   Per-window model stays in each window's Radio station.
2. **Window management** (`platform/`): spawn / focus-if-open / close a window (project, launcher,
   settings, export). Single-canonical instances where required (settings).
3. **Native close handling**: intercept `winit CloseRequested` (no objc) → the themed close-while-
   running confirm hooks in here (shares P2-20's close path).
4. Each window is a Freya `App` root under `apps/<window>/` (symmetric; no project-window special case).
5. **Unrecoverable per-window error → graceful close (owns the mechanism for P4-13/P4-14).** When a
   window hits a fault it can't recover from — today: a project folder that won't open / a defs or
   session file that won't load (`open_project` / `use_init_session` in
   `apps/project/state/hooks.rs`) — it should **close that window**; if it was the **last** window,
   open the **launcher** (P4-02) instead; otherwise the other windows just stay put. Until this lands,
   those paths **`panic!`** as a loud interim placeholder (deliberately *not* a silent fallback — a
   project can't exist without a root). Wire them through this close path when it exists.

6. **Quit vs. close, and reopen-on-startup.** `use_open_project` drops a project from the open-set
   when its window closes — but quitting closes windows too, so the persisted set ends up empty and
   "Reopen projects on startup" restores nothing. The Dioxus app got the split for free (its Quit was
   a raw `terminate:` that never delivered a per-window close; a *deliberate* close did remove the
   entry, so closing everything by hand correctly meant "open the launcher next time"). Ours routes
   Quit through the same close path on purpose, to keep the close-while-running confirm — so quit-all
   must mark itself and suppress the per-window removal. `create_global_config` already hands `main`
   the restore list (`reopen`); this item is what consumes it, spawning a window per path (filtering
   ones that no longer exist) when `reopen_on_startup` is set.

## Acceptance
- [ ] A change to shared settings/theme is seen by every open window at once.
- [ ] ⌘Q with N windows open restores those N on next launch; deliberately closing them all doesn't.
- [ ] Windows spawn/focus/close; native close (red button / ⌘Q / dock) routes through the confirm.
- [ ] An unrecoverable open/restore error closes that window (→ launcher if it was the last), replacing
      the interim `panic!` in `apps/project/state/hooks.rs`.

## Freya / references
- Plan §4 (client/server split; `create_global` for singletons), §6 (multi-window), §8 (native menu
  is a separate open item). state-arch (per-window Radio vs global). `platform/` module (plan §3).
