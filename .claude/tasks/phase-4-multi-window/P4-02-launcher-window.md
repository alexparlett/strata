# P4-02 · Launcher window

**Phase:** 4 · **Status:** ✅ · **DEV_TASKS:** U1 · **Depends on:** P4-01

## Goal
The launcher: recent/pinned projects, open, and the entry point when no project is open.

## As built (`apps/launcher/`)
- `mod.rs` — the `LauncherApp` window root (760×560 to the canvas card, transparent titlebar +
  traffic-light inset) and the `launcher` theme (`define_theme!` with `%[no_ext]`, since four
  sibling views read it and there is no one `Launcher` component to hang the builder off).
- `model.rs` — `ProjectList::build(config, query)`: the filter (name **or** path, case-insensitive)
  and the PINNED / RECENT split, unit-tested without a window.
- `views/` — `title_bar` (the drag strip), `rail` (brand · Projects pill · Settings), `projects`
  (filter · OPEN · groups · the two empty states), `row` (avatar · name/path · Pin · Reveal ·
  Remove), `open` (the rfd folder picker + the close-behind-it hand-off).

Data is the app-global config store, live: `use_config(ConfigChan::Recents)` + `ConfigChan::Open`
to render, `write_config(…, &[ConfigChan::Recents], …)` with `AppConfig::{set_pinned,
remove_recent}` for the row actions. No launcher-local copy of the recents, so a pin here and a
window's `push_recent` can't overwrite each other (the Dioxus bug).

Opening goes through **`platform::open_project`** — the shared window path — so a project that
already has a window is *focused*, and the launcher then closes itself. A recent whose folder no
longer exists on disk is removed rather than offered: `config::load` prunes the recents at startup
(`AppConfig::prune_missing`), and a press on a row whose folder went away mid-session drops the
entry (`platform::resolve_recent` — shared with the header switcher and the menubar's Open
Recent). A resolve that fails for any other reason is reported and the launcher stays up.

Deferred at their own seams: the rail's **Settings** gear logs (P4-03 owns the single-instance
settings window), and the in-window this-window/new-window prompt is P4-13's (`OpenPref`).

## Acceptance
- [x] Recents + pinned render; open/pin/reveal/remove work; filter + empty states match the canvas.
- [x] Opening a project transitions to the project window (or focuses the one it already has).
- [ ] The gear opens settings — **P4-03**; inert with a note until then.

## Freya / references
- Design: `Launcher.dc.html`. DEV_TASKS U1. Shared recents + the window model (P4-01).
