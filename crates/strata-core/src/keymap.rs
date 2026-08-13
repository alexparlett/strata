//! The keymap: the command table and chord resolution, settings-driven end to end.
//!
//! One source of truth ([`COMMANDS`]) holds every command's label, description, default
//! chord, and whether it is fixed. Bindings resolve through [`effective_chord`] — the user
//! override from [`Settings::keybinds`] when present and valid, the built-in default
//! otherwise — so dispatch, menu hints, the Settings ▸ Keymap UI, and hand-edited config
//! JSON all agree. Dispatch itself is distributed (each feature listens for its own
//! command); this module only answers *which* command a chord means.
//!
//! **Editing a binding is [`propose`] then [`apply`], and nothing else.** The Keymap pane
//! (P4-08) has four ways to change a chord — capture a press, reset one row, take a chord off
//! another command, reset every row — and they are all the same two steps over a [`Rebind`]:
//! ask what the change would cost, then commit it. The policy and the sentence that explains a
//! refusal live here beside `validate_bind` rather than in the pane, because a hand-edited
//! config reaches the same rules through [`effective_chord`] and the two must not drift.

use crate::config::{Command, KeyBind, KeyChord, Settings};

/// Metadata for one command: display strings + the built-in default chord.
pub struct CommandMeta {
    pub command: Command,
    pub label: &'static str,
    pub desc: &'static str,
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
    command!(OpenProject, "Open project", "Pick a project folder and open it", [primary] "o"),
    command!(Quit, "Quit Strata", "Close every window and quit", [primary] "q"),
    command!(
        CloseProject,
        "Close project",
        "Close this window and return to the launcher",
        [primary shift] "w"
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

/// A change the user asked for on one command's binding.
///
/// Three variants because there are three things a keymap UI can ask for, and every path
/// through it is one of them: capture ([`To`](Rebind::To)), the per-row reset
/// ([`Default`](Rebind::Default)), and the unbind a reassignment performs on the command it
/// takes a chord *from* ([`Off`](Rebind::Off)).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Rebind {
    /// Take this chord.
    To(KeyChord),
    /// Go back to the built-in default — i.e. drop the override entirely.
    Default,
    /// Have no shortcut at all (an explicit unbind).
    Off,
}

impl Rebind {
    /// The chord this rebind would leave the command holding, `None` for an unbind.
    pub fn chord(&self, cmd: Command) -> Option<KeyChord> {
        match self {
            Self::To(chord) => Some(chord.clone()),
            Self::Default => Some(default_chord(cmd)),
            Self::Off => None,
        }
    }
}

/// What a [`Rebind`] would cost — the answer [`propose`] gives, and the only gate in front of
/// [`apply`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Bind {
    /// Nothing is in the way: commit it.
    Ready,
    /// Rebindable commands hold the chord. Committing means taking it off **all** of them, so
    /// this is the one outcome the UI can offer to push through (its Reassign) — with `message`
    /// as the question it asks. `holders` is in [`COMMANDS`] order and never empty; it holds more
    /// than one only for a chord a hand-edited config already duplicated.
    Clash {
        holders: Vec<Command>,
        message: String,
    },
    /// Refused outright, with why. Nothing to offer: either the chord can't be bound at all or
    /// a fixed command holds it, and a fixed command cannot give one up.
    Refused { message: String },
}

