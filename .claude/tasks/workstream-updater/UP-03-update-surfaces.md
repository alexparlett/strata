# UP-03 · Surfaces: launcher affordance, dialog, setting, palette command

**Workstream:** Updater · **Status:** ⬜ · **Depends on:** UP-02 (`UpdateStatus` + actions)

## Goal
The mechanism becomes visible: a setting that gates the automatic check, an affordance where the
version already shows, one confirm-shaped dialog in front of the restart, and a palette command
for checking on demand. Deliberately quiet — no toast system, no badge on every window.

## Current state (verified 2026-08-12)
- The launcher rail prints the version — `Meta::new(env!("CARGO_PKG_VERSION"))` at
  `apps/launcher/views/rail.rs:68`. This is the one place the app already talks about its
  version, which makes it the natural update affordance.
- The results status bar is **per results pane** (`workbench/results/status_bar.rs`,
  constructed per `ResultsState`) — it is not an app status bar and is the wrong home for an
  app-scoped indicator.
- Settings toggles are five mechanical edits, four compiler-checked: field on `Settings`
  (`strata-core/src/config.rs:144`, `#[serde(default …)]`), `Default` (`config.rs:365-390`),
  the `settings_merge!` list (`config.rs:307-323` — omission is a build error), a
  `settings_index!` entry (`apps/settings/search.rs:95-192`), and the pane row. The boolean-row
  template is `ConfirmClose` (`apps/settings/views/system.rs:97-107`): `Anchor::X.row()` +
  `Switch`, both handlers through `SettingsCtx::edit`. A **new** field with a serde default is
  safe against existing config files; only a **changed** default needs migration
  (`config.rs:490-499`).
- Confirm dialogs are the slot pattern: a `State<Option<T>>` provided at the window root, the
  dialog mounted unconditionally and watching it, askers `slot.set(Some(..))`. Precedence is
  mount order (`apps/project/app.rs:756-796`). Two documented traps: read the slot into a value
  **before** `set(None)` (generational borrow across a match panics —
  `dialogs/close_confirm.rs:79-83`), and dismiss before any action that unmounts the subtree,
  via `spawn_forever` (`close_confirm.rs:91-99`).
- The palette is registered in `apps/project/commands.rs` (`#[command_router]` at `:139`);
  `close_project` (`:210-219`) is the template for a command acting through `PaletteCtx`
  handles. **The palette exists only in project windows** (`app.rs:792`) — it cannot be the
  launcher's path to the updater.

## Build

1. **The setting.** `Settings::check_updates`, default **true**, doc comment naming its
   consumer (the startup check in `state/updates.rs`). The five edits above; the row lands in
   Settings ▸ System beside `ConfirmClose`, wording in the app's IDE register — label
   `Check for updates`, hint one plain sentence (e.g.
   `Ask GitHub for a newer release when Strata starts.`). The toggle gates only the
   *automatic* check; manual checks always work.
2. **The launcher affordance.** The rail's version line becomes state-aware: on
   `Available`/`Ready` it gains an accent action under the version —
   `Update to 0.4.0` (press starts the download) / `Restart to update` (press opens the confirm
   below); `Downloading` shows quiet progress text. `Idle`/`UpToDate`/`Failed` change nothing —
   the rail never nags, and a failed check is a log line, not launcher chrome. When install
   eligibility is degraded (UP-02: no writable bundle), the action opens the release page
   instead and says so in its own wording.
3. **The dialog.** `UpdateConfirm` over its own `State<Option<UpdateAsk>>`, standard 420
   confirm (`components/dialog.rs`; Enter-confirm belongs to the card — the modal base owns
   only Esc). Body: the version it will restart into, and a plain link-out to the release page
   for what changed. Confirm = the UP-02 install action (quit-shaped; every close confirm still
   gets its say **after** it — this dialog asks "restart now?", it does not re-ask "lose the
   running query?", which stays the close confirm's question). Dismiss = status stays `Ready`,
   quietly. Mount it in **both** window kinds that can offer it (launcher root; project root
   after the existing confirms, `app.rs:763-796`), one component, two mounts — the slot is
   per-window, the status behind it is the one app-global.
4. **The palette command.** `Check for updates` in `commands.rs` — no `key` (no keymap change),
   keywords `update upgrade version release`. Body: run the manual check via the `UpdateStatus`
   handle on `PaletteCtx` (new field, gathered in `use_palette_ctx`); if the answer is
   `Available`, surface the project window's affordance path (open the dialog once downloaded,
   or start the download — same presses as the rail, through the same actions; the command adds
   no second implementation). While `Checking`/`Downloading`, the command is a no-op re-press.
5. **Project-window visibility (kept minimal, on purpose).** No persistent indicator in v1: the
   launcher rail and the palette command are the surfaces. If an in-project indicator is wanted
   later it is a phase-5 design question (where does app chrome live in a window that is all
   panes?) — note it there rather than inventing chrome here.

## Acceptance
- [ ] Toggle in Settings ▸ System, searchable, applied through the draft/apply funnel; off means
      no startup check, and the manual command still works.
- [ ] Launcher rail offers the update through its existing version line; no state change for
      up-to-date/failed.
- [ ] One `UpdateConfirm`, mounted at launcher and project roots, slot pattern, borrow-trap
      safe; confirm quits through the normal path (close confirms still fire), dismiss keeps
      `Ready`.
- [ ] Palette command exists in project windows, no keybinding, acts through the same actions
      as the rail.
- [ ] All user-facing text in the IDE register (AGENTS.md §3): terse, single-quoted
      identifiers, no glyphs.

## References
- `apps/launcher/views/rail.rs:68` — the version line.
- `apps/settings/views/system.rs:97-107` — the toggle-row template; `search.rs:95` — the index.
- `components/dialog.rs`, `dialogs/close_confirm.rs` — the confirm shape and its two traps.
- `apps/project/commands.rs:139,210-227` — the router and the command templates.
