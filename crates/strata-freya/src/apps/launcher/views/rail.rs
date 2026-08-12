//! The launcher's left rail: the brand block, the active **Projects** nav pill, and the
//! **Settings** row pinned to the bottom.
//!
//! The pill is tinted background only — no left accent bar, and a text-coloured label
//! (V26). There is exactly one destination today, so it is decoration with a job: it says
//! *where you are*, which is what makes the Settings row read as the other place to go.
//!
//! Both rows are [`SidebarRow`] — the same preset the catalog's rows use, so the hover fill
//! and the a11y (focusable, `Link` role, focus ring) can't drift between the two panes. Only
//! the *selected* fill differs: this rail marks where you are with the canvas's accent tint
//! rather than the catalog's neutral selection grey. The rail *container* is hand-rolled
//! because the fork ships no `SideBar`: its own example builds one from a `rect` too.

use freya::prelude::*;

use crate::apps::launcher::{LauncherThemePartial, LauncherThemePreference};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{R_2, SP_1, SP_2, SP_3, SP_4, SP_5};
use crate::components::sidebar_row::SidebarRow;
use crate::components::typography::{Control, Meta, Title};
use crate::platform::open_settings;
use crate::state::AppCtx;

/// The rail rows' padding (canvas `--sp-3 --sp-4`) — roomier than the catalog's, which sits
/// in a narrower pane.
const ROW_PADDING: Gaps = Gaps::new(SP_3, SP_4, SP_3, SP_4);

/// The rail's width (canvas `width: 200px`), the hairline included.
const RAIL_WIDTH: f32 = 200.;

#[derive(PartialEq)]
pub struct LauncherRail;

impl Component for LauncherRail {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<LauncherThemePartial>,
            LauncherThemePreference,
            "launcher"
        );
        // The Settings window is opened through the shared path, which needs both this
        // window's platform handle (that is how it learns *which* window asked, so it can pin
        // itself above this one) and the app-globals.
        let platform = use_hook(Platform::get);
        let app = use_consume::<AppCtx>();

        // The brand: the app mark in a rounded, clipped tile (the SVG is square and paints
        // its own colours), the wordmark in the scale's Title role, and the build under it.
        let brand = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(SP_4)
            .padding(Gaps::new(0., SP_2, SP_5, SP_2))
            .child(
                rect()
                    .width(Size::px(34.))
                    .height(Size::px(34.))
                    .corner_radius(R_2)
                    .overflow(Overflow::Clip)
                    .child(Icon::new(IconName::StrataLogo).size(34.)),
            )
            .child(
                rect()
                    .vertical()
                    .spacing(SP_1)
                    .child(Title::new("Strata"))
                    .child(Meta::new(env!("CARGO_PKG_VERSION")).color(theme.label_color)),
            );

        // The current destination. `selected` outranks hover in the row's own dress, so the
        // pill stays put under the pointer, which is what a "you are here" marker should do.
        let projects = SidebarRow::new()
            .auto_height()
            .padding(ROW_PADDING)
            .selected(true)
            .active_background(theme.nav_background)
            .child(row_content(IconName::Folder, "Projects"));

        // The gear (W1): the standalone Settings window, opened through
        // `platform::open_settings` — so it is the same single instance ⌘, and the project
        // header's gear reach, pinned above this window.
        let settings = SidebarRow::new()
            .auto_height()
            .padding(ROW_PADDING)
            .on_press(move |_: Event<PressEventData>| {
                open_settings(platform.clone(), app.clone());
            })
            .child(row_content(IconName::Gear, "Settings"));

        rect()
            .width(Size::px(RAIL_WIDTH))
            .height(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .child(
                rect()
                    .width(Size::flex(1.))
                    .height(Size::fill())
                    .vertical()
                    // `Content::Flex` is what makes the spacer below actually flex — without
                    // it a `Size::flex` child grows to the whole axis and pushes Settings
                    // off the bottom.
                    .content(Content::Flex)
                    .background(theme.rail_background)
                    .padding(Gaps::new(SP_5, SP_4, SP_5, SP_4))
                    .spacing(SP_2)
                    .child(brand)
                    .child(projects)
                    // Flexible spacer — pins Settings to the bottom.
                    .child(rect().width(Size::px(1.)).height(Size::flex(1.)))
                    .child(settings),
            )
            .child(Divider::vertical().color(theme.border_fill))
    }
}

/// A rail row's content: glyph + label. The pill around it — padding, radius, hover and
/// selected fills — is [`SidebarRow`]'s.
fn row_content(icon: IconName, label: &str) -> impl IntoElement {
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(SP_4)
        .child(Icon::new(icon).size(15.))
        .child(Control::new(label))
}
