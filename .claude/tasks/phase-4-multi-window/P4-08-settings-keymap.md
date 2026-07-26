# P4-08 · Settings ▸ Keymap (rebindable)

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** W4 · **Depends on:** P4-03, P2-20

## Goal
The Keymap category: rebind the shortcuts P2-20 wired.

## Current state
Not built — but **the override/rebind layer already exists**: P2-20 shipped the full
settings-driven resolution (`strata_core::keymap` — `COMMANDS`, `effective_chord` reads
`Settings.keybinds` with `chord: None` = explicit unbind, `validate_bind` enforces the
conflict policy: primary-modifier rule + `RESERVED_KEYS` + fixed Esc; `chord_caps`/
`describe` feed the rows; `strata-freya::keymap::chord_from_event` is the capture fold).
Rebinds via a hand-edited config work today and every hint/dispatcher reacts. **This task
is pure UI**: the category page, click-to-capture (route through `validate_bind` +
duplicate-chord checks), conflict box, Custom badge, per-row reset, Reset all — then
`config::save`.

## Wiring into the P4-03 shell
The Settings window shell is built: `Route::Keymap` renders `KeymapPane` in `apps/settings/mod.rs`, which
today is a `Pane::not_built(..)` placeholder. Replace that component's body; nothing else changes.

Every control edits `SettingsCtx::draft` (`use_consume::<SettingsCtx>()`) and stops there. The
footer's **Apply** is the only thing that commits — `write_config(.., &[ConfigChan::Settings], ..)`,
once, for the whole struct — so a page must never persist a field itself. The breadcrumb and the
scroll frame are the shell's; the pane renders content only, and reads its colours from the
`settings` component theme (`hint_color` is a setting's subtext).

## Build (DEV_TASKS W4)
- Interactive rows from the real command table (the P2-20 set): **click-to-capture**, a **conflict box**
  (Reassign steals + unbinds the other / Cancel), a **Custom** badge, per-row **reset ↺**, **Add
  shortcut**, **Reset all**. Both capture *and* reset are conflict-checked (no duplicate binding reachable).
- **Unbind** supported (a command may have no chord). Edits the draft; persists on Apply.
- Bindings live in the **shared** settings so a rebind reaches every window; each window re-registers
  its native shortcuts from the current chords on refocus.
- Dynamic shortcut **hints** everywhere derive from the keymap (no hardcoded glyphs).

## Acceptance
- [ ] Rebind / unbind / reset with conflict resolution; no duplicate bindings; changes reach every window.

## Freya / references
- Design: `Settings.dc.html` Keymap. Command table from P2-20 / `Strata.dc.html` `_commands()`.
  Shared settings (P4-01). DEV_TASKS W4.
