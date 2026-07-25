# P4-01 · Multi-window shell + shared state + native close

**Phase:** 4 · **Status:** 🟡 · **DEV_TASKS:** W1 / A8 · **Depends on:** — · **Unblocks:** P4-03, P4-10

## Goal
The plumbing for more than one OS window: app-wide shared state, window spawn/focus, and native
close handling.

## Current state
**Shared state is done** (build item 1): `crate::state::config` holds the app-global `AppConfig`
store — a global `RadioStation<AppConfig, ConfigChan>` (settings · recents · open-set) created in
`main` and shared into each window root by `use_share_config`; `write_config` is the single
mutate-and-persist path; `use_open_project` keeps a window's project in recents + the open-set for
its lifetime. Theme is pure derived state off the `Settings` channel (`use_strata_theme`).

**The window model is done** (items 2 · 4 · 6, and item 3's close routing), built with P4-02 because
the launcher is the surface that needs it — `crate::platform::windows`:

- `WindowRegistry` — a second app-global (`State<Windows>`): the **live** id map (winit `WindowId` →
  launcher / project folder), as opposed to the config store's *persisted* open-set. Each window
  root joins it via `use_register_window` for its lifetime.
- `open_project` / `open_launcher` — focus-if-open, else `launch_window`. The launcher is
  single-instance by construction.
- `quit` / `quit_windows` — ⌘Q (`Command::Quit`, new) and menu Quit close **every** window and set
  a `QUITTING` flag that suppresses `use_open_project`'s open-set removal, so the next launch
  reopens exactly what was on screen.
- `close_this_window` — ⇧⌘W (`Command::CloseProject`, rebound off ⌘Q), File ▸ Close Project and the
  red button close **one** window, drop it from the open-set, and put the **launcher** up when it
  was the last. The `on_close` hook can't do that itself (it's `Send`, and the app would exit
  first), so it vetoes with `Veto::Launcher` and the UI does it — the same bridge as T2's confirm.
- `main`'s `startup()` opens one window per project in the restore set (`with_window` is
  repeatable), else the launcher; argv\[1\] wins outright. Every open path normalizes through
  `resolve_project_folder`, so naming a project's own `.strata` dir opens the project.
- The **menubar follows the focused window** (`menu::use_file_menu`): File ▸ Open… · Open
  Recent ▸ (live off the recents) · Close Project, the last pulled from the menu entirely
  while the launcher is focused. Needed one fork change — `MenuGetter` no longer requires
  `Send` (`alexparlett/freya@cec9e6ae`), since muda's own handles aren't `Send` and the
  builder never leaves the main thread, so an app can keep item handles and update them.
- `Command::Quit` (⌘Q) and `Command::OpenProject` (⌘O) are new; `CloseProject` moved to ⇧⌘W.

## Build (plan §4/§6)
1. ✅ **Shared singletons** — one app-global store (see Current state) rather than three separate
   `create_global` signals: `AppConfig` is what the file holds, so one struct means one load, one
   write, and no field clobbered by a partial save; `ConfigChan` supplies the granularity.
   Per-window model stays in each window's Radio station.
2. ✅ **Window management** (`platform/windows.rs`, see Current state): spawn / focus-if-open /
   close, for the launcher and project windows. Settings and Export join it with P4-03 / P4-10 —
   settings needs the single-instance treatment `open_launcher` already models.
3. 🟡 **Native close handling**: `winit CloseRequested` (no objc) routes through the window's
   `on_close` hook, which now vetoes for both the T2 confirm and the last-window→launcher rule.
   Remaining: the Dock-icon Quit still `terminate:`s un-vetoed (winit 0.30 exposes no
   `applicationShouldTerminate` — see P6-02).
4. ✅ Each window is a Freya `App` root under `apps/<window>/` (symmetric; no project-window
   special case): `apps/project/` and `apps/launcher/`.
5. **Unrecoverable per-window error → graceful close.** When a window hits a fault it can't recover
   from — today: a defs or session file that won't load (`open_project` / `use_init_session` in
   `apps/project/state/hooks.rs`) — it should **close that window** through
   `platform::close_this_window` (which already lands on the launcher if it was the last). Those
   paths still **`panic!`** as a loud interim placeholder (deliberately *not* a silent fallback — a
   project can't exist without a root). The *launch* half is handled: `main`'s `startup()` reports
   and skips a folder that won't resolve, and the launcher reports rather than opening a doomed
   window.

6. ✅ **Quit vs. close, and reopen-on-startup** — see Current state. `Command::Quit` (⌘Q) is now its
   own command, distinct from `Command::CloseProject` (⇧⌘W).

## Acceptance
- [x] A change to shared settings/theme is seen by every open window at once.
- [x] ⌘Q with N windows open restores those N on next launch; deliberately closing them all doesn't.
- [x] Windows spawn/focus/close; native close (red button / ⌘Q / menu) routes through the confirm.
- [ ] An unrecoverable defs/session restore error closes that window (→ launcher if it was the last),
      replacing the interim `panic!` in `apps/project/state/hooks.rs`.

## Freya / references
- Plan §4 (client/server split; `create_global` for singletons), §6 (multi-window), §8 (native menu
  is a separate open item). state-arch (per-window Radio vs global). `platform/` module (plan §3).
