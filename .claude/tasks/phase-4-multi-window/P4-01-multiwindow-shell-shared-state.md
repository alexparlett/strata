# P4-01 · Multi-window shell + shared state + native close

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** W1 / A8 · **Depends on:** — · **Unblocks:** P4-02, P4-03, P4-10

## Goal
The plumbing for more than one OS window: app-wide shared state, window spawn/focus, and native
close handling.

## Current state
Only the project window exists. `main.rs` launches it. No cross-window state, no other window roots.

## Build (plan §4/§6)
1. **Shared singletons** via `State::create_global` in `main` (before launch): **settings**, **theme**,
   **recents**. Pass them into each window root; theme also drives `use_init_theme`. These are the
   *only* globals — per-window model stays in each window's Radio station.
2. **Window management** (`platform/`): spawn / focus-if-open / close a window (project, launcher,
   settings, export). Single-canonical instances where required (settings).
3. **Native close handling**: intercept `winit CloseRequested` (no objc) → the themed close-while-
   running confirm hooks in here (shares P2-20's close path).
4. Each window is a Freya `App` root under `apps/<window>/` (symmetric; no project-window special case).

## Acceptance
- [ ] A change to shared settings/theme is seen by every open window at once.
- [ ] Windows spawn/focus/close; native close (red button / ⌘Q / dock) routes through the confirm.

## Freya / references
- Plan §4 (client/server split; `create_global` for singletons), §6 (multi-window), §8 (native menu
  is a separate open item). state-arch (per-window Radio vs global). `platform/` module (plan §3).
