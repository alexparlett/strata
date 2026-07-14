//! OS-level **global keyboard commands** (⌘F/⌘K/…), via the `global-hotkey` crate Dioxus
//! bundles + the desktop context's shortcut registry. Everything hotkey-related lives here:
//! the keymap-chord→`HotKey` mapping and [`use_shortcuts`], which registers the global
//! commands **while the window is focused** (so the chords aren't grabbed system-wide in
//! the background) and dispatches a fired command back on the UI thread, in-scope.
//!
//! Why the relay: the registry invokes the callback from the tao loop with a runtime but no
//! Dioxus *scope*, so it can't dispatch directly (`dispatch` may call `window()`, which
//! reads scope context and would panic). The callback parks the command in [`PENDING`] — a
//! per-window global signal; a signal write needs only the runtime, which we re-enter with
//! a `RuntimeGuard` — and a scoped effect drains it through the keymap.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::core::{Runtime, RuntimeGuard};
use dioxus::desktop::{use_window, window, ShortcutHandle};
use dioxus::prelude::*;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::HotKeyState;

use crate::config::Command;
use crate::state::AppState;

/// A fired hotkey's command, parked by the (scope-less) registry callback for the scoped
/// consumer effect in [`use_shortcuts`]. Per-window (a global is per-`VirtualDom`).
static PENDING: GlobalSignal<Option<Command>> = Signal::global(|| None);

/// Install the app's global keyboard shortcuts for this window — call once from the root
/// component. `focused` is the window's focus state (set from the wry `Focused` event):
/// shortcuts are (re)registered while focused and removed on blur, so they aren't held
/// system-wide while Strata is backgrounded.
pub fn use_shortcuts(state: Signal<AppState>) {
    // Drain parked commands in-scope: dispatch may call `window()`, which the callback's
    // bare runtime can't satisfy, but this effect runs inside the component scope.
    use_effect(move || {
        let pending = *PENDING.read();
        if let Some(cmd) = pending {
            *PENDING.write() = None;
            crate::keymap::run(state, cmd);
        }
    });

    let win = use_window();
    let focused = use_signal(|| win.is_focused());

    // (Re)register on focus; remove on blur. Handles live in a plain `Rc<RefCell>`
    // (not a signal) so the `use_drop` below can safely touch them during teardown,
    // when a `use_signal`'s backing box may already be freed.
    let handles: Rc<RefCell<Vec<ShortcutHandle>>> = use_hook(|| Rc::new(RefCell::new(Vec::new())));
    use_effect({
        let handles = handles.clone();
        move || {
            let win = window();
            let rt = Runtime::current();
            for h in handles.borrow_mut().drain(..) {
                win.remove_shortcut(h);
            }
            if focused() {
                for &cmd in crate::keymap::GLOBAL {
                    let Some(hk) = hotkey_for(cmd) else {
                        continue;
                    };
                    // Registered only while focused (this effect gate), so the callback
                    // needs no focus check — the register/remove lifecycle is the gate.
                    let rt = rt.clone();
                    let registered = win.create_shortcut(hk, move |st| {
                        if st == HotKeyState::Pressed {
                            // Re-enter the runtime so the global-signal write is valid; the
                            // scoped effect above then dispatches it.
                            let _guard = RuntimeGuard::new(rt.clone());
                            *PENDING.write() = Some(cmd);
                        }
                    });
                    if let Ok(h) = registered {
                        handles.borrow_mut().push(h);
                    }
                }
            }
        }
    });

    // Remove this window's OS hotkeys when it closes. The shortcut registry is
    // app-global (shared across windows), so a handle left registered after the
    // window's `VirtualDom` is dropped would fire its callback under this window's
    // now-dead runtime and panic on the `PENDING` global-signal write. Blur usually
    // clears them, but a close *while focused* races the teardown — so drop them
    // deterministically here. `win` is captured (not looked up via context) so it's
    // valid during unmount.
    let win = use_hook(window);
    use_drop(move || {
        for h in handles.borrow_mut().drain(..) {
            win.remove_shortcut(h);
        }
    });
}

/// The OS hotkey for `cmd`, from its effective (possibly rebound) chord — `None` if the
/// key doesn't map to a `Code`.
fn hotkey_for(cmd: Command) -> Option<HotKey> {
    let chord = crate::keymap::effective_chord(cmd);
    let mut mods = Modifiers::empty();
    if chord.primary {
        // The platform primary modifier: ⌘ on macOS, Ctrl elsewhere.
        #[cfg(target_os = "macos")]
        {
            mods |= Modifiers::META;
        }
        #[cfg(not(target_os = "macos"))]
        {
            mods |= Modifiers::CONTROL;
        }
    }
    if chord.shift {
        mods |= Modifiers::SHIFT;
    }
    if chord.alt {
        mods |= Modifiers::ALT;
    }
    Some(HotKey::new(Some(mods), code_for(&chord.key)?))
}

/// Map a normalized keymap key name to a `global-hotkey` `Code`. Unknown keys → `None`
/// (that command simply gets no hotkey).
fn code_for(key: &str) -> Option<Code> {
    Some(match key {
        "a" => Code::KeyA,
        "b" => Code::KeyB,
        "c" => Code::KeyC,
        "d" => Code::KeyD,
        "e" => Code::KeyE,
        "f" => Code::KeyF,
        "g" => Code::KeyG,
        "h" => Code::KeyH,
        "i" => Code::KeyI,
        "j" => Code::KeyJ,
        "k" => Code::KeyK,
        "l" => Code::KeyL,
        "m" => Code::KeyM,
        "n" => Code::KeyN,
        "o" => Code::KeyO,
        "p" => Code::KeyP,
        "q" => Code::KeyQ,
        "r" => Code::KeyR,
        "s" => Code::KeyS,
        "t" => Code::KeyT,
        "u" => Code::KeyU,
        "v" => Code::KeyV,
        "w" => Code::KeyW,
        "x" => Code::KeyX,
        "y" => Code::KeyY,
        "z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        "Enter" => Code::Enter,
        "," => Code::Comma,
        "`" => Code::Backquote,
        _ => return None,
    })
}
