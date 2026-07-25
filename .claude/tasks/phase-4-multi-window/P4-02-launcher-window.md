# P4-02 · Launcher window

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** U1 · **Depends on:** P4-01

## Goal
The launcher: recent/pinned projects, open, and the entry point when no project is open.

## Current state
Not built.

## Build (to `Launcher.dc.html`)
- Recent + **PINNED** project groups; row actions **Pin · Reveal · Remove** (3, not 4 — no
  open-in-new-window); pin state from the PINNED grouping + tint (no inline badge).
- Ghost uppercase **Open** (eyebrow type) to pick a folder.
- Filter box → matches; nav pill is tinted-bg only (no accent left-bar).
- Empty states split: no-match (`No projects match "q".`) vs no-recents, left/muted.
- Rail **Settings** gear → opens the settings window (P4-03).
- Reads/writes **recents** through the app-global config store (P4-01, landed):
  `use_config(ConfigChan::Recents)` to render, `write_config(config, &[ConfigChan::Recents], …)` with
  `AppConfig::{set_pinned, remove_recent}` for the row actions — the store persists, so no
  `config::load`/`save` at the call site and no snapshot signal to keep fresh (the Dioxus launcher's
  point-in-time copy is what let a project window's `push_recent` and a pin overwrite each other).
  `ConfigChan::Open` says which recents already have a window — focus those instead of reopening.
- `RecentProject::path` is the **project folder**, not its `.strata` dir (legacy paths are migrated
  on load) — hand it straight to the open path.

## Acceptance
- [ ] Recents + pinned render; open/pin/reveal/remove work; filter + empty states match the canvas.
- [ ] Opening a project transitions to the project window; the gear opens settings.

## Freya / references
- Design: `Launcher.dc.html`. DEV_TASKS U1. Shared recents (P4-01). Freya `SideBarItem`/list.
