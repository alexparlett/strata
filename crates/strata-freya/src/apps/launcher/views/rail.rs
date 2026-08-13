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
//!
//! **The version line is the update affordance** (UP-03). This is the one place the app
//! already talks about its own version, which makes it where an offer of a newer one belongs —
//! and the number it prints is [`CURRENT`], the same const the check compares against,
//! so the rail and the mechanism cannot disagree about what is running. What the action says
//! and what pressing it does are [`Affordance`]'s, shared with the menubar item; there is
//! nothing to draw for `Idle`, `UpToDate`, `Checking` or a failed check, which is what keeps
//! the rail from nagging.

use freya::prelude::*;

use crate::apps::launcher::{LauncherThemePartial, LauncherThemePreference};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{R_2, SP_1, SP_2, SP_3, SP_4, SP_5};
use crate::components::sidebar_row::SidebarRow;
use crate::components::typography::{Control, Meta, Title};
use crate::platform::open_settings;
use crate::state::{install_site, AppCtx, CURRENT};
use crate::theme::{use_roles, Role};
use crate::updater::{press, Affordance, UpdateAsk};

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
        let roles = use_roles();
        // Read, not peeked: this is the one surface that repaints as the updater learns
        // something. The confirm slot beside it is this window's own — the status is
        // app-global, the question is not.
        let status = app.updates;
        let ask = use_consume::<State<Option<UpdateAsk>>>();
        let affordance = Affordance::of(&status.read(), install_site());

        // The brand: the app mark in a rounded, clipped tile (the SVG is square and paints
        // its own colours), the wordmark in the scale's Title role, and the build under it.
        let brand = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(SP_4)
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
                    .child(Meta::new(CURRENT).color(theme.label_color)),
            );

        // The offer, under the brand rather than under the version run itself: the rail is
        // 200px wide and the text column beside the 34px mark has no room for a sentence.
        // Nothing renders at all for `Idle`, `UpToDate`, `Checking` or a failed check — the
        // rail states the version and stops, exactly as it did before this task.
        //
        // The note is given a width so a long line wraps instead of hugging its content off the
        // edge: a text run with no width never wraps, whatever its line cap. `fill` is only
        // worth anything because the column below is `fill` too — a hugging parent would size
        // itself from the brand row and this would wrap at *that*, half the rail's width.
        let note = affordance.note().map(|note| {
            Meta::new(note)
                .color(theme.label_color)
                .width(Size::fill())
                .wrap()
        });
        let action = affordance.action().map(|label| {
            Button::new()
                .flat()
                .compact()
                // The launcher's ghost-action dress (the pane's OPEN), in the accent: one
                // tone for the whole control, so the label cannot disagree with its box.
                .theme_colors(
                    ButtonColorsThemePartial::default()
                        .color(roles.get(Role::Accent))
                        .hover_color(roles.get(Role::Accent)),
                )
                .on_press(move |_: Event<PressEventData>| press(status, ask))
                .child(Control::new(label))
        });

        // `fill`, not the hug this column would otherwise take: the note above is a `fill`
        // child, and a `fill` child of a hugging parent fills whatever the *widest sibling*
        // came to — here the brand row, about half the rail. The size has to land on the node
        // the rail lays out (AGENTS.md §3), which is this wrapper.
        let brand = rect()
            .width(Size::fill())
            .vertical()
            .spacing(SP_2)
            .padding(Gaps::new(0., SP_2, SP_5, SP_2))
            .child(brand)
            .maybe_child(note)
            .maybe_child(action);

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
