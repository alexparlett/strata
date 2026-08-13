//! What the Keymap pane is showing, and what it is in the middle of doing.
//!
//! **The bindings are not modelled here.** Unlike the Engine pane's rows, a keybinding is
//! already the shape the setting stores — `Settings::keybinds` holds exactly what a rebind
//! writes, and a chord is captured whole rather than typed a character at a time — so there is
//! no intermediate list to keep, and [`rows`] is a pure projection of the draft through
//! [`strata_core::keymap`]. What *is* modelled is the transient half the draft cannot hold:
//! which row is listening for a key, and which rebind is waiting for an answer.
//!
//! [`Editing`] is one value rather than a capture flag beside a conflict flag, because the two
//! states are exclusive and a pair of flags can be in a combination that means nothing — the
//! canvas's own view model carries `keymapCapture` and `keymapConflict` separately and then
//! clears one whenever it sets the other, which is this enum written out longhand.

use strata_core::config::{Command, Settings};
use strata_core::keymap::{
    chord_caps, describe, effective_chord, is_custom, is_fixed, Rebind, COMMANDS,
};

/// One row of the table: a command, what it says about itself, and the chord it holds.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyRow {
    pub command: Command,
    pub label: &'static str,
    pub desc: &'static str,
    /// The chord as display key caps (`["⇧", "⌘", "T"]`), empty when the command is unbound.
    pub caps: Vec<String>,
    /// Not rebindable (Esc). Shows its caps and nothing else — no capture, no reset.
    pub fixed: bool,
    /// The user has an override for this command **that takes effect**, whatever it resolved to.
    /// Drives the **Custom** badge and whether there is anything to reset — one field for both,
    /// so a row cannot wear the badge and refuse the control. Never true for a
    /// [`fixed`](Self::fixed) row: `keymap::is_custom` says so, because `effective_chord` ignores
    /// an override of a fixed command and hands back the default.
    pub custom: bool,
}

impl KeyRow {
    /// Whether the row has no shortcut at all, and so offers **Add shortcut** in place of caps.
    pub fn unbound(&self) -> bool {
        self.caps.is_empty()
    }
}

/// Every command as a row, in [`COMMANDS`] order — which is also the order a duplicate chord
/// resolves in, so the table reads top-down the way dispatch does.
pub fn rows(settings: &Settings) -> Vec<KeyRow> {
    COMMANDS
        .iter()
        .map(|meta| {
            let command = meta.command;
            let (label, desc) = describe(command);
            KeyRow {
                command,
                label,
                desc,
                caps: effective_chord(settings, command)
                    .map(|chord| chord_caps(&chord))
                    .unwrap_or_default(),
                fixed: is_fixed(command),
                custom: is_custom(settings, command),
            }
        })
        .collect()
}

/// Whether anything is overridden — what **Reset all** is offered for.
pub fn has_overrides(settings: &Settings) -> bool {
    !settings.keybinds.is_empty()
}

/// A rebind the pane cannot commit without an answer: the message, and the commands it would
/// take the chord from when there are any to take it from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Blocked {
    /// The row the note appears under.
    pub command: Command,
    /// What was asked for, held so pushing it through does not have to reconstruct it.
    pub rebind: Rebind,
    /// The commands that would be unbound to make room. **Empty** when there is nothing to offer
    /// — the chord itself was refused — and the note then shows the message alone. More than one
    /// only for a chord a hand-edited config already duplicated, and all of them have to go or
    /// the asker gets a chord `resolve` still hands to someone else.
    pub holders: Vec<Command>,
    pub message: String,
}

/// What the pane is doing beyond showing the bindings.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum Editing {
    #[default]
    Idle,
    /// Listening for the next key press, for this command.
    Capturing(Command),
    /// A rebind is waiting for an answer.
    Blocked(Blocked),
}

impl Editing {
    /// The command whose row is listening, if one is.
    pub fn capturing_command(&self) -> Option<Command> {
        match self {
            Self::Capturing(command) => Some(*command),
            _ => None,
        }
    }

