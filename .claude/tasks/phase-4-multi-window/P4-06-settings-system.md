# P4-06 · Settings ▸ System (+ history limit)

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** W3 / U12 · **Depends on:** P4-03

## Goal
The System category, including the query-history limit.

## Current state
Not built. `Settings.max_history` caps `project.history` (P3-14).

**`Settings.open_pref` is live and has no control here yet** (P4-13). The project window's open
path reads it on every open — This window / New window / Ask, with Ask raising the This/New prompt
— but the only way to *change* it today is that prompt's "Remember, don't ask again" checkbox,
which is one-way in practice: once remembered, nothing in the UI puts it back to Ask. That is the
reason this control is worth pulling before the rest of the Settings window.
The segmented control that sets it directly is this task's: the canvas has it as
`Settings.dc.html`'s `data-openpref` row ("Opening a project": Ask each time · This window ·
New window). Write it through `write_config(.., &[ConfigChan::Settings], ..)` like every other
setting — nothing else needs touching, the readers are already there.

## Wiring into the P4-03 shell
The Settings window shell is built: `Route::System` renders `SystemPane` in `apps/settings/mod.rs`, which
today is a `Pane::not_built(..)` placeholder. Replace that component's body; nothing else changes.

Every control edits `SettingsCtx::draft` (`use_consume::<SettingsCtx>()`) and stops there. The
footer's **Apply** is the only thing that commits — `write_config(.., &[ConfigChan::Settings], ..)`,
once, for the whole struct — so a page must never persist a field itself. The breadcrumb and the
scroll frame are the shell's; the pane renders content only, and reads its colours from the
`settings` component theme (`hint_color` is a setting's subtext).

## Build
- System prefs as a uniform divider-separated list (no ALL-CAPS labels — U12 alignment).
- **History limit** = `Settings.max_history` (default 100) as a numeric input, like the data-display
  fields; the history list is truncated to the cap after each insert (P3-14).
- **Opening a project** = `Settings.open_pref`, the three-way segmented control above.

## Acceptance
- [ ] System fields edit the draft; history-limit changes cap the History drawer.
- [ ] Setting "Opening a project" to This/New stops the prompt appearing and lands opens there;
      back to Ask and the prompt returns.

## Freya / references
- Design: `Settings.dc.html` System. DEV_TASKS W3/U12. `Settings.max_history`.
