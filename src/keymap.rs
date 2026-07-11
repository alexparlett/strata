//! Keybindings. `app::handle_key` builds a [`KeyChord`] from each key event, [`resolve`]s
//! it to a [`Command`], and [`run`]s it — so rebinding a shortcut is a pure keymap edit and
//! the global handler never learns feature specifics.
//!
//! Two layers, kept separate (VS Code-style):
//! - **keymap** ([`resolve`]): which chord triggers which command — user-overridable,
//!   persisted in [`crate::config::Settings::keybinds`] (empty = the [`default_chord`] table).
//! - **dispatch** ([`run`]): what a command *does* — a fixed action / direct call, or, for a
//!   context-dependent command (⌘F `Find`), whatever mounted context [`register`]ed for it.

use std::collections::HashMap;

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::config::{Command, KeyChord};
use crate::session::WorkspaceId;
use crate::state::AppState;

/// Which mounted context currently owns a context-dependent command. Plain `Copy` data
/// (not a closure), so it lives safely in a per-window `GlobalSignal`; [`run`] maps the
/// kind to an action. New contexts (e.g. an editor find) add a variant here — never a
/// literal-key arm in `handle_key`.
#[derive(Clone, Copy, PartialEq)]
pub enum Context {
    ResultsFind,
}

/// Per-window registry: a context command → its active owner. Written by the owning
/// component's mount effect; read by [`run`].
static REGISTRY: GlobalSignal<HashMap<Command, (WorkspaceId, Context)>> =
    Signal::global(|| HashMap::new());

/// Claim `cmd` for `owner` while it's the active context (last writer wins).
pub fn register(cmd: Command, owner: WorkspaceId, ctx: Context) {
    REGISTRY.write().insert(cmd, (owner, ctx));
}

/// Release `cmd` — but only if `owner` still holds it (safe to call from any tab's
/// deactivate / unmount without stealing the slot from whoever owns it now).
pub fn unregister_if(cmd: Command, owner: WorkspaceId) {
    let mut reg = REGISTRY.write();
    if reg.get(&cmd).map(|(o, _)| *o == owner).unwrap_or(false) {
        reg.remove(&cmd);
    }
}

/// Commands eligible for an OS global hotkey — everything except the context keys
/// (`Cancel`/Esc, which stay on the focused component). The `hotkeys` module registers
/// these; `handle_key` runs only the *non*-global ones so the two layers never double-fire.
pub const GLOBAL: &[Command] = &[
    Command::Find,
    Command::NewTab,
    Command::ReopenTab,
    Command::CloseActiveTab,
    Command::SaveQuery,
    Command::RunQuery,
    Command::CommandPalette,
    Command::OpenSettings,
    Command::CycleWindow,
];

/// Whether `cmd` is delivered by the global-hotkey layer (vs the focused-component / DOM
/// layer — i.e. `Cancel`).
pub fn is_global(cmd: Command) -> bool {
    GLOBAL.contains(&cmd)
}

/// Every command, so [`resolve`] can scan their effective chords.
const ALL: [Command; 10] = [
    Command::Find,
    Command::NewTab,
    Command::ReopenTab,
    Command::CloseActiveTab,
    Command::SaveQuery,
    Command::RunQuery,
    Command::CommandPalette,
    Command::OpenSettings,
    Command::CycleWindow,
    Command::Cancel,
];

/// The built-in default chord for `cmd` (used when no user override binds it).
fn default_chord(cmd: Command) -> KeyChord {
    use Command::*;
    let c = |primary: bool, shift: bool, key: &str| KeyChord {
        primary,
        shift,
        alt: false,
        key: key.to_string(),
    };
    match cmd {
        Find => c(true, false, "f"),
        NewTab => c(true, false, "t"),
        ReopenTab => c(true, true, "t"),
        CloseActiveTab => c(true, false, "w"),
        SaveQuery => c(true, false, "s"),
        RunQuery => c(true, false, "Enter"),
        CommandPalette => c(true, false, "k"),
        OpenSettings => c(true, false, ","),
        CycleWindow => c(true, false, "`"),
        Cancel => c(false, false, "Escape"),
    }
}

/// The chord bound to `cmd` right now — a user override if present, else the default.
pub fn effective_chord(cmd: Command) -> KeyChord {
    crate::settings::SETTINGS
        .resolve()
        .peek()
        .keybinds
        .iter()
        .find(|b| b.command == cmd)
        .map(|b| b.chord.clone())
        .unwrap_or_else(|| default_chord(cmd))
}

/// Normalize a key event into a chord (folding ⌘/Ctrl into `primary`, matching the old
/// `meta || ctrl` treatment).
fn chord_from_event(e: &KeyboardEvent) -> KeyChord {
    let m = e.modifiers();
    KeyChord {
        primary: m.meta() || m.ctrl(),
        shift: m.shift(),
        alt: m.alt(),
        key: normalize_key(&e.key()),
    }
}

/// A canonical key name: lowercased character, or a named key we bind (`Enter`/`Escape`).
fn normalize_key(k: &Key) -> String {
    match k {
        Key::Character(c) => c.to_lowercase(),
        Key::Enter => "Enter".to_string(),
        Key::Escape => "Escape".to_string(),
        Key::Tab => "Tab".to_string(),
        other => format!("{other:?}"),
    }
}

/// The command currently bound to this key event, if any.
pub fn resolve(e: &KeyboardEvent) -> Option<Command> {
    let chord = chord_from_event(e);
    ALL.into_iter().find(|&cmd| effective_chord(cmd) == chord)
}

/// Execute `cmd`. Returns whether it did anything — so `handle_key` only calls
/// `prevent_default` on a handled key (an unowned `Find` falls through untouched).
pub fn run(state: Signal<AppState>, cmd: Command) -> bool {
    use Command::*;
    match cmd {
        Find => fire_find(state),
        NewTab => {
            dispatch(state, Action::NewTab);
            true
        }
        ReopenTab => {
            dispatch(state, Action::ReopenTab);
            true
        }
        CloseActiveTab => {
            let active = crate::session::active_id();
            if active != 0 {
                dispatch(state, Action::CloseTab(active));
                true
            } else {
                false
            }
        }
        SaveQuery => {
            dispatch(state, Action::SaveQuery);
            true
        }
        RunQuery => {
            dispatch(state, Action::RunQuery);
            true
        }
        CommandPalette => {
            crate::overlays::toggle_cmdk();
            true
        }
        OpenSettings => {
            crate::overlays::toggle_settings();
            true
        }
        CycleWindow => {
            crate::window::cycle_to_next_window();
            true
        }
        Cancel => {
            if crate::overlays::any_open() {
                dispatch(state, Action::CloseOverlays);
            } else {
                dispatch(state, Action::CancelQuery);
            }
            true
        }
    }
}

/// Toggle the find popover of whichever results toolbar owns `Find` (the active tab).
/// No-op when nothing owns it (no results showing → nothing to find).
fn fire_find(state: Signal<AppState>) -> bool {
    match REGISTRY.peek().get(&Command::Find).copied() {
        Some((ws, Context::ResultsFind)) => {
            let open = crate::runs::RUNS
                .resolve()
                .get(ws)
                .map(|e| e.peek().find_open)
                .unwrap_or(false);
            dispatch(state, Action::SetResultsFind { ws, open: !open });
            true
        }
        None => false,
    }
}
