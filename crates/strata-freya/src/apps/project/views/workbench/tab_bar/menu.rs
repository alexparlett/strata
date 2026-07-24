//! The tab's right-click context menu: one [`Menu`] of tab actions, opened at the cursor via
//! [`ContextMenu::open_from_event`] by the right-clicked tab and scoped to *that* tab.
//!
//! A faithful port of the Dioxus `tab_menu_items`: Rename · Duplicate — Close · Close others · Close
//! to the right · Close all — Reopen closed tab. `MenuButton` / `MenuContainer` are theme-driven (the
//! same components the tab-controls dropdowns use), so only the separators take an explicit colour.

use freya::prelude::*;
use freya::radio::Radio;
use strata_core::config::Command;

use crate::apps::project::state::{Chan, SessionState, TabId};
use crate::components::divider::Divider;
use crate::components::typography::Prose;
use crate::keymap::KeyHint;

/// A comfortable width for a menu holding [`menu_row`]s — room for the longest label plus
/// its right-aligned chord. Doubles as each row's width cap and the menu's `min_width`:
/// the cap stops an uncapped `fill` blowing the menu out to the window width in the
/// globally-positioned `ContextMenu` overlay (whose available width is the window), and
/// the `min_width` guarantees the row that same room inside an `Attached` dropdown
/// (whose available width is the trigger's — without it the row squeezes and
/// `MenuButton`'s `Overflow::Clip` cuts the hint off).
pub const HINT_MENU_WIDTH: f32 = 200.;

/// The horizontal chrome around a menu row: the `menu_container` card padding (4 × 2)
/// plus `MenuButton`'s default padding (12 × 2). Subtracted from [`HINT_MENU_WIDTH`] so
/// a row-capped menu (the `ContextMenu`, where the row is what sets the width) lands on
/// the same card width as a `min_width`-floored dropdown.
const MENU_ROW_CHROME: f32 = 32.;

/// A menu row with a right-aligned, keymap-derived shortcut hint: the row fills the
/// available width capped so the menu's card lands at [`HINT_MENU_WIDTH`], and
/// `SpaceBetween` pushes the hint to its right edge, tracking any rebind reactively
/// ([`KeyHint`] renders nothing when unbound).
pub fn menu_row(label: &str, hint: Command) -> impl IntoElement {
    rect()
        .horizontal()
        .width(Size::fill())
        .max_width(Size::px(HINT_MENU_WIDTH - MENU_ROW_CHROME))
        .cross_align(Alignment::Center)
        .main_align(Alignment::SpaceBetween)
        .spacing(16.)
        .child(Prose::new(label))
        .child(KeyHint(hint))
}

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
    closer: crate::apps::project::close::TabCloser,
    settings: State<strata_core::config::Settings>,
) -> Menu {
    // Built from an event handler (no reactive context), so this `read` is a peek: the reopen
    // stack can't change while this transient menu is up (reopening dismisses it), so the state
    // at open time is the state for its whole life.
    let can_reopen = !radio.read().closed.is_empty();

    Menu::new()
        .min_width(Size::px(HINT_MENU_WIDTH))
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
                    // Through the shared gate — the T2 confirm when this tab's query
                    // is in flight.
                    closer.close(radio, settings, id);
                    ContextMenu::close();
                })
                .child(menu_row("Close", Command::CloseActiveTab)),
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
                .enabled(can_reopen)
                .on_press(move |_| {
                    radio.write().reopen_last();
                    ContextMenu::close();
                })
                .child(menu_row("Reopen closed tab", Command::ReopenTab)),
        )
}

/// A menu divider: the shared [`Divider`] configured for a hug-content menu — `fill_minimum` (a plain
/// `fill` would blow the menu out to the window width, since the container hugs its children) with a
/// little vertical breathing room.
pub fn menu_sep(color: Color) -> Divider {
    Divider::horizontal()
        .length(Size::fill_minimum())
        .color(color)
        .margin(Gaps::new_all(4.))
}
