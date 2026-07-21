# Phase 4 — Multi-window

The other OS windows and the machinery that lets them share state: **launcher**, **settings**,
**export**, the **config / register-table** modal, cross-window shared state, and native close
handling.

## State of play
Greenfield in Freya — only the project window exists (`apps/project/`). Per the port plan §6 these
land here, and per §4 the cross-window singletons (settings, theme, recents) use
**`State::create_global`** (created in `main`, passed into each window root), **not** a per-window
Radio station. Each window is its own Freya `App` root under `apps/<window>/`. Native close uses
**`winit CloseRequested`** (no objc). The Dioxus app shipped all of this (W1–W4, D6–D8) — this is the
Freya rebuild.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| P4-01 | Multi-window shell + shared state (`create_global`) + native close | ⬜ | W1/A8 | — |
| P4-02 | Launcher window | ⬜ | U1 | P4-01 |
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

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core ✓]` logic in `strata-core`.

> The **Connections** pieces (rail button, sidebar pane, config LOCATION toggle, object stores) are
> their own cross-cutting workstream — see `workstream-connections/` (W7).
