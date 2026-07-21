//! The tab's right-click context menu: one [`Menu`] of tab actions, opened at the cursor via
//! [`ContextMenu::open_from_event`] by the right-clicked tab and scoped to *that* tab.
//!
//! A faithful port of the Dioxus `tab_menu_items`: Rename · Duplicate — Close · Close others · Close
//! to the right · Close all — Reopen closed tab. `MenuButton` / `MenuContainer` are theme-driven (the
//! same components the tab-controls dropdowns use), so only the separators take an explicit colour.

use freya::prelude::*;
use freya::radio::Radio;

use crate::apps::project::state::{Chan, SessionState, TabId};
use crate::components::divider::Divider;
use crate::components::typography::Prose;

/// Build the tab context menu for tab `id`. `sep` is the separator colour, passed in because this runs
/// from an event handler — no hooks, so it can't read the theme itself. `renaming` is the tab's own
/// rename flag: "Rename" just flips it on and the tab reacts (seeds the draft, focuses the input, and
/// commits). Each action runs then closes the menu — a menu-item press lands *inside* the menu, so it
/// won't dismiss on its own (only an outside press does).
pub fn tab_context_menu(
    id: TabId,
    mut radio: Radio<SessionState, Chan>,
    sep: Color,
    mut renaming: State<bool>,
) -> Menu {
    Menu::new()
        // Rename → just flip the tab into rename mode. The tab reacts (seeds the draft + focuses the
        // input) in its own scope, so it survives this menu closing.
        .child(
            MenuButton::new()
                .on_press(move |_| {
                    renaming.set(true);
                    ContextMenu::close();
                })
                .child(Prose::new("Rename")),
        )
        .child(
            MenuButton::new()
                .on_press(move |_| {
                    radio.write().duplicate(id);
                    ContextMenu::close();
                })
                .child(Prose::new("Duplicate")),
        )
        .child(menu_sep(sep))
        .child(
            MenuButton::new()
                .on_press(move |_| {
                    radio.write().close_one(id);
                    ContextMenu::close();
                })
                .child(Prose::new("Close")),
        )
        .child(
            MenuButton::new()
                .on_press(move |_| {
                    radio.write().close_others(id);
                    ContextMenu::close();
                })
                .child(Prose::new("Close others")),
        )
        .child(
            MenuButton::new()
                .on_press(move |_| {
                    radio.write().close_right(id);
                    ContextMenu::close();
                })
                .child(Prose::new("Close to the right")),
        )
        .child(
            MenuButton::new()
                .on_press(move |_| {
                    radio.write().close_all();
                    ContextMenu::close();
                })
                .child(Prose::new("Close all")),
        )
        .child(menu_sep(sep))
        .child(
            MenuButton::new()
                .on_press(move |_| {
                    radio.write().reopen_last();
                    ContextMenu::close();
                })
                .child(Prose::new("Reopen closed tab")),
        )
}

/// A menu divider: the shared [`Divider`] configured for a hug-content menu — `fill_minimum` (a plain
/// `fill` would blow the menu out to the window width, since the container hugs its children) with a
/// little vertical breathing room.
fn menu_sep(color: Color) -> Divider {
    Divider::horizontal()
        .length(Size::fill_minimum())
        .color(color)
        .margin(Gaps::new_all(4.))
}
