//! The application menubar (macOS): the App menu — About · Hide/Show · Quit — and a
//! standard Edit menu — Undo · Redo · Cut · Copy · Paste · Select All — built with muda
//! through the fork's `menu` feature.
//!
//! **Quit routes through the close veto**, never Cocoa's `terminate:`: it's a *custom*
//! item (muda's `PredefinedMenuItem::quit()` would send `terminate:`, the very thing
//! that bypassed the T2 confirm), whose event the handler turns into
//! [`NativeEventExt::request_close_window`] — red-button semantics, so the
//! close-while-running confirm keeps its say.
//!
//! **The Edit menu is custom items too**, not muda's predefined set: the predefined
//! items send Cocoa first-responder selectors (`undo:` / `copy:` / …) that a Skia view
//! never receives — the exact swallowing tangle the Dioxus app fought (DEV_TASKS F8).
//! Instead each item's event **synthesizes the command's effective chord into the
//! focused window's keyboard pipeline** ([`NativeEventExt::send_key_press`]), so menu
//! clicks and accelerator presses flow through the exact same path as typed keys — the
//! focused element (SQL editor, find input, …) and its `EditBindings` decide.
//! First-responder semantics, without Cocoa.
//!
//! Accelerators derive from the keymap (`effective_chord`), keeping it the single
//! source of truth; the OS handles an accelerator before the window sees the key, so
//! the corresponding in-window listener simply never fires while the menu carries it —
//! same command either way. Accelerators are read at launch (a rebind updates the menu
//! on next start — live menu updates can ride P4-08), but *dispatch* resolves the live
//! settings, so a rebound chord acts correctly even before restart.
//!
//! **Deliberately not ported from the Dioxus app**: its `global-hotkey` OS-hotkey layer
//! (`strata-dioxus` `use_shortcuts`) and its `PredefinedMenuItem` Edit set. Both were
//! webview workarounds — OS hotkeys fired before wry swallowed the keys, and predefined
//! items worked only because WKWebView answers Cocoa's first-responder selectors. With
//! native winit delivery every key reaches the keymap's listeners directly (resolved
//! live, per focused window), so the hotkey manager, its focus-gated registration, and
//! its chord→`Code` table have no job here.

use freya::menu::accelerator::Accelerator;
use freya::menu::{AboutMetadata, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use freya::prelude::{Code, Key, Modifiers, ModifiersExt, NamedKey, NativeEventExt, State};
use strata_core::config::{Command, KeyChord, Settings};

/// A custom menubar item — the typed vocabulary the builder and the event handler
/// share, so dispatch is an exhaustive `match`, not string comparison (the Dioxus
/// menu's `MenuCmd` pattern). Grows a variant per item as the menu fills out (P6-02).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuCmd {
    /// Quit Strata — routed through the close veto, never Cocoa `terminate:`.
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl MenuCmd {
    const ALL: [Self; 7] = [
        Self::Quit,
        Self::Undo,
        Self::Redo,
        Self::Cut,
        Self::Copy,
        Self::Paste,
        Self::SelectAll,
    ];

    /// The stable string id muda carries (namespaced clear of predefined items).
    fn id(self) -> &'static str {
        match self {
            Self::Quit => "strata.quit",
            Self::Undo => "strata.edit.undo",
            Self::Redo => "strata.edit.redo",
            Self::Cut => "strata.edit.cut",
            Self::Copy => "strata.edit.copy",
            Self::Paste => "strata.edit.paste",
            Self::SelectAll => "strata.edit.select-all",
        }
    }

    /// The command a [`MenuEvent`]'s id names, if it is one of ours (tray menus and
    /// predefined items share muda's event stream — foreign ids are simply not ours).
    fn parse(id: &MenuId) -> Option<Self> {
        Self::ALL.into_iter().find(|cmd| id.0 == cmd.id())
    }

    /// The keymap command an Edit item dispatches through the focused window's
    /// keyboard pipeline. `None` for Quit, which routes through the close veto instead.
    fn edit_command(self) -> Option<Command> {
        match self {
            Self::Quit => None,
            Self::Undo => Some(Command::Undo),
            Self::Redo => Some(Command::Redo),
            Self::Cut => Some(Command::Cut),
            Self::Copy => Some(Command::Copy),
            Self::Paste => Some(Command::Paste),
            Self::SelectAll => Some(Command::SelectAll),
        }
    }
}

impl From<MenuCmd> for MenuId {
    fn from(cmd: MenuCmd) -> Self {
        MenuId::new(cmd.id())
    }
}

