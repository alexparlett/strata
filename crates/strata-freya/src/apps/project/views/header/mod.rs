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
use crate::platform::open_settings;
use crate::state::AppCtx;

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

/// The header's **window chrome alone** — the drag + double-press-to-fill recipe over the
/// bar's height, with none of the project content. For the load-fault arm, which replaces
/// the whole subtree including [`HeaderBar`] but is still a window the user must be able to
/// move (the OS traffic lights sit in the same corner either way). Transparent: it is a
/// region, not a surface.
#[derive(PartialEq)]
pub struct WindowDragStrip {
    /// The window's fill mark — the same flag [`HeaderBar`] writes, owned by the window
    /// root, so a fill toggled from the fault arm is tracked like any other.
    pub filled_by_app: State<bool>,
}

impl Component for WindowDragStrip {
    fn render(&self) -> impl IntoElement {
        // The unfill-clears-the-mark rule, same as the full bar's: leaving fill by any route
        // makes a later user-side fill persistable again.
        let is_filled = Platform::get().is_maximized;
        let mut filled_by_app = self.filled_by_app;
        use_side_effect(move || {
            if !*is_filled.read() && *filled_by_app.peek() {
                filled_by_app.set(false);
            }
        });

        rect()
            .width(Size::window_percent(100.))
            .height(Size::px(48.))
            .on_pointer_down(title_bar_press(is_filled, self.filled_by_app))
    }
}

impl Component for HeaderBar {
    fn render(&self) -> impl IntoElement {
        // Only the bar's own surface is themed here. Its content — the switcher's accent glyph,
        // the dropdown's text ramp — is root palette colours read through the normal hook, and
        // its avatars / separators are the shared `Avatar` and `Divider::menu` components.
        let HeaderBarTheme {
            background,
            border_fill,
            color,
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

        // The command palette is P6-01, so that button exists (with its live chord in the
        // tooltip) and logs until it lands — the same stub `project.rs`'s catch-all holds for
        // the chord, reachable by mouse. The gear opens the real Settings window.
        let search_title = use_hint_title("Search", Command::CommandPalette);
        let settings_title = use_hint_title("Settings", Command::OpenSettings);
        // The Settings window is opened through the shared path, which needs this window's
        // platform handle (that is how it learns *which* window asked, so it can pin itself
        // above this one) and the app-globals.
        let platform = use_hook(Platform::get);
        let app = use_consume::<AppCtx>();

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
            TooltipContainer::new(Tooltip::new_text(title))
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
                    open_settings(platform.clone(), app.clone());
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