    /// Whether `command`'s row is the one listening.
    pub fn capturing(&self, command: Command) -> bool {
        self.capturing_command() == Some(command)
    }

    /// The note under `command`'s row, if the blocked rebind is that row's.
    pub fn blocked(&self, command: Command) -> Option<&Blocked> {
        match self {
            Self::Blocked(blocked) if blocked.command == command => Some(blocked),
            _ => None,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use strata_core::config::{KeyBind, KeyChord};

    fn chord(key: &str) -> KeyChord {
        KeyChord {
            primary: true,
            shift: false,
            alt: false,
            key: key.to_string(),
        }
    }

    #[test]
    fn every_command_gets_a_row_in_table_order() {
        let rows = rows(&Settings::default());
        assert_eq!(rows.len(), COMMANDS.len());
        assert_eq!(
            rows.iter().map(|r| r.command).collect::<Vec<_>>(),
            COMMANDS.iter().map(|m| m.command).collect::<Vec<_>>()
        );
        assert!(rows.iter().all(|r| !r.custom && !r.unbound()));
        assert!(!has_overrides(&Settings::default()));
    }

    #[test]
    fn the_fixed_row_is_the_only_one() {
        let rows = rows(&Settings::default());
        let fixed: Vec<_> = rows.iter().filter(|r| r.fixed).map(|r| r.command).collect();
        assert_eq!(fixed, vec![Command::Cancel]);
        let esc = rows.iter().find(|r| r.fixed).expect("Dismiss is a row");
        assert_eq!(esc.caps, vec!["Esc"]);
    }

    #[test]
    fn a_fixed_row_never_reads_as_custom() {
        let settings = Settings {
            keybinds: vec![KeyBind {
                command: Command::Cancel,
                chord: Some(chord("d")),
            }],
            ..Settings::default()
        };
        let esc = rows(&settings)
            .into_iter()
            .find(|r| r.command == Command::Cancel)
            .expect("Dismiss is a row");
        assert!(esc.fixed && !esc.custom);
        assert_eq!(esc.caps, vec!["Esc"]);
        assert!(has_overrides(&settings));
    }

    #[test]
    fn an_override_reads_as_custom_and_an_unbind_as_addable() {
        let settings = Settings {
            keybinds: vec![
                KeyBind {
                    command: Command::Find,
                    chord: Some(chord("g")),
                },
                KeyBind {
                    command: Command::NewTab,
                    chord: None,
                },
            ],
            ..Settings::default()
        };
        let rows = rows(&settings);
        let row = |cmd| {
            rows.iter()
                .find(|r| r.command == cmd)
                .expect("every command has a row")
                .clone()
        };

        let find = row(Command::Find);
        assert!(find.custom && !find.unbound());
        assert_eq!(find.caps, vec!["⌘", "G"]);

        let new_tab = row(Command::NewTab);
        assert!(new_tab.custom && new_tab.unbound());

        let save = row(Command::SaveQuery);
        assert!(!save.custom && !save.unbound());
        assert!(has_overrides(&settings));
    }

    #[test]
    fn editing_answers_only_for_its_own_row() {
        let idle = Editing::Idle;
        assert_eq!(idle.capturing_command(), None);
        assert!(!idle.capturing(Command::Find));
        assert_eq!(idle.blocked(Command::Find), None);

        let capturing = Editing::Capturing(Command::Find);
        assert_eq!(capturing.capturing_command(), Some(Command::Find));
        assert!(capturing.capturing(Command::Find));
        assert!(!capturing.capturing(Command::NewTab));
        assert_eq!(capturing.blocked(Command::Find), None);

        let blocked = Editing::Blocked(Blocked {
            command: Command::Find,
            rebind: Rebind::To(chord("t")),
            holders: vec![Command::NewTab],
            message: "⌘T is already assigned to 'New query tab'".to_string(),
        });
        assert!(blocked.blocked(Command::Find).is_some());
        assert_eq!(blocked.blocked(Command::NewTab), None);
        assert!(!blocked.capturing(Command::Find));
    }
}