/// A [`KeyChord`] as a muda accelerator (`CmdOrCtrl+Q`). `None` when the chord has no
/// muda-parsable key — the item then simply ships without an accelerator.
fn accelerator(chord: &KeyChord) -> Option<Accelerator> {
    let mut spec = String::new();
    if chord.primary {
        spec.push_str("CmdOrCtrl+");
    }
    if chord.shift {
        spec.push_str("Shift+");
    }
    if chord.alt {
        spec.push_str("Alt+");
    }
    spec.push_str(&chord.key);
    spec.parse().ok()
}

/// A [`KeyChord`] as a synthesizable key event: the chord's key plus its modifier
/// flags (`primary` folds to the platform primary modifier). `None` when the key name
/// is neither a single character nor a keyboard-types named key.
fn synthetic_key(chord: &KeyChord) -> Option<(Key, Modifiers)> {
    let mut chars = chord.key.chars();
    let key = match (chars.next(), chars.next()) {
        (Some(_), None) => Key::Character(chord.key.clone().into()),
        _ => Key::Named(chord.key.parse::<NamedKey>().ok()?),
    };
    let mut modifiers = Modifiers::empty();
    if chord.primary {
        modifiers |= Modifiers::ctrl_or_meta();
    }
    if chord.shift {
        modifiers |= Modifiers::SHIFT;
    }
    if chord.alt {
        modifiers |= Modifiers::ALT;
    }
    Some((key, modifiers))
}

/// The launch-time accelerator chords, resolved from settings before the menu builder
/// runs (the builder runs on the event loop thread as a `Send` closure, so it captures
/// plain data — not the settings handle).
#[derive(Clone)]
pub struct MenuChords {
    pub quit: Option<KeyChord>,
    pub undo: Option<KeyChord>,
    pub redo: Option<KeyChord>,
    pub cut: Option<KeyChord>,
    pub copy: Option<KeyChord>,
    pub paste: Option<KeyChord>,
    pub select_all: Option<KeyChord>,
}

/// The effective menubar chords, resolved from settings at launch.
pub fn menu_chords(settings: &Settings) -> MenuChords {
    let chord = |cmd| strata_core::keymap::effective_chord(settings, cmd);
    MenuChords {
        quit: chord(Command::CloseProject),
        undo: chord(Command::Undo),
        redo: chord(Command::Redo),
        cut: chord(Command::Cut),
        copy: chord(Command::Copy),
        paste: chord(Command::Paste),
        select_all: chord(Command::SelectAll),
    }
}

/// Build the menubar: the App menu, then the standard Edit menu.
pub fn app_menu(chords: MenuChords) -> Menu {
    let quit = MenuItem::with_id(
        MenuCmd::Quit,
        "Quit Strata",
        true,
        chords.quit.as_ref().and_then(accelerator),
    );
    let app = Submenu::new("Strata", true);
    let items: &[&dyn freya::menu::IsMenuItem] = &[
        &PredefinedMenuItem::about(
            Some("About Strata"),
            Some(AboutMetadata {
                name: Some("Strata".to_string()),
                comments: Some("A local Athena-style parquet query workspace".to_string()),
                ..Default::default()
            }),
        ),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::hide(Some("Hide Strata")),
        &PredefinedMenuItem::hide_others(None),
        &PredefinedMenuItem::show_all(None),
        &PredefinedMenuItem::separator(),
        &quit,
    ];
    if let Err(err) = app.append_items(items) {
        tracing::error!("menubar: appending App menu items failed: {err}");
    }

    // An unbound command has no chord to dispatch through the keyboard pipeline, so
    // its item ships disabled — the shortcut and the menu stay one mechanism.
    let edit_item = |cmd: MenuCmd, label: &str, chord: &Option<KeyChord>| {
        MenuItem::with_id(cmd, label, chord.is_some(), chord.as_ref().and_then(accelerator))
    };
    let edit = Submenu::new("Edit", true);
    let items: &[&dyn freya::menu::IsMenuItem] = &[
        &edit_item(MenuCmd::Undo, "Undo", &chords.undo),
        &edit_item(MenuCmd::Redo, "Redo", &chords.redo),
        &PredefinedMenuItem::separator(),
        &edit_item(MenuCmd::Cut, "Cut", &chords.cut),
        &edit_item(MenuCmd::Copy, "Copy", &chords.copy),
        &edit_item(MenuCmd::Paste, "Paste", &chords.paste),
        &PredefinedMenuItem::separator(),
        &edit_item(MenuCmd::SelectAll, "Select All", &chords.select_all),
    ];
    if let Err(err) = edit.append_items(items) {
        tracing::error!("menubar: appending Edit menu items failed: {err}");
    }

    let menu = Menu::new();
    for submenu in [&app, &edit] {
        if let Err(err) = menu.append(submenu) {
            tracing::error!("menubar: appending submenu failed: {err}");
        }
    }
    menu
}

