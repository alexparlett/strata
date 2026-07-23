# P2-20 · Keyboard shortcuts (wire the design's keymap)

**Phase:** 2 — Workbench · **Status:** ✅ · **DEV_TASKS:** W4 (base bindings **+ settings-driven resolution**), T2 (OS close) · **Depends on:** — · **Enables:** P2-09 (⌘F), P2-15 (⌘↵), P2-16 (⌘S), tab shortcuts

> **Shipped beyond the original scope** (decision: bindings are settings-state from day one; no
> registry/bus pattern):
> - **`strata-core::keymap`** — the `COMMANDS` table (labels/descs/default chords, `Cancel` fixed),
>   `effective_chord(&Settings, cmd)` (user override → default fallback; invalid/reserved binds
>   warn + fall back), first-match `resolve`, `validate_bind` (primary-modifier rule +
>   `RESERVED_KEYS` a/c/x/v/z/y + fixed Esc), `chord_caps`/`hint`. `Settings.keybinds` is live —
>   a hand-edited config rebinds today; **P4-08 is pure UI on top of `validate_bind`**.
> - **Distributed dispatch, no bus**: each feature attaches
>   `.on_global_key_down(keymap::on_command/on_commands(settings, …))` (strata-freya `keymap.rs`);
>   precedence = document (pre-order) order; a handled command consumes via `prevent_default`.
>   Backed by **fork changes**: consistent `EventName::cmp` total order, same-name global
>   listeners fire in document order (`EventsMeasurer::document_order` → `Tree::document_order`),
>   `Menu`/`Popup` Esc self-close consumes, `WinitPlatformExt::close_current_window()`.
> - **Editor pre-key gate** (`editor/tab.rs`): primary-held non-editor chords skip the buffer
>   (no more ⌘T typing "t" / ⌘↵ inserting a newline) while the globals still fire.
> - **Keymap-derived hints**: tab context/overflow menu rows (`KeyHint`), the status bar's
>   "⌘↵ to run", the running body's "Cancel · Esc" — all react to rebinds.
> - **T2**: `apps/project/close.rs` — atomics+channel bridge to the fork's `with_on_close` veto;
>   themed `CloseConfirm` `Popup` doubles as the modal key barrier; red button / ⌘Q /
>   ⌘W-on-running-tab share one predicate + dialog; respects `confirm_close_running`.
>   A **muda menubar** (fork `menu` feature — freya#782 implemented; see P6-02) replaces winit's
>   default menu, whose Quit sent Cocoa `terminate:` directly (swallowing ⌘Q before the keymap
>   AND bypassing the `on_close` veto). Strata's Quit is a *custom* item routed through the new
>   `RendererContext::request_close_window` (red-button semantics → the T2 confirm), accelerator
>   derived from `keymap::effective_chord`. **Known gap:** Dock-icon Quit still `terminate:`s
>   un-vetoed (needs `applicationShouldTerminate`; winit 0.31's bring-your-own-delegate closes
>   it — see P6-02).

## Goal
Wire the keyboard shortcuts the design defines — **none are implemented in Freya yet**. This provides
the dispatch layer the run/find/save/tab tasks hang their combos off. Native key events (plan §6), so
the Dioxus webview-swallowing / objc complexity is gone.

## Current state
No global keyboard-shortcut handling in `strata-freya`. The **Strata canvas is the source of truth**:
`Strata.dc.html` `_commands()` (+ `_capsFromEvent` / `matchCommand`).

## The command set (from `Strata.dc.html`)
| id | command | keys | notes |
|---|---|---|---|
| `palette` | Command palette | ⌘K | palette itself = U11 / Phase 6; bind the key here |
| `newtab` | New query tab | ⌘T | |
| `reopentab` | Reopen closed tab | ⇧⌘T | |
| `closetab` | Close tab | ⌘W | |
| `closeproject` | Close project → launcher | ⌘Q | intercept the OS close too — see Build 4 |
| `run` | Run query | ⌘↵ | while running, **Esc = Cancel** |
| `savequery` | Save query | ⌘S | |
| `settings` | Open settings | ⌘, | settings window = Phase 4 |
| `cyclewindows` | Cycle windows | ⌘\` | multi-window = Phase 4 |
| `dismiss` | Close overlays/menus | Esc | `fixed: true` |
| `find` | Find in results | ⌘F | **results-scoped** — only in the Strata canvas toolbar title (`Find in results (⌘F)`), not restated in `Results.dc.html` |

## Build
1. A global key handler at each window root (`on_global_key_down`) reading
   `KeyboardEventData.modifiers` (keyboard events **do** carry modifiers, unlike pointer events) →
   match against a command table mirroring the canvas `matchCommand` → dispatch. This is the default
   binding + dispatch, not the rebindable system.
2. Dispatch each command to its target. Targets in later phases (palette U11/P6, settings window P4,
   cycle-windows P4) get the **binding now**; stub/no-op the target with a note until it's built.
3. **Scoping:** `find` (⌘F) is results-scoped; `dismiss` (Esc) closes the topmost overlay first, and
   is Cancel while a query runs.
4. **Intercept OS-triggered closes (T2):** red-button / ⌘Q / dock-quit via native `winit
   CloseRequested` (no objc) → a themed *close-while-running* confirm when a query is in flight,
   otherwise close. This is the close half of `closeproject` / `closetab` — it belongs with the
   keybindings, not the tab nits.

## Acceptance
- [ ] Each listed shortcut fires its action (or a clearly-stubbed target) via native key events.
- [ ] Esc dismisses the topmost overlay; ⌘↵ runs; ⌘F opens find; ⌘T/⌘W/⇧⌘T act on tabs.
- [ ] An OS-triggered close (red button / ⌘Q / dock) with a query running shows the confirm; else closes.

## Freya / references
- `Strata.dc.html` `_commands()` / `matchCommand` (authoritative). Plan §6 (native keymap — complexity
  evaporates). `KeyboardEventData.modifiers` (verified in the fork).
- **The rebindable keymap** (overrides, click-to-capture, conflict resolution, Settings ▸ Keymap page)
  is **Phase 6 / W4** — this task is the default bindings + dispatch it builds on.
