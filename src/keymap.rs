//! Keybindings. `app::handle_key` builds a [`KeyChord`] from each key event, [`resolve`]s
//! it to a [`Command`], and [`run`]s it — so rebinding a shortcut is a pure keymap edit and
//! the global handler never learns feature specifics.
//!
//! Two layers, kept separate (VS Code-style):
//! - **keymap** ([`resolve`]): which chord triggers which command — user-overridable,
//!   persisted in [`crate::config::Settings::keybinds`] (empty = the [`COMMANDS`] defaults).
//! - **dispatch** ([`run`]): what a command *does* — a fixed action / direct call, or, for a
//!   context-dependent command (⌘F `Find`), whatever mounted context [`register`]ed for it.
//!
//! Command metadata (order, label, description, default chord, whether it's an OS global
//! hotkey) lives in the single [`COMMANDS`] table; only the *action* is a per-command arm,
//! in [`run`]. Adding or rebinding a command touches the table + one `run` arm.

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

/// Per-command metadata — the **single source of truth** for order, label, description, the
/// built-in default chord (stored as primitives since `KeyChord` isn't const-constructible),
/// and whether the command is delivered via the OS global-hotkey layer. [`all_commands`] /
/// [`describe`] / [`default_chord`] / [`is_global`] all derive from it; the *action* stays a
/// per-command arm in [`run`].
struct CommandMeta {
    command: Command,
    label: &'static str,
    desc: &'static str,
    primary: bool,
    shift: bool,
    alt: bool,
    key: &'static str,
    /// Delivered via the OS global-hotkey layer (`crate::hotkeys`) rather than the
    /// focused-component / DOM layer (`Cancel` = Esc).
    global: bool,
}

/// The command table, in display order. Edited to add / retune a command (its *action* is
/// the matching arm in [`run`]).
const COMMANDS: &[CommandMeta] = &[
    CommandMeta { command: Command::CommandPalette, label: "Command palette", desc: "Search tables, columns & commands", primary: true, shift: false, alt: false, key: "k", global: true },
    CommandMeta { command: Command::NewTab, label: "New query tab", desc: "Open a fresh SQL tab", primary: true, shift: false, alt: false, key: "t", global: true },
    CommandMeta { command: Command::ReopenTab, label: "Reopen closed tab", desc: "Restore the last tab you closed", primary: true, shift: true, alt: false, key: "t", global: true },
    CommandMeta { command: Command::CloseActiveTab, label: "Close tab", desc: "Close the active query tab", primary: true, shift: false, alt: false, key: "w", global: true },
    CommandMeta { command: Command::SaveQuery, label: "Save query", desc: "Save the active query to the project", primary: true, shift: false, alt: false, key: "s", global: true },
    CommandMeta { command: Command::RunQuery, label: "Run query", desc: "Execute the current SQL", primary: true, shift: false, alt: false, key: "Enter", global: true },
    CommandMeta { command: Command::Find, label: "Find in results", desc: "Search within the results grid", primary: true, shift: false, alt: false, key: "f", global: true },
    CommandMeta { command: Command::OpenSettings, label: "Settings", desc: "Open the Settings window", primary: true, shift: false, alt: false, key: ",", global: true },
    CommandMeta { command: Command::CycleWindow, label: "Cycle windows", desc: "Focus the next project window", primary: true, shift: false, alt: false, key: "`", global: true },
    CommandMeta { command: Command::Cancel, label: "Dismiss", desc: "Close overlays & menus, or cancel a running query", primary: false, shift: false, alt: false, key: "Escape", global: false },
];

/// The metadata for `cmd` (every [`Command`] variant is listed in [`COMMANDS`]).
fn meta(cmd: Command) -> &'static CommandMeta {
    COMMANDS
        .iter()
        .find(|m| m.command == cmd)
        .expect("every Command is listed in COMMANDS")
}

