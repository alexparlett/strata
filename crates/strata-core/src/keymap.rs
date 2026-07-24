//! The keymap: the command table and chord resolution, settings-driven end to end.
//!
//! One source of truth ([`COMMANDS`]) holds every command's label, description, default
//! chord, and whether it is fixed. Bindings resolve through [`effective_chord`] — the user
//! override from [`Settings::keybinds`] when present and valid, the built-in default
//! otherwise — so dispatch, menu hints, the Settings ▸ Keymap UI, and hand-edited config
//! JSON all agree. Dispatch itself is distributed (each feature listens for its own
//! command); this module only answers *which* command a chord means.

use crate::config::{Command, KeyChord, Settings};

/// Metadata for one command: display strings + the built-in default chord.
pub struct CommandMeta {
    pub command: Command,
    pub label: &'static str,
    pub desc: &'static str,
    // The default chord as primitives (a `KeyChord` holds a `String`, so it can't be
    // built in a `const` table).
    primary: bool,
    shift: bool,
    alt: bool,
    key: &'static str,
    /// Not rebindable (Esc/dismiss): overrides in the settings are ignored and
    /// [`validate_bind`] rejects any attempt to bind it.
    pub fixed: bool,
}

impl CommandMeta {
    pub fn default_chord(&self) -> KeyChord {
        KeyChord {
            primary: self.primary,
            shift: self.shift,
            alt: self.alt,
            key: self.key.to_string(),
        }
    }
}

macro_rules! command {
    ($command:ident, $label:literal, $desc:literal, [$($cap:ident)*] $key:literal $(, $fixed:ident)?) => {
        CommandMeta {
            command: Command::$command,
            label: $label,
            desc: $desc,
            primary: command!(@has primary [$($cap)*]),
            shift: command!(@has shift [$($cap)*]),
            alt: command!(@has alt [$($cap)*]),
            key: $key,
            fixed: command!(@fixed $($fixed)?),
        }
    };
    (@has $want:ident []) => { false };
    (@has primary [primary $($rest:ident)*]) => { true };
    (@has shift [shift $($rest:ident)*]) => { true };
    (@has alt [alt $($rest:ident)*]) => { true };
    (@has $want:ident [$other:ident $($rest:ident)*]) => { command!(@has $want [$($rest)*]) };
    (@fixed) => { false };
    (@fixed fixed) => { true };
}

/// Every command, in display order — which is also the **resolution order**: if two
/// bindings ever hold the same chord, the first entry wins, deterministically.
pub const COMMANDS: &[CommandMeta] = &[
    command!(CommandPalette, "Command palette", "Toggle the command palette", [primary] "k"),
    command!(NewTab, "New query tab", "Open a new query tab", [primary] "t"),
    command!(ReopenTab, "Reopen closed tab", "Reopen the last closed tab", [primary shift] "t"),
    command!(CloseActiveTab, "Close tab", "Close the current query tab", [primary] "w"),
    command!(
        CloseProject,
        "Close project",
        "Close the project window and return to the launcher",
        [primary] "q"
    ),
    command!(RunQuery, "Run query", "Execute the current query", [primary] "Enter"),
    command!(SaveQuery, "Save query", "Save the active query to the project", [primary] "s"),
    command!(Undo, "Undo", "Undo the last edit in the query editor", [primary] "z"),
    command!(Redo, "Redo", "Redo the last undone edit in the query editor", [primary shift] "z"),
    command!(Cut, "Cut", "Cut the selection in the query editor", [primary] "x"),
    command!(Copy, "Copy", "Copy the selection in the query editor or results grid", [primary] "c"),
    command!(Paste, "Paste", "Paste the clipboard into the query editor", [primary] "v"),
    command!(SelectAll, "Select all", "Select the query editor's whole buffer or every results cell", [primary] "a"),
    command!(Find, "Find in results", "Search within the results grid", [primary] "f"),
    command!(OpenSettings, "Open settings", "Open the settings window", [primary] ","),
    command!(
        CycleWindow,
        "Cycle windows",
        "Move focus between open project windows",
        [primary] "`"
    ),
    command!(Cancel, "Dismiss", "Close overlays and menus", [] "Escape", fixed),
];