/// What `rebind` would do to `cmd`'s binding, given the settings as they stand.
///
/// The **whole** conflict policy for the UI, in one call: [`validate_bind`]'s chord rules, then
/// the duplicate search over every other command's effective chord. `cmd` itself is never a
/// holder, so re-pressing a command's current chord is [`Ready`](Bind::Ready) rather than a
/// clash with itself, and the per-row reset is checked by the same code as a capture — which it
/// has to be, since a default chord can have been taken by another command in the meantime.
pub fn propose(settings: &Settings, cmd: Command, rebind: &Rebind) -> Bind {
    let Some(chord) = rebind.chord(cmd) else {
        return match validate_bind(cmd, &default_chord(cmd)) {
            Ok(()) => Bind::Ready,
            Err(err) => Bind::Refused {
                message: err.to_string(),
            },
        };
    };
    if !matches!(rebind, Rebind::Default) {
        if let Err(err) = validate_bind(cmd, &chord) {
            return Bind::Refused {
                message: err.to_string(),
            };
        }
    }
    let holders = holders(settings, cmd, &chord);
    let Some(&first) = holders.first() else {
        return Bind::Ready;
    };
    let caps = chord_caps(&chord).concat();
    if let Some(&fixed) = holders.iter().find(|held| is_fixed(**held)) {
        return Bind::Refused {
            message: format!("{caps} is reserved for '{}'", describe(fixed).0),
        };
    }
    Bind::Clash {
        holders,
        message: format!("{caps} is already assigned to '{}'", describe(first).0),
    }
}

/// Every command other than `cmd` whose effective chord is `chord`, in [`COMMANDS`] order.
///
/// A `Vec` and not the first match, because a hand-edited config can put two commands on one
/// chord (`duplicate_chords_resolve_in_table_order` is that state) — and a reassignment that
/// freed only the first would hand the asker a chord `resolve` still gives to the second.
fn holders(settings: &Settings, cmd: Command, chord: &KeyChord) -> Vec<Command> {
    COMMANDS
        .iter()
        .map(|m| m.command)
        .filter(|other| *other != cmd)
        .filter(|other| effective_chord(settings, *other).as_ref() == Some(chord))
        .collect()
}

/// Commit `rebind` onto `cmd` — unconditionally. Call [`propose`] first; a
/// [`Clash`](Bind::Clash) the user pushed through is `apply(.., holder, &Rebind::Off)` for every
/// holder and then this, so taking a chord is expressed as the bindings it actually changes.
///
/// [`Rebind::Default`] **removes** the override rather than writing the default chord into it:
/// a row that is back to its default must stop reading as custom, and a stored copy of the
/// default would also freeze it against a later change to [`COMMANDS`].
///
/// A [`To`](Rebind::To) of the command's *own* default is that same removal, not a stored copy of
/// it. An override equal to the default is indistinguishable from no override as far as
/// [`effective_chord`] is concerned, so keeping one would mark a row Custom — and offer to reset
/// it — for a chord nobody changed. Pressing a command's existing shortcut is a no-op, which is
/// what it looks like.
pub fn apply(settings: &mut Settings, cmd: Command, rebind: &Rebind) {
    match rebind {
        Rebind::Default => clear_bind(settings, cmd),
        Rebind::To(chord) if *chord == default_chord(cmd) => clear_bind(settings, cmd),
        Rebind::To(chord) => set_bind(settings, cmd, Some(chord.clone())),
        Rebind::Off => set_bind(settings, cmd, None),
    }
}

/// Drop `cmd`'s override, whatever it was — back to the chord [`COMMANDS`] gives it.
fn clear_bind(settings: &mut Settings, cmd: Command) {
    settings.keybinds.retain(|bind| bind.command != cmd);
}

/// Write `cmd`'s override, replacing any it already had (`None` = an explicit unbind).
fn set_bind(settings: &mut Settings, cmd: Command, chord: Option<KeyChord>) {
    clear_bind(settings, cmd);
    settings.keybinds.push(KeyBind {
        command: cmd,
        chord,
    });
}

/// Drop every override — every command back to the chord [`COMMANDS`] gives it. The defaults
/// are distinct by construction (pinned by a test), so this can never leave a duplicate behind
/// and needs no [`propose`] in front of it.
pub fn reset_all(settings: &mut Settings) {
    settings.keybinds.clear();
}

