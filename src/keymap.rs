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

/// Every command, in display order. [`resolve`] scans their effective chords, and
/// the Settings ▸ Keymap list renders from this — so that list always reflects the
/// real, override-aware bindings ([`effective_chord`]) rather than a parallel
/// hardcoded copy.
pub const ALL_COMMANDS: [Command; 10] = [
    Command::CommandPalette,
    Command::NewTab,
    Command::ReopenTab,
    Command::CloseActiveTab,
    Command::SaveQuery,
    Command::RunQuery,
    Command::Find,
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
    crate::settings::keybinds()
        .iter()
        .find(|b| b.command == cmd)
        .map(|b| b.chord.clone())
        .unwrap_or_else(|| default_chord(cmd))
}

/// A short label + description for `cmd` — the single source for the Settings ▸
/// Keymap list (and, later, the command palette).
pub fn describe(cmd: Command) -> (&'static str, &'static str) {
    use Command::*;
    match cmd {
        CommandPalette => ("Command palette", "Search tables, columns & commands"),
        NewTab => ("New query tab", "Open a fresh SQL tab"),
        ReopenTab => ("Reopen closed tab", "Restore the last tab you closed"),
        CloseActiveTab => ("Close tab", "Close the active query tab"),
        SaveQuery => ("Save query", "Save the active query to the project"),
        RunQuery => ("Run query", "Execute the current SQL"),
        Find => ("Find in results", "Search within the results grid"),
        OpenSettings => ("Settings", "Open the Settings window"),
        CycleWindow => ("Cycle windows", "Focus the next project window"),
        Cancel => ("Dismiss", "Close overlays & menus, or cancel a running query"),
    }
}

/// The primary modifier's cap symbol — ⌘ on macOS, `Ctrl` elsewhere (matches how a
/// chord's `primary` resolves from `meta || ctrl`).
#[cfg(target_os = "macos")]
const PRIMARY_CAP: &str = "⌘";
#[cfg(not(target_os = "macos"))]
const PRIMARY_CAP: &str = "Ctrl";

/// The display key-caps for `chord` — modifiers (primary / ⇧ / ⌥) then the key,
/// e.g. `["⌘", "⇧", "T"]`. Drives the read-only Keymap list; rebinding (W4) writes
/// back through [`effective_chord`] so the display stays in lockstep.
pub fn chord_caps(chord: &KeyChord) -> Vec<String> {
    let mut caps = Vec::new();
    if chord.primary {
        caps.push(PRIMARY_CAP.to_string());
    }
    if chord.shift {
        caps.push("⇧".to_string());
    }
    if chord.alt {
        caps.push("⌥".to_string());
    }
    caps.push(key_cap(&chord.key));
    caps
}

/// Display form of a normalized key name (`Enter` → `↵`, single chars upper-cased).
fn key_cap(key: &str) -> String {
    match key {
        "Enter" => "↵".to_string(),
        "Escape" => "Esc".to_string(),
        "Tab" => "⇥".to_string(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => k.to_string(),
    }
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
    ALL_COMMANDS
        .into_iter()
        .find(|&cmd| effective_chord(cmd) == chord)
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
            crate::window::spawn_settings_window();
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
