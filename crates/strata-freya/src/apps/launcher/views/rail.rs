//! The launcher's left rail: the brand block, the active **Projects** nav pill, and the
//! **Settings** row pinned to the bottom.
//!
//! The pill is tinted background only — no left accent bar, and a text-coloured label
//! (V26). There is exactly one destination today, so it is decoration with a job: it says
//! *where you are*, which is what makes the Settings row read as the other place to go.
//!
//! Both rows are Freya's [`SideBarItem`], which already carries the hover fill, the padding
//! and the rule that an **active** item doesn't light on hover — and whose `sidebar_item`
//! theme both theme files already author. The rail *container* is hand-rolled because the
//! fork ships no `SideBar`: its own example builds one from a `rect` too.

use freya::prelude::*;

use crate::apps::launcher::{LauncherThemePartial, LauncherThemePreference};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Control, Meta, Title};

/// The rows' corner radius (the canvas's `--r-1`); `sidebar_item` ships 12.
const ROW_RADIUS: f32 = 6.;

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

        // The brand: the app mark in a rounded, clipped tile (the SVG is square and paints
        // its own colours), the wordmark in the scale's Title role, and the build under it.
        let brand = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(12.)
            .padding(Gaps::new(0., 4., 16., 4.))
            .child(
                rect()
                    .width(Size::px(34.))
                    .height(Size::px(34.))
                    .corner_radius(8.)
                    .overflow(Overflow::Clip)
                    .child(Icon::new(IconName::StrataLogo).size(34.)),
            )
            .child(
                rect()
                    .vertical()
                    .spacing(2.)
                    .child(Title::new("Strata"))
                    .child(Meta::new(env!("CARGO_PKG_VERSION")).color(theme.label_color)),
            );

        // The current destination, wearing the accent tint. `Activable` is what
        // `SideBarItem` reads its active state from, and active outranks hover in its own
        // dress — so it stays put under the pointer, which is what a "you are here" marker
        // should do.
        let projects = Activable::new(
            SideBarItem::new()
                .theme(
                    SideBarItemThemePartial::default()
                        .active_background(theme.nav_background)
                        .corner_radius(CornerRadius::new_all(ROW_RADIUS)),
                )
                .child(row_content(IconName::Folder, "Projects")),
        )
        .active(true);

        // The gear (W1) — the standalone Settings window is P4-03, so the row is live and
        // logs until it lands. Wiring it here rather than at that seam would fold the
        // single-instance settings window into the launcher.
        let settings = SideBarItem::new()
            .theme(
                SideBarItemThemePartial::default()
                    .color(theme.muted_color)
                    .corner_radius(CornerRadius::new_all(ROW_RADIUS)),
            )
            .on_press(move |_: Event<PressEventData>| {
                tracing::debug!("launcher: settings window not built yet (P4-03)");
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
                    .padding(Gaps::new(16., 12., 16., 12.))
                    .spacing(4.)
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
/// active fills — is `SideBarItem`'s.
fn row_content(icon: IconName, label: &str) -> impl IntoElement {
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(12.)
        .child(Icon::new(icon).size(15.))
        .child(Control::new(label))
}
