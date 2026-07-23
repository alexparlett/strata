//! Freya-side keymap glue: the event→chord fold, the distributed-dispatch handler
//! builder, and reactive shortcut hints.
//!
//! Dispatch is distributed (no registry): each feature attaches
//! `.on_global_key_down(keymap::on_command(settings, Command::X, action))` to its own
//! rect. Same-name global listeners fire in document (pre-order) order and a handled
//! command **consumes** the press via `prevent_default` — both semantics guaranteed by
//! our Freya fork — so precedence is simply *where a listener sits in the tree*. Beware
//! the pre-order pitfall: an ancestor's listener fires before its descendants', so a
//! lower-precedence Esc consumer must live on a node that comes *after* the
//! higher-precedence one in document order, not on a shared ancestor.

use freya::text_edit::{ChordKey, EditBindings, EditChord};
use freya::prelude::*;
use strata_core::config::{Command, KeyChord, Settings};

use crate::components::typography::Meta;

/// Fold a key event into a normalized [`KeyChord`]: `primary` = ⌘ *or* Ctrl (every ⌘
/// shortcut also responds to Ctrl), characters lowercased (⇧⌘T arrives as `"T"` but is
/// stored as `"t"`), named keys by name (`"Enter"`, `"Escape"`, `"ArrowUp"`).
/// `None` for modifier-only presses — a chord needs an actual key.
pub fn chord_from_event(e: &KeyboardEventData) -> Option<KeyChord> {
    let key = match &e.key {
        Key::Character(c) => c.to_lowercase(),
        Key::Named(named) => match named {
            // `Super` / `Hyper` are spec-deprecated aliases of Meta, but a platform may
            // still deliver them — they must fold to "modifier only", not to a key.
            #[allow(deprecated)]
            NamedKey::Shift
            | NamedKey::Control
            | NamedKey::Alt
            | NamedKey::AltGraph
            | NamedKey::Meta
            | NamedKey::Super
            | NamedKey::Hyper
            | NamedKey::Fn
            | NamedKey::FnLock
            | NamedKey::CapsLock
            | NamedKey::NumLock
            | NamedKey::ScrollLock
            | NamedKey::Symbol
            | NamedKey::SymbolLock => return None,
            named => format!("{named:?}"),
        },
    };
    Some(KeyChord {
        primary: e.modifiers.intersects(Modifiers::META | Modifiers::CONTROL),
        shift: e.modifiers.contains(Modifiers::SHIFT),
        alt: e.modifiers.contains(Modifiers::ALT),
        key,
    })
}

/// Build an `on_global_key_down` handler for one command: fold the event, resolve it
/// against the live settings (`peek` — rebinds apply instantly, no re-render), and when
/// it names `cmd` and `action` handles it, consume the press so listeners later in
/// document order never see it. `action` returns `false` to decline — "not applicable
/// right now" (e.g. Esc while not renaming) — leaving the press for the next listener.
pub fn on_command(
    settings: State<Settings>,
    cmd: Command,
    mut action: impl FnMut() -> bool + 'static,
) -> impl FnMut(Event<KeyboardEventData>) {
    move |e: Event<KeyboardEventData>| {
        let Some(chord) = chord_from_event(&e) else {
            return;
        };
        if strata_core::keymap::resolve(&settings.peek(), &chord) == Some(cmd) && action() {
            e.prevent_default();
        }
    }
}

/// Multi-command variant of [`on_command`] for a node that owns several shortcuts — an
/// element holds **one** handler per event name, so a second `.on_global_key_down`
/// would replace the first. Folds and resolves once, then hands the command to
/// `dispatch`; returning `true` consumes the press.
pub fn on_commands(
    settings: State<Settings>,
    mut dispatch: impl FnMut(Command) -> bool + 'static,
) -> impl FnMut(Event<KeyboardEventData>) {
    move |e: Event<KeyboardEventData>| {
        let Some(chord) = chord_from_event(&e) else {
            return;
        };
        let Some(cmd) = strata_core::keymap::resolve(&settings.peek(), &chord) else {
            return;
        };
        if dispatch(cmd) {
            e.prevent_default();
        }
    }
}

/// The text-editing bindings derived from the settings: the effective chords of every
/// editing command ([`Command::is_edit`]) converted for the freya-edit layer, which
/// matches `EditBindings` in `process_key` instead of its old hardcoded
/// ⌘A/⌘C/⌘X/⌘V/⌘Z/⌘Y. An unbound command yields no chord — that action's shortcut is
/// disabled. Keep every mounted editor's data in sync with this (see the editor tab's
/// side effect).
pub fn edit_bindings(settings: &Settings) -> EditBindings {
    let chords = |cmd| {
        strata_core::keymap::effective_chord(settings, cmd)
            .and_then(|chord| edit_chord(&chord))
            .into_iter()
            .collect()
    };
    EditBindings {
        select_all: chords(Command::SelectAll),
        copy: chords(Command::Copy),
        cut: chords(Command::Cut),
        paste: chords(Command::Paste),
        undo: chords(Command::Undo),
        redo: chords(Command::Redo),
    }
}

