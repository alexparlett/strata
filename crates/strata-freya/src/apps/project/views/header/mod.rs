//! The window header — **and the window's title bar** (design `Header.dc.html` /
//! `Strata.dc.html` `data-rg="header"`): the brand, the project switcher, and the ⌘K palette +
//! ⌘, settings cluster, over the 48px strip the OS traffic lights float in.
//!
//! **It is the title bar.** The window ships transparent-titlebar + fullsize-content-view +
//! hidden-title (see [`ProjectApp::window`]), so this bar *is* the strip AppKit would have
//! drawn, and has to behave like one:
//!
//! - the traffic lights are the real OS buttons, inset to sit in this bar — the left padding is
//!   the gutter that keeps them clear of the brand;
//! - pressing the bar's background drags the window, and double-pressing it **fills** the window
//!   to the current monitor (macOS *zoom*) or restores its previous size — winit's `drag_window`
//!   / `set_maximized` pair, the fork's `WindowDragExt::window_drag` recipe kept here for the
//!   reason in [`title_bar_press`]. Filling is not native fullscreen; that stays the green
//!   button's job;
//! - every interactive child stops the pointer-down from reaching the bar, so pressing a control
//!   never drags the window (the Dioxus header did the same, one cluster at a time).
//!
//! **Our** fill is not remembered (nor is fullscreen): `use_autosave` keeps persisting the last
//! geometry from before it, so a restart reopens at the size the user chose — normal IDE
//! behaviour. A window the *user* sized to fill the screen is a different thing and does persist;
//! [`title_bar_press`] is where the two are told apart.
//!
//! [`ProjectApp::window`]: crate::apps::project::ProjectApp::window
//! [`use_autosave`]: crate::apps::project::state::use_autosave

mod project_menu;

use freya::prelude::*;
use strata_core::config::Command;

use self::project_menu::ProjectMenu;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Title;
use crate::keymap::use_hint_title;

/// The gutter that keeps the bar's content clear of the OS traffic lights. The window insets
/// them to (13, 16) (`with_traffic_light_inset`), so the three buttons end around x = 67; 82 is
/// the reserve the Dioxus app shipped (`.ps-app.mac .ps-header`).
const TRAFFIC_LIGHT_GUTTER: f32 = 82.;

define_theme!(
    %[component]
    pub HeaderBar {
        %[fields]
        background: Color,
        color: Color,
        border_fill: Color,
        /// The switcher's folder glyph.
        accent: Color,
        /// The switcher dropdown: the rule between groups, the section labels, and a row's
        /// name / path.
        menu_divider_fill: Color,
        menu_label_color: Color,
        menu_name_color: Color,
        menu_path_color: Color,
    }
);

#[derive(PartialEq)]
pub struct HeaderBar {
    /// Set while *this* bar's double-press is what filled the window — the window root owns the
    /// flag because `use_autosave` reads it too (a prop, not context: one known consumer).
    pub filled_by_app: State<bool>,
    pub theme: Option<HeaderBarThemePartial>,
}

impl HeaderBar {
    pub fn new(filled_by_app: State<bool>) -> Self {
        Self {
            filled_by_app,
            theme: None,
        }
    }
}

/// The title bar's press handler: **drag** on a single press, toggle **fill** on a double press.
///
/// This is the fork's `WindowDragExt::window_drag` recipe, kept app-side for one reason: the
/// session has to know whether a fill was *ours*. macOS's `isZoomed` — what
/// `Platform::is_maximized` mirrors — is a **frame comparison**, not a state flag, so a window the
/// user sized to the screen themselves (macOS 15's edge-tiling, or the green button's *Fill*)
/// reports zoomed exactly like our double-press does. That size is one they chose and must
/// persist; ours is transient. Marking the fills we initiate is what separates them — see
/// [`use_autosave`](crate::apps::project::state::use_autosave).
fn title_bar_press(
    is_filled: State<bool>,
    mut filled_by_app: State<bool>,
) -> impl FnMut(Event<PointerEventData>) {
    move |e: Event<PointerEventData>| match EventsCombos::pressed(e.global_location()) {
        PressEventType::Single => Platform::get().with_window(None, |window| {
            let _ = window.drag_window();
        }),
        PressEventType::Double => {
            // Decide the direction from the mirrored state (up to date, and readable here —
            // unlike inside the renderer callback), then mark it as ours before dispatching.
            let filling = !*is_filled.peek();
            filled_by_app.set(filling);
            Platform::get().with_window(None, move |window| window.set_maximized(filling));
        }
        _ => {}
    }
}

