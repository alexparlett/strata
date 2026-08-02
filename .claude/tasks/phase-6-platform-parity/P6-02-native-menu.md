# P6-02 · Native menu bar (decision + menu-follows-opener)

**Phase:** 6 · **Status:** ✅ **done** · **DEV_TASKS:** F8 · **Depends on:** P4-01

> **Decision:** a native muda menubar, yes — recorded 2026-07-23 with P2-20, completed 2026-08-01.
> The fork implements [freya#782](https://github.com/marc2332/freya/issues/782): a `menu` feature
> (muda promoted to a shared workspace dep with tray-icon; `LaunchConfig::with_menu(builder,
> handler)`; installed via `init_for_nsapp` at resume; muda's single global event stream fans out to
> menubar *and* tray handlers when both features are on; `feature_menu` example added).

## Goal
A native macOS menu bar (File/Edit/Window…), or a deliberate decision not to.

## What shipped

**App** — About · Settings… · Hide/Hide Others/Show All · Quit Strata.
**File** — New Query · Open… · Open Recent ▸ · Save Query · Close Project.
**Edit** — Undo · Redo · Cut · Copy · Paste · Select All.
**Window** — Minimize · Zoom · Cycle Windows.

All of it in `crates/strata-freya/src/menu.rs`, whose module doc carries the reasoning. Four things
it settled:

- **Quit and Close Project are different things, and both route through the close veto**, never
  Cocoa's `terminate:` (`PredefinedMenuItem::quit()` sends exactly that — the thing that bypassed
  the T2 confirm). Quit asks every window and marks the app quitting, so the open set survives to
  the next launch; Close Project asks the focused window, which drops it from that set.
- **The Edit menu is custom items, not muda's predefined set** — which **overturns this task's
  original plan** ("predefined Edit items where possible") and F8's. The predefined items send
  Cocoa first-responder selectors (`undo:` / `copy:` / …) that a Skia view never receives; that is
  the swallowing tangle the Dioxus app fought with per-window shims. Instead each item synthesizes
  its command's effective chord into the focused window's keyboard pipeline
  (`NativeEventExt::send_key_press`), so a menu click and a typed key take the identical path and
  the focused element decides. First-responder semantics without Cocoa — and it retires the shims,
  the `global-hotkey` layer, and the whole per-window-divergent-menu problem F8 was written around.
