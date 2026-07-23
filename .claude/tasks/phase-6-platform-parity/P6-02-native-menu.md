# P6-02 · Native menu bar (decision + menu-follows-opener)

**Phase:** 6 · **Status:** 🟡 **decision made, core built with P2-20** · **DEV_TASKS:** F8 · **Depends on:** P4-01

> **Decision recorded + first slice shipped (2026-07-23, with P2-20):** native muda menubar, yes.
> The fork now implements freya#782 — a `menu` feature (muda promoted to a shared workspace dep
> with tray-icon; `LaunchConfig::with_menu(builder, handler)`; installed via `init_for_nsapp` at
> resume; muda's single global event stream fans out to menubar *and* tray handlers when both
> features are on; `feature_menu` example added). Strata ships a minimal **App menu** (About ·
> Hide/Show · custom Quit routed through the new `RendererContext::request_close_window` →
> `on_close` veto → T2 confirm; accelerator derived from the keymap). Remaining for this task:
> the fuller menu set (File/Window…), **menu-follows-opener**, live accelerator updates on rebind
> (P4-08), and — deliberately — still **no Edit menu** until a design exists that doesn't starve
> the editor's ⌘C/⌘V/⌘X (predefined Edit items claim those as accelerators; DEV_TASKS F8).

## Goal
A native macOS menu bar (File/Edit/Window…), or a deliberate decision not to.

## Current state
Not built. Freya has tray menus only. **Decision pending:** `madsmtm/menubar` vs an in-app menu
(plan §8). Native key events remove much of the *reason* the Dioxus menu existed (⌘A/⌘C swallowing,
the whole F8 muda/shortcut tangle).

**P2-20 findings (2026-07-23)** — the current state is a stopgap and this task is where it gets
fixed properly:

- winit's default macOS menu is now **disabled** (fork `LaunchConfig::with_macos_default_menu`):
  its Quit item sent Cocoa `terminate:` directly — swallowing ⌘Q before the keymap AND bypassing
  the `on_close` veto. Cost: no app menu at all (no About/Hide/⌘H/Services) — fine as a stopgap,
  not the end state.
- **Muda is already in the fork's dependency tree**: `tray-icon` 0.21 depends on `muda` 0.17, and
  `freya-winit/src/tray_icon.rs` already forwards `tray_icon::menu::MenuEvent` through the event
  loop — exactly the plumbing an app menubar needs. Upstream Freya wants this too
  ([freya#782](https://github.com/marc2332/freya/issues/782), Todo since 2024 — we can implement
  it in the fork and offer it upstream). Unify: promote muda to a shared workspace dep (one
  version with tray-icon), add a `menu` feature + `LaunchConfig::with_menu(…)`, init on the event
  loop thread (muda macOS: build + `init_for_nsapp()` on main), forward `MenuEvent` like tray does.
- **Strata's menu then routes Quit as an event**: a *custom* "Quit Strata" item (NOT
  `PredefinedMenuItem::quit()` — that also sends `terminate:`) with its accelerator derived from
  `keymap::effective_chord(CloseProject)`, whose MenuEvent dispatches the same predicate + confirm
  dialog as the red button. Accelerators re-register on rebind (the Dioxus app's dance). A muda
  accelerator intercepts the chord before the keymap listener — one consumer either way, same
  command.
- **winit 0.31 (beta) fixes the root cause — but the migration is gated.** The 0.31 changelog:
  *"On macOS, remove custom application delegates. You are now allowed to override the
  application delegate yourself."* With our own `NSApplicationDelegate` (objc2 `define_class!`,
  a supported pattern in 0.31 — not the 0.30 panic trap of #4458), `applicationShouldTerminate`
  returns `NSTerminateCancel` while a query runs and routes to the confirm — vetoing **every**
  `terminate:` path: menu Quit, **Dock Quit**, even logout. The T2 `CloseGuard` bridge is already
  the right interface: the delegate reads the same atomics and pings the same channel — no
  app-side redesign, just a new veto entry point. Bonus: `ApplicationHandlerExtMacOS::
  standard_key_binding` for macOS keybinding integration.
  **Blockers (as of 2026-07):** still beta (0.31.0-beta.2, Nov 2025; stable line is 0.30.13,
  Mar 2026); `accesskit_winit` 0.33.2 (Jul 2026) still requires winit ^0.30.5 — Freya's a11y
  can't drop it; and the migration is upstream-Freya-sized: 118 beta.1 changelog entries incl.
  the **pointer-event overhaul** (freya-core's whole event mapping), typed user-events removed
  (`user_event` → `user_wake_up` — freya-winit's `NativeEvent` proxy architecture), `Resized` →
  `SurfaceResized`, per-platform `WindowAttributes`. Carrying that in the fork alone = heavy
  divergence; do it when upstream Freya migrates (or alongside it), then land the delegate.
  Until then Dock-icon Quit stays un-veto-able — accepted platform behaviour.

## Build
- Decide + document: `madsmtm/menubar` (native) vs in-app menu vs none-for-now.
- If built: **menu-follows-opener** (launcher → light menu; project → full; settings → match its
  opener). Predefined Edit items where possible (native Cut/Copy/Paste/Undo); the grid's ⌘A/⌘C are
  native events now (P2-20), so the custom-item shims that caused the muda crash aren't needed.

## Acceptance
- [ ] A decision is recorded; if a menu ships, it follows the opener and uses predefined items where possible.

## Freya / references
- Plan §8 (native menu open item). DEV_TASKS F8 (the muda/shortcut analysis + the crash). `platform/`.