/// Whether `cmd` carries an override **that takes effect** — what the Keymap row's **Custom**
/// badge and its reset control both answer to.
///
/// A command bound to no chord at all is still custom: the override is what the user did to it,
/// not what it ended up holding. A **fixed** command is never custom, however many entries a
/// hand-edited config gives it, because [`effective_chord`] ignores every one of them and hands
/// back the default — a badge saying otherwise would sit beside the built-in chord, on a row
/// with no reset control to clear it.
pub fn is_custom(settings: &Settings, cmd: Command) -> bool {
    !is_fixed(cmd) && settings.keybinds.iter().any(|bind| bind.command == cmd)
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
        for cmd in [
            Command::Find,
            Command::NewTab,
            Command::ReopenTab,
            Command::CloseActiveTab,
            Command::OpenProject,
            Command::CloseProject,
            Command::Quit,
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
        assert_eq!(COMMANDS.len(), 19);
    }

    #[test]
    fn defaults_resolve() {
        let s = Settings::default();
        assert_eq!(resolve(&s, &chord(true, false, "t")), Some(Command::NewTab));
        assert_eq!(
            resolve(&s, &chord(true, true, "t")),
            Some(Command::ReopenTab)
        );
        assert_eq!(
            resolve(&s, &chord(true, false, "Enter")),
            Some(Command::RunQuery)
        );
        assert_eq!(
            resolve(&s, &chord(false, false, "Escape")),
            Some(Command::Cancel)
        );
        assert_eq!(
            resolve(&s, &chord(true, false, "`")),
            Some(Command::CycleWindow)
        );
        assert_eq!(resolve(&s, &chord(true, false, "q")), Some(Command::Quit));
        assert_eq!(
            resolve(&s, &chord(true, true, "w")),
            Some(Command::CloseProject)
        );
        assert_eq!(
            resolve(&s, &chord(true, false, "w")),
            Some(Command::CloseActiveTab)
        );
        assert_eq!(resolve(&s, &chord(true, false, "z")), Some(Command::Undo));
        assert_eq!(resolve(&s, &chord(true, true, "z")), Some(Command::Redo));
        assert_eq!(resolve(&s, &chord(true, false, "x")), Some(Command::Cut));
        assert_eq!(resolve(&s, &chord(true, false, "c")), Some(Command::Copy));
        assert_eq!(resolve(&s, &chord(true, false, "v")), Some(Command::Paste));
        assert_eq!(
            resolve(&s, &chord(true, false, "a")),
            Some(Command::SelectAll)
        );
        assert_eq!(resolve(&s, &chord(true, false, "y")), None);
        assert_eq!(resolve(&s, &chord(false, false, "t")), None);
    }

    #[test]
    fn override_wins_and_frees_the_default() {
        let s = settings_with(vec![KeyBind {
            command: Command::RunQuery,
            chord: Some(chord(true, false, "r")),
        }]);
        assert_eq!(
            resolve(&s, &chord(true, false, "r")),
            Some(Command::RunQuery)
        );
        assert_eq!(resolve(&s, &chord(true, false, "Enter")), None);
    }

    #[test]
    fn edit_commands_are_rebindable() {
        let s = settings_with(vec![KeyBind {
            command: Command::Undo,
            chord: Some(chord(true, false, "y")),
        }]);
        assert_eq!(resolve(&s, &chord(true, false, "y")), Some(Command::Undo));
        assert_eq!(resolve(&s, &chord(true, false, "z")), None);
        assert_eq!(resolve(&s, &chord(true, true, "z")), Some(Command::Redo));

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
        let s = settings_with(vec![KeyBind {
            command: Command::Find,
            chord: Some(chord(false, false, "f")),
        }]);
        assert_eq!(
            effective_chord(&s, Command::Find),
            Some(default_chord(Command::Find))
        );
        let s = settings_with(vec![
            KeyBind {
                command: Command::Cancel,
                chord: Some(chord(true, false, "d")),
            },
            KeyBind {
                command: Command::Cancel,
                chord: None,
            },
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
        assert_eq!(
            validate_bind(Command::Find, &chord(true, false, "c")),
            Ok(())
        );
        assert_eq!(
            validate_bind(Command::Undo, &chord(true, false, "y")),
            Ok(())
        );
        assert_eq!(
            validate_bind(Command::Find, &chord(true, true, "g")),
            Ok(())
        );
    }

    #[test]
    fn duplicate_chords_resolve_in_table_order() {
        let s = settings_with(vec![KeyBind {
            command: Command::SaveQuery,
            chord: Some(chord(true, false, "t")),
        }]);
        assert_eq!(resolve(&s, &chord(true, false, "t")), Some(Command::NewTab));
    }

    #[test]
    fn every_default_chord_is_distinct() {
        let mut seen = Vec::new();
        for meta in COMMANDS {
            let chord = meta.default_chord();
            assert!(
                !seen.contains(&chord),
                "{:?} repeats a default chord",
                meta.command
            );
            seen.push(chord);
        }
    }

    #[test]
    fn propose_answers_the_four_outcomes() {
        let s = Settings::default();
        assert_eq!(
            propose(&s, Command::Find, &Rebind::To(chord(true, false, "g"))),
            Bind::Ready
        );
        assert_eq!(
            propose(&s, Command::Find, &Rebind::To(chord(true, false, "f"))),
            Bind::Ready
        );
        let Bind::Clash { holders, message } =
            propose(&s, Command::Find, &Rebind::To(chord(true, false, "t")))
        else {
            panic!("⌘T is New query tab's");
        };
        assert_eq!(holders, vec![Command::NewTab]);
        assert_eq!(message, "⌘T is already assigned to 'New query tab'");
        assert_eq!(
            propose(&s, Command::Find, &Rebind::To(chord(false, false, "g"))),
            Bind::Refused {
                message: BindError::MissingPrimary.to_string()
            }
        );
        assert_eq!(
            propose(&s, Command::Cancel, &Rebind::To(chord(true, false, "g"))),
            Bind::Refused {
                message: BindError::FixedCommand.to_string()
            }
        );
        assert_eq!(
            propose(&s, Command::Cancel, &Rebind::Off),
            Bind::Refused {
                message: BindError::FixedCommand.to_string()
            }
        );
        assert_eq!(propose(&s, Command::Cancel, &Rebind::Default), Bind::Ready);
        assert_eq!(propose(&s, Command::Find, &Rebind::Off), Bind::Ready);
    }

    #[test]
    fn a_chord_a_fixed_command_holds_is_reserved_not_offered() {
        let s = Settings::default();
        let Bind::Refused { message } = propose(
            &s,
            Command::Find,
            &Rebind::To(chord(false, false, "Escape")),
        ) else {
            panic!("Esc is Dismiss's, and Dismiss can't give it up");
        };
        assert_eq!(message, BindError::MissingPrimary.to_string());

        assert_eq!(
            holders(&s, Command::Find, &default_chord(Command::Cancel)),
            vec![Command::Cancel]
        );
    }

    #[test]
    fn apply_commits_the_three_rebinds() {
        let mut s = Settings::default();
        apply(&mut s, Command::Find, &Rebind::To(chord(true, false, "g")));
        assert_eq!(hint(&s, Command::Find), "⌘G");
        assert!(is_custom(&s, Command::Find));

        apply(&mut s, Command::Find, &Rebind::To(chord(true, true, "g")));
        assert_eq!(s.keybinds.len(), 1);
        assert_eq!(hint(&s, Command::Find), "⇧⌘G");

        apply(&mut s, Command::Find, &Rebind::Off);
        assert_eq!(effective_chord(&s, Command::Find), None);
        assert!(is_custom(&s, Command::Find));

        apply(&mut s, Command::Find, &Rebind::Default);
        assert!(s.keybinds.is_empty());
        assert!(!is_custom(&s, Command::Find));
        assert_eq!(hint(&s, Command::Find), "⌘F");
    }

    #[test]
    fn a_reassignment_is_an_unbind_and_a_bind() {
        let mut s = Settings::default();
        let want = chord(true, false, "t");
        let Bind::Clash { holders, .. } = propose(&s, Command::Find, &Rebind::To(want.clone()))
        else {
            panic!("⌘T is taken");
        };
        for holder in holders {
            apply(&mut s, holder, &Rebind::Off);
        }
        apply(&mut s, Command::Find, &Rebind::To(want.clone()));

        assert_eq!(resolve(&s, &want), Some(Command::Find));
        assert_eq!(effective_chord(&s, Command::NewTab), None);
        assert_eq!(resolve(&s, &chord(true, false, "f")), None);
    }

    #[test]
    fn a_reset_is_conflict_checked_like_a_capture() {
        let mut s = Settings::default();
        apply(
            &mut s,
            Command::SaveQuery,
            &Rebind::To(chord(true, false, "g")),
        );
        apply(&mut s, Command::Find, &Rebind::To(chord(true, false, "s")));

        let Bind::Clash { holders, .. } = propose(&s, Command::SaveQuery, &Rebind::Default) else {
            panic!("resetting Save query wants Find's ⌘S back");
        };
        assert_eq!(holders, vec![Command::Find]);
    }

    #[test]
    fn a_fixed_command_is_never_custom() {
        let s = settings_with(vec![KeyBind {
            command: Command::Cancel,
            chord: Some(chord(true, false, "d")),
        }]);
        assert_eq!(hint(&s, Command::Cancel), "Esc");
        assert!(!is_custom(&s, Command::Cancel));
        let s = settings_with(vec![KeyBind {
            command: Command::Find,
            chord: Some(chord(true, false, "d")),
        }]);
        assert!(is_custom(&s, Command::Find));
    }

    #[test]
    fn binding_a_command_to_its_own_default_is_not_an_override() {
        let mut s = Settings::default();
        apply(
            &mut s,
            Command::Find,
            &Rebind::To(default_chord(Command::Find)),
        );
        assert!(s.keybinds.is_empty());
        assert!(!is_custom(&s, Command::Find));
        assert_eq!(hint(&s, Command::Find), "⌘F");

        apply(&mut s, Command::Find, &Rebind::To(chord(true, false, "g")));
        assert!(is_custom(&s, Command::Find));
        apply(
            &mut s,
            Command::Find,
            &Rebind::To(default_chord(Command::Find)),
        );
        assert!(!is_custom(&s, Command::Find));
    }

    #[test]
    fn a_clash_names_every_holder_so_reassign_frees_them_all() {
        let mut s = settings_with(vec![KeyBind {
            command: Command::CloseActiveTab,
            chord: Some(chord(true, false, "t")),
        }]);
        let want = chord(true, false, "t");
        assert_eq!(
            effective_chord(&s, Command::NewTab).as_ref(),
            Some(&want),
            "NewTab still holds its default"
        );

        let Bind::Clash { holders, .. } = propose(&s, Command::Find, &Rebind::To(want.clone()))
        else {
            panic!("⌘T is doubly taken");
        };
        assert_eq!(holders, vec![Command::NewTab, Command::CloseActiveTab]);

        for holder in holders {
            apply(&mut s, holder, &Rebind::Off);
        }
        apply(&mut s, Command::Find, &Rebind::To(want.clone()));
        assert_eq!(resolve(&s, &want), Some(Command::Find));
    }

    #[test]
    fn reset_all_clears_every_override() {
        let mut s = Settings::default();
        apply(&mut s, Command::Find, &Rebind::To(chord(true, false, "g")));
        apply(&mut s, Command::NewTab, &Rebind::Off);
        reset_all(&mut s);
        assert!(s.keybinds.is_empty());
        assert_eq!(hint(&s, Command::Find), "⌘F");
        assert_eq!(hint(&s, Command::NewTab), "⌘T");
    }

    #[test]
    fn caps_and_hints() {
        let s = Settings::default();
        assert_eq!(
            chord_caps(&default_chord(Command::ReopenTab)),
            ["⇧", "⌘", "T"]
        );
        assert_eq!(hint(&s, Command::RunQuery), "⌘↵");
        assert_eq!(hint(&s, Command::Cancel), "Esc");
        assert_eq!(hint(&s, Command::CycleWindow), "⌘`");
        assert_eq!(hint(&s, Command::OpenSettings), "⌘,");
        assert_eq!(hint(&s, Command::Undo), "⌘Z");
        assert_eq!(hint(&s, Command::Redo), "⇧⌘Z");
    }
}
