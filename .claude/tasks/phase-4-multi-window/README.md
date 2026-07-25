# Phase 4 — Multi-window

The other OS windows and the machinery that lets them share state: **launcher**, **settings**,
**export**, the **config / register-table** modal, cross-window shared state, native close
handling, and **project lifecycle** (open/load + session persistence).

## State of play
The **multi-window spine is up**: two window roots (`apps/project/`, `apps/launcher/`), two
app-globals created in `main` and handed to each root — the config store (`state/config.rs`) and the
live window registry (`platform/windows.rs`) — and one window path everything opens through
(focus-if-open, launcher-if-last, quit-closes-all). Per plan §4 the cross-window singletons use
**`State::create_global`**, **not** a per-window Radio station; native close uses **`winit
CloseRequested`** (no objc). What's left in this phase is the *windows themselves*: settings,
export, the config modal — plus P4-13's open/create UI. The Dioxus app shipped all of this
(W1–W4, D6–D8) — this is the Freya rebuild.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| P4-01 | Multi-window shell + shared state (`create_global`) + native close | 🟡 shared state · window model · quit-vs-close done; per-window fault close + Dock quit remain | W1/A8 | — |
| P4-02 | Launcher window | ✅ | U1 | P4-01 |
| P4-03 | Settings window shell (draft/save, live theme, single-instance) | ⬜ | W1/U12 | P4-01 |
| P4-04 | Settings ▸ Appearance | ⬜ | U12 | P4-03 |
| P4-05 | Settings ▸ Data-display | ⬜ | U12 | P4-03 |
| P4-06 | Settings ▸ System (+ history limit) | ⬜ | W3/U12 | P4-03 |
| P4-07 | Settings ▸ Engine (properties editor) | ⬜ | W2 | P4-03 |
| P4-08 | Settings ▸ Keymap (rebindable) | ⬜ | W4 | P4-03, P2-20 |
| P4-09 | Settings search | ⬜ | W3 | P4-03 |
| P4-10 | Export window (rebuild to canvas) | ⬜ | D6/U13 | P4-01, P2-01 |
| P4-11 | Config / register-table modal | ⬜ | U14/D7 | — |
| P4-12 | Import (read) options (CSV/JSON) | ⬜ | D8 | P4-11 |
| P4-13 | Open / create a project (`.strata/` load) | 🟡 internals + the open path done (`OpenPref` honoured everywhere; This Window = keyed remount); **New Project** UI remains | lifecycle | P4-01 · *pull early* |
| P4-14 | Session persistence + autosave | 🟡 tabs + history + window geometry done (load + autosave, incl. a final save on close/re-root); layout awaits its store | lifecycle | P4-13 |

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core ✓]` logic in `strata-core`.

> The **Connections** pieces (rail button, sidebar pane, config LOCATION toggle, object stores) are
> their own cross-cutting workstream — see `workstream-connections/` (W7).
