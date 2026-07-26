# P4-07 · Settings ▸ Engine (properties editor)

**Phase:** 4 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** W2 · **Depends on:** P4-03

## Goal
The Engine category: a free-form DataFusion **properties** editor, applied live (with restart-gated
runtime keys).

## Current state
Not built. Core: `engine_config` is a flat `ENGINE_KEYS` catalog (name/default/`Kind`/desc);
`Settings.engine` is a `BTreeMap` of **non-default overrides only**.

## Wiring into the P4-03 shell
The Settings window shell is built: `Route::Engine` renders `EnginePane` in `apps/settings/mod.rs`, which
today is a `Pane::not_built(..)` placeholder. Replace that component's body; nothing else changes.

Every control edits `SettingsCtx::draft` (`use_consume::<SettingsCtx>()`) and stops there. The
footer's **Apply** is the only thing that commits — `write_config(.., &[ConfigChan::Settings], ..)`,
once, for the whole struct — so a page must never persist a field itself. The breadcrumb and the
scroll frame are the shell's; the pane renders content only, and reads its colours from the
`settings` component theme (`hint_color` is a setting's subtext).

**Engine is the one pane that will want its own frame.** The canvas gives it a full-height flex
layout with a *Revert changes* action on the breadcrumb line, which the shell's fixed
breadcrumb-then-content frame does not allow for. Widen `Pane` (a trailing-action slot, and an
opt-out of the scroll frame) rather than bypassing it, so the other four keep one frame.

## Build (DEV_TASKS W2, design24)
- A JetBrains-style key/value **Properties** editor: add / remove / duplicate / paste rows, **catalog
  autocomplete** (Freya `Popup`) against `ENGINE_KEYS`, a selection inspector.
- **Per-value validation** (bool/int/bytes/duration/timezone/enum) blocks Apply + reveals inline on
  blur; a reset-all button; `== default` clears the key.
- **Applied live:** on Save, `crate::engine` rebuilds the `SessionContext` config; the 9 `ConfigOptions`
  live-set via `SetEngineConfig`; a changed `datafusion.runtime.*` is **restart-gated**
  (`EngineRestartRequired` → a restart modal). `format.*` feeds the grid formatter.

## Acceptance
- [ ] Add/edit/remove properties with autocomplete + validation; Save applies live; runtime.* prompts restart.

## Freya / references
- Design: `Settings.dc.html` Engine. Core `engine_config` (`ENGINE_KEYS`), `Command::SetEngineConfig`,
  `Event::EngineRestartRequired`. DEV_TASKS W2. (strata-forms retained for config/export/connections.)
