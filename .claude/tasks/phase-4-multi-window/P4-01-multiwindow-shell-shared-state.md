# P4-01 · Multi-window shell + shared state + native close

**Phase:** 4 · **Status:** ✅ · **DEV_TASKS:** W1 / A8 · **Depends on:** — · **Unblocks:** P4-03, P4-10

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
3. ✅ **Native close handling**: `winit CloseRequested` (no objc) routes through the window's
   `on_close` hook, which now vetoes for both the T2 confirm and the last-window→launcher rule.
   The Dock-icon Quit veto is **P6-02's** (the winit 0.31 `applicationShouldTerminate` delegate
   migration, documented there in full — accepted platform behaviour until then), not this task's.
4. ✅ Each window is a Freya `App` root under `apps/<window>/` (symmetric; no project-window
   special case): `apps/project/` and `apps/launcher/`.
5. ✅ **Unrecoverable per-window error → graceful close.** The interim `panic!`s are gone:
   `ProjectRoot` runs the fallible IO (defs load/scaffold + session restore, one
   `state::hooks::open_project` returning `Result<Loaded, String>`) once per mount, and what it
   found decides the subtree — `ProjectLoaded` (the whole former body, stores built from the
   pre-loaded serde values, the `Loaded` behind an `Rc` compared by pointer) or
   `ProjectLoadFailed` (`views/dialogs/load_failed.rs`): a T2-family `Dialog` naming the folder
   and the file-path-carrying error, with **Try again** (an `EngineRestart` bump — the remount
   re-runs the load, so a fixed file or a transient failure recovers in place) and **Close
   window** (also Enter), which runs `spawn_forever(close_this_window(…))` — launcher when it was
   the last. Because `ProjectRoot` is keyed on (folder, generation), a re-root into a broken
   project and an engine restart hit the same detection. The fault path mounts no engine,
   provides no `Subtree`, and never promotes the project in the recents. The recoverable session arms
   (missing → blank; corrupt → kept aside, blank) are unchanged; only defs failures, an
   unreadable session, and a corrupt session whose rename-aside fails are faults.

   Review-settled details. The dialog is **non-modal** (`Dialog::modal(false)`) — it is the
   window's whole content with no feature listeners behind it, and the menubar's Open…/Settings
   items arrive as synthesized key presses, so a modal barrier would kill ⌘O and ⌘,; it also
   stands down while the This/New prompt is up, which would otherwise paint *under* it while
   answering Enter. The fault arm **drains the close-confirm slot**, acting rather than
   re-asking: `guard.running` can only be true there for runs orphaned by a stop the user
   already confirmed — the re-root or restart that replaced the subtree asked the T2 question
   (or the pref that gates every writer of the slot asked never to ask) — so a vetoed red-button
   close or a parked re-root completes an answered question rather than sitting in a slot
   nothing renders (AGENTS.md §2 records the boundary). The fault arm **claims the open-set**
   (`use_claim_open`, the open-set half of `use_open_project` with no recents promotion): it is
   still a window on that project, so a quit reopens it — resurfacing the fault, which is honest —
   and a deliberate close drops it from reopen-on-startup (the acceptance below). The add half is
   load-bearing, not symmetry: a remove-on-drop alone is evicted by the remount a **failed** Try
   again performs, with nothing re-adding it — the quit after that failed retry would silently
   forget the window. It also sets **`OpenCtx::faulted`**, which turns the one open decision that
   was a no-op — naming this window's own project — into a retry (`apply`'s `Nothing` arm bumps
   the generation), so fix-the-file-then-⌘O works; a faulted window focused from *another* window
   still only raises the dialog, whose Try again is the visible recovery. And it keeps the
   window's chrome: `WindowDragStrip` (the header's drag + double-press-to-fill recipe, bare)
   mounts as an overlay **after** the dialog in document order, so it hit-tests above the
   backdrop and a fault window restored onto a detached monitor can still be moved.

   Efficiency: the `Rc<Loaded>` is handed to `use_init_project` / `use_init_session` whole, and
   the defs/session are cloned **inside** the run-once initializers — a re-render of
   `ProjectLoaded` costs an `Rc` bump, never a copy of the catalog or the tabs' text.

   **Registration-race note:** the close is user-initiated (a dialog or red-button press), so
   this window's — and any doomed sibling's — `use_register_window` has long landed by the time
   `is_last()` is asked, and `open_launcher`'s focus-if-open absorbs the double-launcher
   direction. That argument holds *because* the surfacing is a dialog: if it ever flips to
   auto-close, the close must first await this window's own id (the same `post_callback` round
   trip the registration uses).

   The *launch* half was already handled: `main`'s `startup()` reports and skips a folder that
   won't resolve, and the launcher reports rather than opening a doomed window.

   > **The write side is P4-15**, and the two should read as one policy. A file that won't *load*
   > means the window can't exist, so it closes; a file that won't be *written* leaves a perfectly
   > good window whose durable copy is behind — recoverable, and so a visible standing indication
   > rather than a close. Settle the wording together: a user who cannot read and a user who cannot
   > write should not be told in two unrelated registers.

6. ✅ **Quit vs. close, and reopen-on-startup** — see Current state. `Command::Quit` (⌘Q) is now its
   own command, distinct from `Command::CloseProject` (⇧⌘W).

## Acceptance
- [x] A change to shared settings/theme is seen by every open window at once.
- [x] ⌘Q with N windows open restores those N on next launch; deliberately closing them all doesn't.
- [x] Windows spawn/focus/close; native close (red button / ⌘Q / menu) routes through the confirm.
- [x] An unrecoverable defs/session restore error closes that window (→ launcher if it was the last),
      replacing the interim `panic!` in `apps/project/state/hooks.rs`.

## Freya / references
- Plan §4 (client/server split; `create_global` for singletons), §6 (multi-window), §8 (native menu
  is a separate open item). state-arch (per-window Radio vs global). `platform/` module (plan §3).