impl Component for HeaderBar {
    fn render(&self) -> impl IntoElement {
        // The bar's own surface is `header_bar`'s; the *content* tints come from the sheet, like
        // the activity rail's — the palette already carries them, and none of them is a
        // header-only dress.
        let HeaderBarTheme {
            background,
            border_fill,
            color,
            ..
        } = get_theme!(&self.theme, HeaderBarThemePreference, "header_bar");

        // The window's live fill state (the fork's mirror of winit's `is_maximized`). Leaving
        // fill by *any* route — our double-press, the OS, dragging the window out of a tile —
        // clears our mark, so a later user-side fill is never mistaken for ours and stays
        // persistable.
        let is_filled = Platform::get().is_maximized;
        let mut filled_by_app = self.filled_by_app;
        use_side_effect(move || {
            if !*is_filled.read() && *filled_by_app.peek() {
                filled_by_app.set(false);
            }
        });

        // Both actions are placeholders: the command palette is P6-01 and the settings window is
        // P4-03, so the buttons exist (with their live chord in the tooltip) and log until those
        // land. Their chords are already consumed by `project.rs`'s catch-all — this is the same
        // stub, reachable by mouse.
        let search_title = use_hint_title("Search", Command::CommandPalette);
        let settings_title = use_hint_title("Settings", Command::OpenSettings);

        // The brand: the app mark in a rounded, clipped tile (the SVG is square and paints its
        // own colours), then the wordmark in the scale's Title role — ui 600 14.5, the comp's.
        let brand = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.)
            .child(
                rect()
                    .width(Size::px(22.))
                    .height(Size::px(22.))
                    .corner_radius(6.)
                    .overflow(Overflow::Clip)
                    .child(Icon::new(IconName::StrataLogo).size(22.)),
            )
            .child(Title::new("Strata"));

        // A 30×30 action button wearing the standard `button` dress — which *is* the comp's
        // `c-elev` fill + `c-border2` hairline + muted glyph. The pointer-down stop is what keeps
        // a press on it from dragging the window.
        let action = |icon: IconName, size: f32| {
            Button::new()
                .width(Size::px(30.))
                .height(Size::px(30.))
                .on_pointer_down(|e: Event<PointerEventData>| e.stop_propagation())
                .child(Icon::new(icon).size(size))
        };
        let tip = |title: String, button: Button| {
            TooltipContainer::new(Tooltip::new(title))
                .position(AttachedPosition::Bottom)
                .child(button)
        };

        let cluster = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.)
            .child(tip(
                search_title,
                action(IconName::Search, 15.).on_press(move |_| {
                    tracing::debug!("header: command palette not built yet (P6-01)");
                }),
            ))
            .child(tip(
                settings_title,
                action(IconName::Gear, 16.).on_press(move |_| {
                    tracing::debug!("header: settings window not built yet (P4-03)");
                }),
            ));

        let bar = rect()
            .width(Size::fill())
            .height(Size::flex(1.))
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .padding(Gaps::new(0., 12., 0., TRAFFIC_LIGHT_GUTTER))
            .spacing(12.)
            // Drag / double-press-to-fill: on the bar itself, so it covers the whole strip except
            // the controls that opt out above.
            .on_pointer_down(title_bar_press(is_filled, self.filled_by_app))
            .child(brand)
            .child(Divider::vertical().length(Size::px(20.)).color(border_fill))
            .child(ProjectMenu)
            // Flexible spacer — pins the cluster to the right edge.
            .child(rect().height(Size::px(1.)).width(Size::flex(1.)))
            .child(cluster);

        rect()
            .background(background)
            .color(color)
            .content(Content::Flex)
            .height(Size::px(48.))
            .width(Size::fill())
            .child(bar)
            .child(Divider::horizontal().color(border_fill))
    }
}