/// A settings [`KeyChord`] as a freya-edit [`EditChord`]. `None` when the key name has
/// no keyboard-types equivalent (the chord then simply doesn't drive the editor —
/// better a dead shortcut than a mis-matched one).
fn edit_chord(chord: &KeyChord) -> Option<EditChord> {
    let mut chars = chord.key.chars();
    let key = match (chars.next(), chars.next()) {
        (Some(c), None) => ChordKey::Character(c),
        // Named keys are stored by name ("Enter", "ArrowUp" — see `chord_from_event`),
        // which is exactly keyboard-types' `NamedKey` `FromStr` vocabulary.
        _ => match chord.key.parse::<NamedKey>() {
            Ok(named) => ChordKey::Named(named),
            Err(_) => {
                tracing::warn!("no editor-chord equivalent for key {:?}", chord.key);
                return None;
            }
        },
    };
    Some(EditChord {
        primary: chord.primary,
        shift: chord.shift,
        alt: chord.alt,
        key,
    })
}

/// The effective hint string for `cmd` (`"⇧⌘T"`, `""` when unbound), reactively: the
/// `.read()` subscribes this component to the settings global, so a rebind repaints
/// every hint in every window.
pub fn use_hint(cmd: Command) -> String {
    let settings = use_consume::<State<Settings>>();
    strata_core::keymap::hint(&settings.read(), cmd)
}

/// A tooltip title with the command's effective chord appended — `"Save query (⌘S)"`,
/// or just the label when the command is unbound. Reactive like [`use_hint`], so a
/// rebind repaints every tooltip.
pub fn use_hint_title(label: &str, cmd: Command) -> String {
    let hint = use_hint(cmd);
    if hint.is_empty() {
        label.to_string()
    } else {
        format!("{label} ({hint})")
    }
}

/// A muted key-cap caption (`⇧⌘T`) for menu rows and labels. Renders nothing when the
/// command is unbound. A component rather than a helper so menus built from event
/// handlers (no hook scope) still get the hooks at render time, under the window root's
/// contexts.
#[derive(PartialEq)]
pub struct KeyHint(pub Command);

impl Component for KeyHint {
    fn render(&self) -> impl IntoElement {
        let hint = use_hint(self.0);
        let color = use_theme().read().colors.text_secondary;
        rect().maybe(!hint.is_empty(), |el| el.child(Meta::new(hint).color(color)))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use strata_core::config::Command;

    fn event(key: Key, modifiers: Modifiers) -> KeyboardEventData {
        KeyboardEventData::new(key, Code::Unidentified, modifiers)
    }

    #[test]
    fn folds_characters_lowercased_and_primary_from_meta_or_ctrl() {
        // ⇧⌘T arrives as the character "T".
        let chord = chord_from_event(&event(
            Key::Character("T".into()),
            Modifiers::META | Modifiers::SHIFT,
        ))
        .unwrap();
        assert!(chord.primary && chord.shift && !chord.alt);
        assert_eq!(chord.key, "t");

        // Ctrl folds into primary too.
        let chord =
            chord_from_event(&event(Key::Character("t".into()), Modifiers::CONTROL)).unwrap();
        assert!(chord.primary);
    }

    #[test]
    fn folds_named_keys_by_name() {
        let chord = chord_from_event(&event(Key::Named(NamedKey::Enter), Modifiers::META)).unwrap();
        assert_eq!(chord.key, "Enter");
        let chord = chord_from_event(&event(Key::Named(NamedKey::Escape), Modifiers::empty()))
            .unwrap();
        assert_eq!(chord.key, "Escape");
        assert!(!chord.primary && !chord.shift && !chord.alt);
    }

    #[test]
    fn modifier_only_presses_fold_to_none() {
        for named in [NamedKey::Shift, NamedKey::Meta, NamedKey::Control, NamedKey::Alt] {
            assert!(chord_from_event(&event(Key::Named(named), Modifiers::META)).is_none());
        }
    }

    #[test]
    fn edit_bindings_follow_settings() {
        use strata_core::config::{KeyBind, KeyChord};

        // Defaults: the full editing set, one chord each.
        let bindings = edit_bindings(&Settings::default());
        assert_eq!(bindings.select_all, vec![EditChord::primary('a')]);
        assert_eq!(bindings.copy, vec![EditChord::primary('c')]);
        assert_eq!(bindings.cut, vec![EditChord::primary('x')]);
        assert_eq!(bindings.paste, vec![EditChord::primary('v')]);
        assert_eq!(bindings.undo, vec![EditChord::primary('z')]);
        assert_eq!(bindings.redo, vec![EditChord::primary_shift('z')]);

        // Rebind redo to ⌘Y, unbind undo entirely: the editor follows.
        let settings = Settings {
            keybinds: vec![
                KeyBind {
                    command: Command::Redo,
                    chord: Some(KeyChord {
                        primary: true,
                        shift: false,
                        alt: false,
                        key: "y".to_string(),
                    }),
                },
                KeyBind { command: Command::Undo, chord: None },
            ],
            ..Settings::default()
        };
        let bindings = edit_bindings(&settings);
        assert!(bindings.undo.is_empty());
        assert_eq!(bindings.redo, vec![EditChord::primary('y')]);

        // Named keys convert through keyboard-types' vocabulary.
        let settings = Settings {
            keybinds: vec![KeyBind {
                command: Command::Undo,
                chord: Some(KeyChord {
                    primary: true,
                    shift: false,
                    alt: false,
                    key: "Enter".to_string(),
                }),
            }],
            ..Settings::default()
        };
        let bindings = edit_bindings(&settings);
        assert_eq!(bindings.undo[0].key, ChordKey::Named(NamedKey::Enter));
    }

    #[test]
    fn folded_defaults_resolve() {
        let settings = Settings::default();
        let chord = chord_from_event(&event(
            Key::Character("T".into()),
            Modifiers::META | Modifiers::SHIFT,
        ))
        .unwrap();
        assert_eq!(
            strata_core::keymap::resolve(&settings, &chord),
            Some(Command::ReopenTab)
        );
    }
}