- **Accelerators are state**: derived from the keymap (`effective_chord`), re-applied live on
  rebind (`sync_chords`, off a destructured `MenuChords` so a new command can't forget one), and
  **suspended** for the life of a chord capture — the OS resolves an accelerator before the window
  sees the key, so an armed menubar makes ⌘C copy instead of bind (P4-08).
- **The menubar is scoped to the focused window** (`MenuScope`: `Project(OpenCtx)` · `Launcher` ·
  `Panel`, resolved into a four-flag `Gate`), and `use_file_menu` lives **inside**
  `use_register_window` so a new kind of window cannot ship without saying what its menubar is.
  This is the corrected form of *menu-follows-opener* — see below.

## Menu-follows-opener, as resolved

F8's target design was "launcher → light menu; project → full; settings → match its opener", and
its reason was the Dioxus app's per-window Edit divergence: custom ⌘A/⌘C items existed only so the
results grid could claim them, non-grid windows needed muda-handler shims, and a Settings window
that closed while carrying a divergent menu stranded its parent's. **None of that survives the
Freya design.** There is one Edit menu, it routes through the focused window's pipeline, and the
grid's ⌘A/⌘C are ordinary keymap listeners. So there is nothing to diverge and nothing to strand.

What actually varies is narrower and real: which *File* and *Window* items the focused window can
carry out. That is the `MenuScope`:

| Scope | Windows | File / Window |
|---|---|---|
| `Project(OpenCtx)` | a project window | everything its arm supports, plus the open path Open Recent resolves through (`OpenPref`) |
| `Launcher` | the launcher | Open… · Open Recent · Settings…; no project to close, save into or open a tab in |
| `Panel` | Settings · Export · Configure | none — greyed, and Close Project leaves the menu entirely |

**The gate is four flags, not a rank**, because *where a command's listener lives* differs per item
and the differences don't nest:

| Flag | Gates | True when |
|---|---|---|
| `workspace` | Open… · Open Recent · Settings… | the launcher or a project window — the same split `WindowKind::is_workspace` draws, asserted equal in `use_register_window` |
| `project` | Close Project | a project window, **in every arm** — its listener is on the window root, and a window whose load failed is the one you most want to close |
| `workbench` | New Query · Save Query | the project subtree is mounted (`OpenCtx::loaded`) — their listeners are in the workbench, so on the loading and fault arms they grey |
| `cyclable` | Cycle Windows | there is a second workspace window; the only flag that is about the app rather than this window |

Ordering these as "how much of a project window is this" was the first shape and it was wrong: a
faulted project window is `workspace + project` but not `workbench`, which no scale expresses.

**Settings is a `Panel`, not "matched to its opener"** — the deliberate divergence from F8. It has
no listener for any File or Window command, so matching its opener would mean showing items that do
nothing. Greying Settings… there is safe either way AppKit resolves a disabled item's key
equivalent (**unverified, and deliberately not relied on**): if the press falls through, this
window's own consuming listener takes it; if the menubar claims it, it stops there. Both end in
"nothing happens", which is right for a window already open.

**The bug this fixed.** Configure and Export never called `use_file_menu` at all, so with either
focused the menubar still carried the owner project window's File menu — and `MenuCmd::CloseProject`
routes at the *focused* window, so File ▸ Close Project (and ⇧⌘W) closed the panel while naming the
project. Making the call part of `use_register_window` is what stops that recurring.

## `Command::CycleWindow`, built here

⌘` had a `COMMANDS` entry, a Keymap row and a palette hint, but its only handler was the project
window's stub (`Command::CycleWindow | Command::Find =>` logging "target not built yet" and
consuming the press) — nothing owned it, and P6-01 had already removed the launcher's half. The
Window menu is where that became visible, so it is built here rather than deferred:

- **`Windows::cycle_from(current)`** — the workspace windows sorted by `WindowId`, the one after
  `current`, wrapping; `None` when it is the only one. Ordered by id because that is arbitrary but
  **stable for a window's life**, so the ring doesn't reshuffle between presses the way a
  `HashMap`'s iteration order would. Not open-order: nothing records that, and a second index to
  hold it would be a register to keep in step for a tie-break nobody can perceive.
- **`use_register_window` now returns this window's id.** It is the one thing a window cannot learn
  any other way, and the hook already waits for it.
- **The command declines when there is nowhere to go** (returns `false`, so the press falls
  through) rather than focusing this window again — and the menubar greys the item on the same
  fact, via `Gate::cyclable`.

`Command::Find` keeps its stub; it belongs to P2-09 and has no menu item.

## Deliberately not built

- **No Close Window in the Window menu.** The predefined item carries ⌘W, which is Close Tab here,
  and a menu accelerator resolves before the window sees the key — it would take the chord app-wide.
- **No Help or Services menu.** Help was a dev-only empty submenu in the Dioxus app; Services needs
  a Cocoa responder a Skia view doesn't provide.
- **Dock-icon Quit stays un-vetoable.** winit 0.31 fixes the root cause — its changelog: *"On
  macOS, remove custom application delegates. You are now allowed to override the application
  delegate yourself."* With our own `NSApplicationDelegate` (objc2 `define_class!`, supported in
  0.31 — not the 0.30 panic trap of #4458), `applicationShouldTerminate` returns
  `NSTerminateCancel` while a query runs and routes to the confirm, vetoing **every** `terminate:`
  path including Dock Quit and logout. The T2 `CloseGuard` bridge is already the right interface —
  the delegate reads the same atomics and pings the same channel, so no app-side redesign.
  **Blockers (2026-08):** still beta (0.31.0-beta.2, Nov 2025; stable line 0.30.13, Mar 2026);
  `accesskit_winit` 0.33.2 still requires winit ^0.30.5, so Freya's a11y can't drop it; and the
  migration is upstream-Freya-sized — 118 beta.1 changelog entries including the **pointer-event
  overhaul** (freya-core's whole event mapping), typed user-events removed (`user_event` →
  `user_wake_up`, i.e. freya-winit's `NativeEvent` proxy architecture), `Resized` →
  `SurfaceResized`, per-platform `WindowAttributes`. Carrying that in the fork alone is heavy
  divergence: do it when upstream Freya migrates, then land the delegate. Until then, accepted
  platform behaviour.

## Known limitations

**A muda `Format(ZeroWidth)` crash is unresolved, not absent** (DEV_TASKS F8). A panic in
`MenuItem::fire_menu_item_click` → `to_nsimage` → `to_png` was reported against muda 0.17.2 —
muda rendering a phantom zero-width icon we never set. Its exact trigger was never captured, and
this task **widened** the custom-`MenuItem` path it came from (10 items to 13) without reproducing
it. Nobody has seen it since P2-20; that is unmeasured, not fixed. If it fires, start from F8's
history rather than refactoring pre-emptively.

**Predefined items carry accelerators we can't take back.** muda sets Hide ⌘H, Hide Others ⌥⌘H and
Minimize ⌘M itself, and `set_accelerator` is `MenuItem`'s, not `PredefinedMenuItem`'s. So those
three chords are effectively reserved: Settings ▸ Keymap will let you bind one, and the OS will
still resolve the menu item first — including mid-capture, where `suspend_accelerators` cannot
reach them. Left as it is because they are macOS-reserved chords anyway (no app lets you rebind
⌘H) and taking them back would mean hand-rolling the three items muda gets right. If it ever bites,
the fix is `keymap::propose` refusing them, beside the policy it already owns — **not** a second
gate in the menu.

## Acceptance
- [x] A decision is recorded; if a menu ships, it follows the opener and uses predefined items where possible.

Predefined where they work: About · Hide/Hide Others/Show All · separators · Minimize · Zoom. Custom
where predefined would send a selector nothing answers (the Edit set) or bypass the close veto
(Quit) — each argued in `menu.rs`.

## Freya / references
Plan §8 (native menu open item). DEV_TASKS F8 (the muda/shortcut analysis + the crash). `menu.rs`,
`platform/windows.rs`. Invariants: "An app-global surface that follows the focused window…" and
"A menubar accelerator is state…" in `docs/reference/INVARIANTS.md`.