fn meta(cmd: Command) -> &'static CommandMeta {
    COMMANDS
        .iter()
        .find(|m| m.command == cmd)
        .expect("every Command has a COMMANDS entry (enforced by test)")
}

/// (label, description) for display (Settings ▸ Keymap rows, the command palette).
pub fn describe(cmd: Command) -> (&'static str, &'static str) {
    let m = meta(cmd);
    (m.label, m.desc)
}

pub fn default_chord(cmd: Command) -> KeyChord {
    meta(cmd).default_chord()
}

pub fn is_fixed(cmd: Command) -> bool {
    meta(cmd).fixed
}

/// Why a chord can't be bound to a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindError {
    /// The command is fixed (Esc/dismiss) and can't be rebound or unbound.
    FixedCommand,
    /// Rebindable chords must hold the primary modifier (⌘/Ctrl) so they can't collide
    /// with plain typing.
    MissingPrimary,
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FixedCommand => write!(f, "this shortcut can't be changed"),
            Self::MissingPrimary => write!(f, "shortcuts need ⌘ (or Ctrl)"),
        }
    }
}

/// Whether `chord` may be bound to `cmd`. The single conflict-policy funnel: the
/// Settings ▸ Keymap capture UI and hand-edited config entries (via [`effective_chord`])
/// both go through it.
pub fn validate_bind(cmd: Command, chord: &KeyChord) -> Result<(), BindError> {
    if is_fixed(cmd) {
        return Err(BindError::FixedCommand);
    }
    if !chord.primary {
        return Err(BindError::MissingPrimary);
    }
    Ok(())
}

/// The chord that actually triggers `cmd`: the user override when present and valid
/// (`None` = explicit unbind), the built-in default otherwise. Invalid overrides —
/// including any override of a fixed command — are ignored with a warning, falling back
/// to the default, so a bad hand-edit can never brick a shortcut.
pub fn effective_chord(settings: &Settings, cmd: Command) -> Option<KeyChord> {
    let bind = settings.keybinds.iter().find(|b| b.command == cmd);
    match bind {
        None => Some(default_chord(cmd)),
        Some(bind) => match &bind.chord {
            None if is_fixed(cmd) => {
                tracing::warn!("ignoring unbind of fixed command {cmd:?}");
                Some(default_chord(cmd))
            }
            None => None,
            Some(chord) => match validate_bind(cmd, chord) {
                Ok(()) => Some(chord.clone()),
                Err(err) => {
                    tracing::warn!("ignoring invalid bind {chord:?} for {cmd:?}: {err}");
                    Some(default_chord(cmd))
                }
            },
        },
    }
}

/// The first command (in [`COMMANDS`] order) whose effective chord matches.
pub fn resolve(settings: &Settings, chord: &KeyChord) -> Option<Command> {
    COMMANDS
        .iter()
        .find(|m| effective_chord(settings, m.command).as_ref() == Some(chord))
        .map(|m| m.command)
}

/// The chord as display key caps, canvas modifier order (⇧ ⌥ ⌘) then the key:
/// `["⇧", "⌘", "T"]`.
pub fn chord_caps(chord: &KeyChord) -> Vec<String> {
    let mut caps = Vec::new();
    if chord.shift {
        caps.push("⇧".to_string());
    }
    if chord.alt {
        caps.push("⌥".to_string());
    }
    if chord.primary {
        caps.push("⌘".to_string());
    }
    caps.push(key_cap(&chord.key));
    caps
}