/// Every command, in display order. [`resolve`] scans their effective chords, and the
/// Settings ▸ Keymap list renders from this — so the list always reflects the real,
/// override-aware bindings ([`effective_chord`]) rather than a parallel hardcoded copy.
pub fn all_commands() -> impl Iterator<Item=Command> {
    COMMANDS.iter().map(|m| m.command)
}

/// The commands eligible for an OS global hotkey — everything except the context keys
/// (`Cancel`/Esc, which stay on the focused component). The `hotkeys` module registers
/// these; `handle_key` runs only the *non*-global ones so the two layers never double-fire.
pub fn global_commands() -> impl Iterator<Item=Command> {
    COMMANDS.iter().filter(|m| m.global).map(|m| m.command)
}

/// Whether `cmd` is delivered by the global-hotkey layer (vs the focused-component / DOM
/// layer — i.e. `Cancel`).
pub fn is_global(cmd: Command) -> bool {
    meta(cmd).global
}

/// A short label + description for `cmd` — the single source for the Settings ▸ Keymap list
/// (and, later, the command palette).
pub fn describe(cmd: Command) -> (&'static str, &'static str) {
    let m = meta(cmd);
    (m.label, m.desc)
}

/// The built-in default chord for `cmd` (used when no user override binds it).
pub fn default_chord(cmd: Command) -> KeyChord {
    let m = meta(cmd);
    KeyChord {
        primary: m.primary,
        shift: m.shift,
        alt: m.alt,
        key: m.key.to_string(),
    }
}

/// The chord bound to `cmd` right now — a user override if present (which may be an
/// **explicit unbind** → `None`), else the default. `None` means the command has no shortcut.
pub fn effective_chord(cmd: Command) -> Option<KeyChord> {
    match crate::settings::keybinds().into_iter().find(|b| b.command == cmd) {
        Some(b) => b.chord,
        None => Some(default_chord(cmd)),
    }
}

/// The primary modifier's cap symbol — ⌘ on macOS, `Ctrl` elsewhere (matches how a
/// chord's `primary` resolves from `meta || ctrl`).
#[cfg(target_os = "macos")]
const PRIMARY_CAP: &str = "⌘";
#[cfg(not(target_os = "macos"))]
const PRIMARY_CAP: &str = "Ctrl";

/// The display key-caps for `chord` — modifiers (primary / ⇧ / ⌥) then the key,
/// e.g. `["⌘", "⇧", "T"]`. Drives the Keymap list; [`hint`] joins them for tooltips.
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

/// The compact display of `cmd`'s current chord for tooltips / kbd chips, e.g. `"⌘K"` /
/// `"⌘⇧T"` / `"⌘↵"` — or an **empty string** if `cmd` is unbound (so a `kbd` chip vanishes
/// and a `title` can drop it). Tracks the live, override-aware binding.
pub fn hint(cmd: Command) -> String {
    match effective_chord(cmd) {
        Some(c) => chord_caps(&c).join(""),
        None => String::new(),
    }
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
/// `meta || ctrl` treatment). Public so the Settings ▸ Keymap rebinding capture can turn a
/// keypress into a chord.
pub fn chord_from_event(e: &KeyboardEvent) -> KeyChord {
    let m = e.modifiers();
    KeyChord {
        primary: m.meta() || m.ctrl(),
        shift: m.shift(),
        alt: m.alt(),
        key: normalize_key(&e.key()),
    }
}

/// Whether a key event is a bare modifier press (⌘ / ⇧ / ⌥ / Ctrl, no other key) — chord
/// capture ignores these and waits for the real key.
pub fn is_modifier_key(e: &KeyboardEvent) -> bool {
    matches!(e.key(), Key::Meta | Key::Shift | Key::Alt | Key::Control)
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
    all_commands().find(|&cmd| effective_chord(cmd).as_ref() == Some(&chord))
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
            // Overlays self-close on Esc (Dialog / Backdrop own their own handler); this
            // guard just stops Esc from *also* cancelling a running query while one is open.
            if !crate::overlays::any_open() {
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
