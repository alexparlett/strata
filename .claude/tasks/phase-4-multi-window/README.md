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
phase is the settings **categories** (P4-08 / P4-09, Appearance, Data-display, System and Engine
having landed with P4-04 / P4-05 / P4-06 / P4-07). **P4-11** shipped the
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

> **P4-15's silence is fixed; its *standing condition* isn't.** Every `.strata` and app-config
> writer now reports a failed write as an event and answers whether it landed — one funnel
> (`apps/project/state/persist.rs`), covering defs, session and history, plus
> `strata_core::config::save` finally returning its `Result`. What remains is the half that makes
> a failure visible for as long as it *holds* rather than only when it happened (item 3), and the
> destructive-case decision (item 4). Both matter beyond their own task: the final session save on
> the way down records into a log that dies with its window, and the eight bookkeeping config
> writes deliberately report nowhere, and neither is answerable with another event row.
> It is the *write*-side counterpart to **P4-01 item 5** (a file that won't load closes the
> window) — and deliberately not a phase-5 item: P5 is design polish, not resiliency.

## Tasks

| # | Task | Status                                                                                                                      | DEV_TASKS | Depends on |
|---|---|-----------------------------------------------------------------------------------------------------------------------------|---|---|
| P4-01 | Multi-window shell + shared state (`create_global`) + native close | 🟡 shared state · window model · quit-vs-close done; per-window fault close + Dock quit remain                              | W1/A8 | — |
| P4-02 | Launcher window | ✅                                                                                                                          | U1 | P4-01 |
| P4-03 | Settings window shell (draft/save, live theme, single-instance) | ✅                                                                                                                          | W1/U12 | P4-01 |
| P4-04 | Settings ▸ Appearance | ✅                                                                                                                          | U12 | P4-03 |
| P4-05 | Settings ▸ Data-display | ✅                                                                                                                          | U12 | P4-03 |
| P4-06 | Settings ▸ System (+ history limit) | ✅                                                                                                                          | W3/U12 | P4-03 |
| P4-07 | Settings ▸ Engine (properties editor) |  ✅                                                                                                                            | W2 | P4-03 |
| P4-08 | Settings ▸ Keymap (rebindable) | ⬜                                                                                                                          | W4 | P4-03, P2-20 |
| P4-09 | Settings search | ⬜                                                                                                                          | W3 | P4-03 |
| P4-10 | Export window (rebuild to canvas) | ✅                                                                                                                          | D6/U13 | P4-01, P2-01 |
| P4-11 | Configure-table window (register / edit + import options) | ✅                                                                                                                          | U14/D7/D8 | — |
| P4-13 | Open / create a project (`.strata/` load) | ✅ *(the "New Project UI" it was holding open isn't a thing the design has — Open creates if missing; see the file)* | lifecycle | P4-01 |
| P4-14 | Session persistence + autosave | ✅ *(layout landed with P3-01 on `SessionState` rather than as its own store, so it rode the autosave already built)* | lifecycle | P4-13 |
| P4-15 | `.strata` write resiliency (one funnel, nothing silent) | 🟡 funnel + every silent writer done; the standing condition (item 3) and the destructive-case decision (item 4) remain | lifecycle | P4-13, P4-14, P3-13 |
| P4-16 | Child-window lifetimes across an engine restart | ⬜                                                                                                                          | — | P4-10, P4-11 |

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core ✓]` logic in `strata-core`.

> The **Connections** pieces (rail button, sidebar pane, config LOCATION toggle, object stores) are
> their own cross-cutting workstream — see `workstream-connections/` (W7).
