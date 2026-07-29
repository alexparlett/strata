# Phase 4 — Multi-window

The other OS windows and the machinery that lets them share state: **launcher**, **settings**,
**export**, the **Configure-table** window, cross-window shared state, native close
handling, and **project lifecycle** (open/load + session persistence).

## State of play
The **multi-window spine is up**: three window roots (`apps/project/`, `apps/launcher/`,
`apps/settings/`), app-globals created in `main` and handed to each root — the config store
(`state/config.rs`), the live window registry (`platform/windows.rs`) and the Settings window's
theme preview (`state/theme_preview.rs`) — and one window path everything opens through
(focus-if-open, launcher-if-last, quit-closes-all). Per plan §4 the cross-window singletons use
**`State::create_global`**, **not** a per-window Radio station; native close uses **`winit
CloseRequested`** (no objc), and the one place objc *is* reached for is the fork's
`set_window_parent` (P4-03 pins Settings above the window that opened it). What's left in this
phase is the settings **categories** (P4-07…P4-09, Appearance, Data-display and System having
landed with P4-04 / P4-05 / P4-06) — plus P4-13's open/create UI. **P4-11** shipped the
Configure-table window as one task, not two (**P4-12 was folded into it**: the format dropdown is
what selects the import-option set, the option set moves the file-extension filter, and both halves
reach the engine through one `TableSpec`), and settled two things every later surface inherits —
a window carries **no theme of its own** (chrome is the sheet, form vocabulary is the `form` theme),
and a trigger that opens a window **sets a slot** the root acts on rather than holding the window's
handles itself. **P4-10
settled how a window opened *on something* behaves**: an export window carries the run it was
opened on, so it is a child window with deliberately **no** single-instance rule (focusing an open
one would show the wrong run), and it **pins** the snapshot it reads for its whole life — the RAII
pin in AGENTS.md §2 exists because of it.
**P4-04 settled how every later pane commits**: the draft is diffed per-field against its seed
(`Settings::merge_onto`, exhaustive by compiler check), never written wholesale, so a setting
another window wrote while Settings was open survives Apply — add a field, and the build tells you
to merge it. **P4-05 settled what every later pane is made of**: `components::form` — a pane is a
`Form::preferences` of `Row`s, so it carries its own settings and nothing about the rhythm between
them (the module composes `Form` > `Row` > control, the register being a `Variant` on the form).
The Dioxus app shipped all of this (W1–W4, D6–D8) — this is the Freya rebuild.

> **Pull P4-15 before the remaining writers.** `.strata` write failures are reported through
> `tracing` and nowhere the user can see. **P4-11 added a new mutation site** and routed it
> through P3-13's `actions::persisted`, gating its own success on the answer — so the funnel now
> has three callers and one of them proves the shape works. P4-10 landed ahead of it and reports both
> arms through P3-13's `log_event` directly — leaving P4-15 the question of whether an export,
> which writes where the *user* chose rather than into `.strata`, belongs in that funnel at all. P3-13 fixed the three def-mutation paths
> it touched (Save, Save-as-view, drop) and gave them one helper; P4-15 generalises it, covers the
> session / history / app-config writers, and settles what the UI says while a write is failing.
> It is the *write*-side counterpart to **P4-01 item 5** (a file that won't load closes the
> window) — and deliberately not a phase-5 item: P5 is design polish, not resiliency.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| P4-01 | Multi-window shell + shared state (`create_global`) + native close | 🟡 shared state · window model · quit-vs-close done; per-window fault close + Dock quit remain | W1/A8 | — |
| P4-02 | Launcher window | ✅ | U1 | P4-01 |
| P4-03 | Settings window shell (draft/save, live theme, single-instance) | ✅ | W1/U12 | P4-01 |
| P4-04 | Settings ▸ Appearance | ✅ | U12 | P4-03 |
| P4-05 | Settings ▸ Data-display | ✅ | U12 | P4-03 |
| P4-06 | Settings ▸ System (+ history limit) | ✅ | W3/U12 | P4-03 |
| P4-07 | Settings ▸ Engine (properties editor) | ⬜ | W2 | P4-03 |
| P4-08 | Settings ▸ Keymap (rebindable) | ⬜ | W4 | P4-03, P2-20 |
| P4-09 | Settings search | ⬜ | W3 | P4-03 |
| P4-10 | Export window (rebuild to canvas) | ✅ | D6/U13 | P4-01, P2-01 |
| P4-11 | Configure-table window (register / edit + import options) | ✅ | U14/D7/D8 | — |
| P4-13 | Open / create a project (`.strata/` load) | 🟡 internals + the open path done (`OpenPref` honoured everywhere; This Window = keyed remount); **New Project** UI remains | lifecycle | P4-01 · *pull early* |
| P4-14 | Session persistence + autosave | 🟡 tabs + history + window geometry done (load + autosave, incl. a final save on close/re-root); layout awaits its store | lifecycle | P4-13 |
| P4-15 | `.strata` write resiliency (one funnel, nothing silent) | ⬜ | lifecycle | P4-13, P4-14, P3-13 |
| P4-16 | Child-window lifetimes across an engine restart | ⬜ | — | P4-10, P4-11 |

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core ✓]` logic in `strata-core`.

> The **Connections** pieces (rail button, sidebar pane, config LOCATION toggle, object stores) are
> their own cross-cutting workstream — see `workstream-connections/` (W7).
