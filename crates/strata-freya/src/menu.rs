//! The application menubar (macOS): a minimal App menu — About · Hide/Show · Quit —
//! built with muda through the fork's `menu` feature.
//!
//! **Quit routes through the close veto**, never Cocoa's `terminate:`: it's a *custom*
//! item (muda's `PredefinedMenuItem::quit()` would send `terminate:`, the very thing
//! that bypassed the T2 confirm), whose event the handler turns into
//! [`RendererContext::request_close_window`] — red-button semantics, so the
//! close-while-running confirm keeps its say. Its accelerator derives from the keymap
//! (`effective_chord(CloseProject)`), keeping the keymap the single source of truth; the
//! OS handles the accelerator before the window sees the key, so the keymap's own ⌘Q
//! listener simply never fires while the menu carries it — same command either way.
//! (Accelerators are read at launch; a rebind updates the menu on next start — live
//! menu updates can ride P4-08.)
//!
//! Deliberately **no Edit menu**: predefined Cut/Copy/Paste items would claim ⌘C/⌘V/⌘X
//! as menu accelerators and starve the editor, which handles those keys directly —
//! the exact swallowing tangle the Dioxus app fought (DEV_TASKS F8).

use freya::menu::accelerator::Accelerator;
use freya::menu::{AboutMetadata, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use strata_core::config::{Command, KeyChord};

/// A custom menubar item — the typed vocabulary the builder and the event handler
/// share, so dispatch is an exhaustive `match`, not string comparison (the Dioxus
/// menu's `MenuCmd` pattern). Grows a variant per item as the menu fills out (P6-02).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuCmd {
    /// Quit Strata — routed through the close veto, never Cocoa `terminate:`.
    Quit,
}

impl MenuCmd {
    /// The stable string id muda carries (namespaced clear of predefined items).
    fn id(self) -> &'static str {
        match self {
            Self::Quit => "strata.quit",
        }
    }

    /// The command a [`MenuEvent`]'s id names, if it is one of ours (tray menus and
    /// predefined items share muda's event stream — foreign ids are simply not ours).
    fn parse(id: &MenuId) -> Option<Self> {
        [Self::Quit].into_iter().find(|cmd| id.0 == cmd.id())
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

/// Build the menubar. `quit_chord` is the effective CloseProject chord, resolved by the
/// caller before launch (the builder runs on the event loop thread and must be `Send`,
/// so it captures plain data — not the settings handle).
pub fn app_menu(quit_chord: Option<KeyChord>) -> Menu {
    let quit = MenuItem::with_id(
        MenuCmd::Quit,
        "Quit Strata",
        true,
        quit_chord.as_ref().and_then(accelerator),
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
    let menu = Menu::new();
    if let Err(err) = menu.append(&app) {
        tracing::error!("menubar: appending App submenu failed: {err}");
    }
    menu
}

/// The launch menu handler: exhaustive dispatch over [`MenuCmd`]. Quit routes through
/// the close veto (red-button semantics — the T2 confirm decides while a query runs).
pub fn handle_menu_event(event: MenuEvent, mut ctx: freya::prelude::RendererContext) {
    match MenuCmd::parse(event.id()) {
        Some(MenuCmd::Quit) => ctx.request_close_window(None),
        None => {}
    }
}

/// The effective quit chord for the menubar, resolved from settings at launch.
pub fn quit_chord(settings: &strata_core::config::Settings) -> Option<KeyChord> {
    strata_core::keymap::effective_chord(settings, Command::CloseProject)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn menu_cmd_ids_round_trip() {
        for cmd in [MenuCmd::Quit] {
            assert_eq!(MenuCmd::parse(&MenuId::from(cmd)), Some(cmd));
        }
        assert_eq!(MenuCmd::parse(&MenuId::new("not.ours")), None);
    }

    #[test]
    fn default_quit_chord_maps_to_an_accelerator() {
        let chord = quit_chord(&strata_core::config::Settings::default()).unwrap();
        assert!(accelerator(&chord).is_some());
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