fn key_cap(key: &str) -> String {
    match key {
        "Enter" => "↵".to_string(),
        "Escape" => "Esc".to_string(),
        "Tab" => "⇥".to_string(),
        " " => "Space".to_string(),
        "ArrowUp" => "↑".to_string(),
        "ArrowDown" => "↓".to_string(),
        "ArrowLeft" => "←".to_string(),
        "ArrowRight" => "→".to_string(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => k.to_string(),
    }
}

/// The effective chord as one compact hint string (`"⇧⌘T"`), or `""` when unbound —
/// drop the surrounding label too when empty.
pub fn hint(settings: &Settings, cmd: Command) -> String {
    effective_chord(settings, cmd)
        .map(|chord| chord_caps(&chord).concat())
        .unwrap_or_default()
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::config::KeyBind;

    fn chord(primary: bool, shift: bool, key: &str) -> KeyChord {
        KeyChord {
            primary,
            shift,
            alt: false,
            key: key.to_string(),
        }
    }

    fn settings_with(binds: Vec<KeyBind>) -> Settings {
        Settings {
            keybinds: binds,
            ..Settings::default()
        }
    }

    #[test]
    fn every_command_has_a_table_entry() {
        // `meta` unwraps on this invariant; prove it for every variant.
        for cmd in [
            Command::Find,
            Command::NewTab,
            Command::ReopenTab,
            Command::CloseActiveTab,
            Command::CloseProject,
            Command::SaveQuery,
            Command::RunQuery,
            Command::Undo,
            Command::Redo,
            Command::Cut,
            Command::Copy,
            Command::Paste,
            Command::SelectAll,
            Command::CommandPalette,
            Command::OpenSettings,
            Command::CycleWindow,
            Command::Cancel,
        ] {
            let (label, desc) = describe(cmd);
            assert!(!label.is_empty() && !desc.is_empty(), "{cmd:?}");
        }
        assert_eq!(COMMANDS.len(), 17);
    }

    #[test]
    fn defaults_resolve() {
        let s = Settings::default();
        assert_eq!(resolve(&s, &chord(true, false, "t")), Some(Command::NewTab));
        assert_eq!(resolve(&s, &chord(true, true, "t")), Some(Command::ReopenTab));
        assert_eq!(resolve(&s, &chord(true, false, "Enter")), Some(Command::RunQuery));
        assert_eq!(resolve(&s, &chord(false, false, "Escape")), Some(Command::Cancel));
        assert_eq!(resolve(&s, &chord(true, false, "`")), Some(Command::CycleWindow));
        // The text-editing commands are ordinary bindings now.
        assert_eq!(resolve(&s, &chord(true, false, "z")), Some(Command::Undo));
        assert_eq!(resolve(&s, &chord(true, true, "z")), Some(Command::Redo));
        assert_eq!(resolve(&s, &chord(true, false, "x")), Some(Command::Cut));
        assert_eq!(resolve(&s, &chord(true, false, "c")), Some(Command::Copy));
        assert_eq!(resolve(&s, &chord(true, false, "v")), Some(Command::Paste));
        assert_eq!(resolve(&s, &chord(true, false, "a")), Some(Command::SelectAll));
        // ⌘Y was the text layer's legacy redo — unbound by default here.
        assert_eq!(resolve(&s, &chord(true, false, "y")), None);
        // Bare keys never resolve to a rebindable command.
        assert_eq!(resolve(&s, &chord(false, false, "t")), None);
    }

    #[test]
    fn override_wins_and_frees_the_default() {
        let s = settings_with(vec![KeyBind {
            command: Command::RunQuery,
            chord: Some(chord(true, false, "r")),
        }]);
        assert_eq!(resolve(&s, &chord(true, false, "r")), Some(Command::RunQuery));
        // The default ⌘↵ no longer matches anything.
        assert_eq!(resolve(&s, &chord(true, false, "Enter")), None);
    }

    #[test]
    fn edit_commands_are_rebindable() {
        // The whole point of making the editing layer configurable: bind ⌘Y as undo
        // and the default ⌘Z stops resolving (the editor gate then swallows it).
        let s = settings_with(vec![KeyBind {
            command: Command::Undo,
            chord: Some(chord(true, false, "y")),
        }]);
        assert_eq!(resolve(&s, &chord(true, false, "y")), Some(Command::Undo));
        assert_eq!(resolve(&s, &chord(true, false, "z")), None);
        assert_eq!(resolve(&s, &chord(true, true, "z")), Some(Command::Redo));

        // Same for the clipboard set.
        let s = settings_with(vec![KeyBind {
            command: Command::Paste,
            chord: Some(chord(true, true, "v")),
        }]);
        assert_eq!(resolve(&s, &chord(true, true, "v")), Some(Command::Paste));
        assert_eq!(resolve(&s, &chord(true, false, "v")), None);
    }

    #[test]
    fn edit_classification() {
        for cmd in [
            Command::Undo,
            Command::Redo,
            Command::Cut,
            Command::Copy,
            Command::Paste,
            Command::SelectAll,
        ] {
            assert!(cmd.is_edit(), "{cmd:?}");
        }
        assert!(!Command::RunQuery.is_edit());
        assert!(!Command::Cancel.is_edit());
    }

    #[test]
    fn explicit_unbind() {
        let s = settings_with(vec![KeyBind {
            command: Command::SaveQuery,
            chord: None,
        }]);
        assert_eq!(effective_chord(&s, Command::SaveQuery), None);
        assert_eq!(resolve(&s, &chord(true, false, "s")), None);
        assert_eq!(hint(&s, Command::SaveQuery), "");
    }

    #[test]
    fn invalid_overrides_fall_back_to_default() {
        // No primary modifier / any touch of a fixed command: ignored, default restored.
        let s = settings_with(vec![KeyBind {
            command: Command::Find,
            chord: Some(chord(false, false, "f")),
        }]);
        assert_eq!(
            effective_chord(&s, Command::Find),
            Some(default_chord(Command::Find))
        );
        let s = settings_with(vec![
            KeyBind { command: Command::Cancel, chord: Some(chord(true, false, "d")) },
            KeyBind { command: Command::Cancel, chord: None },
        ]);
        assert_eq!(
            effective_chord(&s, Command::Cancel),
            Some(default_chord(Command::Cancel))
        );
    }

    #[test]
    fn validate_bind_policy() {
        assert_eq!(
            validate_bind(Command::Cancel, &chord(true, false, "d")),
            Err(BindError::FixedCommand)
        );
        assert_eq!(
            validate_bind(Command::Find, &chord(false, false, "f")),
            Err(BindError::MissingPrimary)
        );
        // Editing chords are no longer reserved: any primary chord may be bound to any
        // rebindable command — duplicates resolve deterministically in COMMANDS order.
        assert_eq!(validate_bind(Command::Find, &chord(true, false, "c")), Ok(()));
        assert_eq!(validate_bind(Command::Undo, &chord(true, false, "y")), Ok(()));
        assert_eq!(validate_bind(Command::Find, &chord(true, true, "g")), Ok(()));
    }

    #[test]
    fn duplicate_chords_resolve_in_table_order() {
        // Bind SaveQuery to NewTab's ⌘T: NewTab sits earlier in COMMANDS, so ⌘T stays
        // NewTab's — deterministically — until the duplicate is removed.
        let s = settings_with(vec![KeyBind {
            command: Command::SaveQuery,
            chord: Some(chord(true, false, "t")),
        }]);
        assert_eq!(resolve(&s, &chord(true, false, "t")), Some(Command::NewTab));
    }

    #[test]
    fn caps_and_hints() {
        let s = Settings::default();
        assert_eq!(chord_caps(&default_chord(Command::ReopenTab)), ["⇧", "⌘", "T"]);
        assert_eq!(hint(&s, Command::RunQuery), "⌘↵");
        assert_eq!(hint(&s, Command::Cancel), "Esc");
        assert_eq!(hint(&s, Command::CycleWindow), "⌘`");
        assert_eq!(hint(&s, Command::OpenSettings), "⌘,");
        assert_eq!(hint(&s, Command::Undo), "⌘Z");
        assert_eq!(hint(&s, Command::Redo), "⇧⌘Z");
    }
}