/// The launch menu handler: exhaustive dispatch over [`MenuCmd`]. Quit routes through
/// the close veto (red-button semantics — the T2 confirm decides while a query runs);
/// Edit items synthesize their command's *live* effective chord into the focused
/// window's keyboard pipeline, so the focused element and its bindings decide — the
/// same path as typed keys.
pub fn handle_menu_event(
    event: MenuEvent,
    mut ctx: freya::prelude::RendererContext,
    settings: State<Settings>,
) {
    match MenuCmd::parse(event.id()) {
        Some(MenuCmd::Quit) => ctx.request_close_window(None),
        Some(cmd) => {
            let Some(command) = cmd.edit_command() else {
                return;
            };
            let Some(chord) =
                strata_core::keymap::effective_chord(&settings.peek(), command)
            else {
                return;
            };
            let Some((key, modifiers)) = synthetic_key(&chord) else {
                return;
            };
            ctx.send_key_press(None, key, Code::Unidentified, modifiers);
        }
        None => {}
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn menu_cmd_ids_round_trip() {
        for cmd in MenuCmd::ALL {
            assert_eq!(MenuCmd::parse(&MenuId::from(cmd)), Some(cmd));
        }
        assert_eq!(MenuCmd::parse(&MenuId::new("not.ours")), None);
    }

    #[test]
    fn every_edit_item_has_a_command_and_quit_does_not() {
        assert_eq!(MenuCmd::Quit.edit_command(), None);
        for cmd in MenuCmd::ALL.into_iter().filter(|cmd| *cmd != MenuCmd::Quit) {
            assert!(cmd.edit_command().unwrap().is_edit(), "{cmd:?}");
        }
    }

    #[test]
    fn default_chords_map_to_accelerators() {
        let chords = menu_chords(&Settings::default());
        for (name, chord) in [
            ("quit", &chords.quit),
            ("undo", &chords.undo),
            ("redo", &chords.redo),
            ("cut", &chords.cut),
            ("copy", &chords.copy),
            ("paste", &chords.paste),
            ("select_all", &chords.select_all),
        ] {
            let chord = chord.as_ref().unwrap_or_else(|| panic!("{name} unbound"));
            assert!(accelerator(chord).is_some(), "{name}");
            assert!(synthetic_key(chord).is_some(), "{name}");
        }
    }

    #[test]
    fn synthetic_keys_mirror_the_chord() {
        // ⇧⌘Z: character key, primary + shift folded into modifier flags.
        let (key, modifiers) = synthetic_key(&KeyChord {
            primary: true,
            shift: true,
            alt: false,
            key: "z".to_string(),
        })
        .unwrap();
        assert_eq!(key, Key::Character("z".into()));
        assert!(modifiers.contains(Modifiers::ctrl_or_meta()));
        assert!(modifiers.contains(Modifiers::SHIFT));
        assert!(!modifiers.contains(Modifiers::ALT));

        // Named keys go through keyboard-types' vocabulary.
        let (key, _) = synthetic_key(&KeyChord {
            primary: true,
            shift: false,
            alt: false,
            key: "Enter".to_string(),
        })
        .unwrap();
        assert_eq!(key, Key::Named(NamedKey::Enter));

        // An unmappable name degrades to "no dispatch", not a panic.
        assert!(
            synthetic_key(&KeyChord {
                primary: true,
                shift: false,
                alt: false,
                key: "NoSuchKey".to_string(),
            })
            .is_none()
        );
    }

    #[test]
    fn named_and_symbol_keys_map() {
        for key in ["Enter", ",", "`", "t"] {
            let chord = KeyChord {
                primary: true,
                shift: false,
                alt: false,
                key: key.to_string(),
            };
            assert!(accelerator(&chord).is_some(), "{key}");
        }
        // An unbindable oddball degrades to "no accelerator", not a panic.
        let chord = KeyChord {
            primary: true,
            shift: false,
            alt: false,
            key: "NoSuchKey".to_string(),
        };
        assert!(accelerator(&chord).is_none());
    }
}
