# P4-01 · Multi-window shell + shared state + native close

**Phase:** 4 · **Status:** ✅ · **DEV_TASKS:** W1 / A8 · **Depends on:** — · **Unblocks:** P4-03, P4-10

## Goal
The plumbing for more than one OS window: app-wide shared state, window spawn/focus, and native
close handling.

## Current state
**Shared state is done** (build item 1): `crate::state::config` holds the app-global `AppConfig`
store — a global `RadioStation<AppConfig, ConfigChan>` (settings · recents · open-set) created in
`main` and shared into each window root by `use_share_config`; `write_config` is the single
mutate-and-persist path; `use_claim_open` keeps a window's project in the open-set for the life of
its subtree mount and `use_promote_recent` heads the recents with it once it has actually loaded
(item 5). Theme is pure derived state off the `Settings` channel (`use_strata_theme`).

**The window model is done** (items 2 · 4 · 6, and item 3's close routing), built with P4-02 because
the launcher is the surface that needs it — `crate::platform::windows`:

- `WindowRegistry` — a second app-global (`State<Windows>`): the **live** id map (winit `WindowId` →
  launcher / project folder), as opposed to the config store's *persisted* open-set. Each window
  root joins it via `use_register_window` for its lifetime.
- `open_project` / `open_launcher` — focus-if-open, else `launch_window`. The launcher is
  single-instance by construction.
- `quit` / `quit_windows` — ⌘Q (`Command::Quit`, new) and menu Quit close **every** window and set
  a `QUITTING` flag that suppresses `use_claim_open`'s open-set removal, so the next launch
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

   **The load is asynchronous, and that is a third arm** (built after two review passes flagged
   the residual risk and it was deferred). Every step of `open_project` is synchronous
   `std::fs` — and Freya is one event loop drawing every window, so run in the render pass a
   project on a mount that stopped answering (an SMB/NFS mount that went quiet: blocking in the
   kernel, no timeout, uninterruptible — exactly the `SessionLoadError::Unreadable` case the
   fault dialog exists for) froze the entire app, and Try again was a button that re-entered the
   freeze on demand. Freya's `spawn` polls on that same thread, so an `async` block moves
   nothing; the engine's private Tokio runtime cannot help either, since an engine is only built
   *after* a successful load. So:

   - **`task::offload`** (`src/task.rs`) is the one way across: the work runs on a thread of its
     own and the caller awaits a `oneshot`, which reaches the UI executor through the same
     cross-thread wake `async_io::Timer` already uses for the autosave debounce (task waker →
     the runner's channel → the winit `EventLoopProxy` the receiver is parked on). A thread
     **per call**, not a pool or one worker: shared, one wedged mount would hold up the next
     project's open, which is this failure moved one step along rather than removed. Cancelling
     is dropping the answer, never stopping the work — a blocking syscall cannot be interrupted,
     so the honest cost is one parked thread per attempt against a dead mount, named in the
     module doc.
   - **`state::hooks::load_project`** wraps `open_project` in it and mints the `Rc` on the near
     side (`Loaded` crosses the thread as the plain serde values it is).
   - **`ProjectRoot` drives it with `use_future`**, not a hand-rolled `use_state` + `spawn`:
     `FutureState`'s `Pending | Loading | Fulfilled(Ok | Err)` *are* the three arms, and the task
     is scope-bound, so a remount abandons a read in flight rather than letting it write into a
     subtree that has gone. `FutureTask::start` is deliberately unused — a retry stays the
     generation bump, because that is the one mechanism reachable from outside the subtree
     (`OpenCtx::faulted`), and a remount re-runs the future anyway.
   - **`ProjectLoading`** (`views/loading.rs`) is the new arm: the window background alone until
     `SLOW_LOAD` (600ms) has passed, then a `CircularLoader`, "Opening '<name>'" and **Close
     window**. Nothing is drawn for a load nobody could perceive — a spinner that flashes on
     every open is worse than none — and the button says *Close window* rather than Cancel
     because there is no honest way to stop the read. It holds no engine, no store and no
     `Subtree`, so no child window can be opened from it or handed a handle that would outlive
     the mount. Retry storms are ruled out by construction: Try again is only reachable once the
     load has settled.
   - **`use_engineless_close`** (`close.rs`) is the once-only close + confirm-slot drain, now
     shared by both engineless arms rather than copied: the loading arm needs the identical pair
     for the identical reason (`guard.running` can be true there for runs orphaned by whatever
     replaced the last subtree, so a red-button close would otherwise land in a slot nothing
     renders and read as a dead control).
   - **`use_claim_open` moved up to `ProjectRoot`**, and `use_open_project` split into it plus
     `use_promote_recent`. The open-set claim is true of the *mount* and not of the outcome —
     loading, loaded and faulted are all a window on that project — while the recents promotion
     is earned by loading (and needs the name only the defs carry). Hoisting also settles what
     an arm could not: two arms each with an add-on-mount / remove-on-drop pair would depend on
     which way the diff orders the swap between them.

   **The blocking read that actually hangs first was the geometry pre-read**, so it is part of
   the same fix rather than scope beside it: `ProjectApp::window` read `.strata/session.json`
   itself, on the render thread, before the window existed — the *same* `load_session` call, so
   without this the loading arm would never get a chance and the app would freeze one step
   earlier. Geometry is now a **launch input like the folder** (`ProjectApp::window(app, root,
   geometry)`), resolved by the caller through `window_geometry` — `offload` raced against a
   250ms `GEOMETRY_DEADLINE`, because Freya has no runtime resize/move and a window is placed as
   it is created or not at all. Giving up is the right trade (a remembered size is a nicety, a
   window is not, and the window is where every other truth about the project gets told) and it
   costs nothing durable, because **the autosave seed now comes from the session the project
   actually loaded** rather than from that read: seeding `None` would have let the first save
   replace a perfectly good remembered size with the default the window opened at. Two of the
   three callers await it (`platform::open_project`, and `main`'s startup where blocking is free
   — no event loop yet); the menubar's Open Recent runs inside a muda handler with a
   `RendererContext` and no executor, so it uses `window_geometry_blocking` — the one remaining
   wait on a project folder from a thread that matters, now bounded and brief where it was
   unbounded.

   > **Still synchronous, deliberately named:** `platform::resolve_project_folder`'s
   > `fs::canonicalize`. It runs on the UI thread from the picker, the recents surfaces and
   > startup routing, and on a dead mount it blocks like anything else. It is a different
   > question from this one (*does this path name a project* vs *load this project*) with four
   > call sites and no window yet to report into, so it wants its own change — `offload` is
   > already the primitive it would use.

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
   without promoting the recent: it is still a window on that project, so a quit reopens it —
   resurfacing the fault, which is honest — and a deliberate close drops it from reopen-on-startup
   (the acceptance below). The add half is load-bearing, not symmetry: a remove-on-drop alone is
   evicted by the remount a **failed** Try again performs, with nothing re-adding it — the quit
   after that failed retry would silently forget the window. (That started as a fault-arm special
   case, `use_claim_open` beside the recents-promoting `use_open_project`. The async arm made it the
   general rule instead — the claim moved up to `ProjectRoot` and the promotion became
   `use_promote_recent`; see the async section above.) It also sets **`OpenCtx::faulted`**, which turns the one open decision that
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
- [x] A project whose read never answers leaves the **app** responsive. Verified against a scratch
      project with `.strata/session.json` replaced by a **FIFO with no writer** — which blocks in
      `open` with no timeout, exactly as a dead SMB/NFS mount does, and needs no network to
      reproduce. `sample` on the running app showed two `strata-offload` threads parked in
      `open` (the geometry read the deadline gave up on, and the project load) while the **main
      thread sat in `freya_winit::launch`'s event loop** — before this change that thread was the
      one in `open`. The geometry deadline logged its warning and the window opened at the default
      size rather than the app blocking before any window existed.
- [x] The arm that is up while a load is out is the **loading** one, asserted without pixels: with
      a wiped config, `open_projects` held the project and `recent_projects` was **empty** — so
      `ProjectRoot` had mounted and claimed, and `ProjectLoaded` had not. Writing to the FIFO then
      handed the same window over in place (recents became `['sales']`, same pid, no remount), and
      the partial read the FIFO delivered was classified `Corrupt` and moved aside — the
      recoverable path, unchanged.

## Freya / references
- Plan §4 (client/server split; `create_global` for singletons), §6 (multi-window), §8 (native menu
  is a separate open item). state-arch (per-window Radio vs global). `platform/` module (plan §3).
