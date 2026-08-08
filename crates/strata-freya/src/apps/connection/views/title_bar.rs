//! The connection editor's title bar — **and the window's**: a 50px strip carrying the store
//! tile, what the window is doing and which connection it is doing it to, with the real OS
//! traffic lights floating at its left.
//!
//! Nothing here is interactive, so the whole strip is the drag region — the same recipe as the
//! Configure, Export, Settings and launcher bars.

use freya::prelude::*;

use crate::apps::connection::ConnectionCtx;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Meta, Title};
use crate::components::window::window_theme;
use crate::theme::{use_roles, Role};

/// The strip's height (canvas `padding: var(--sp-4) var(--sp-5)` around a 26px tile).
pub const TITLE_BAR_HEIGHT: f32 = 50.;

/// The gutter that keeps the bar's content clear of the OS traffic lights — the same reserve
/// every other window's bar keeps.
const TRAFFIC_LIGHT_GUTTER: f32 = 82.;

#[derive(PartialEq)]
pub struct TitleBar;

impl Component for TitleBar {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        // The subtitle's tone. Through the roles because the window theme has no text field —
        // it dresses chrome (surfaces, rules, the mark), and a recessive line of prose is the
        // semantic ramp's, exactly as the Configure window's status block reads it.
        let muted = use_roles().get(Role::TextMuted);
        let ctx = use_consume::<ConnectionCtx>();
        let target = ctx.target.read();

        // The window's mark: the same glyph the Connections pane and the activity rail use, in
        // an accent-tinted tile.
        let mark = rect()
            .width(Size::px(26.))
            .height(Size::px(26.))
            .corner_radius(6.)
            .center()
            .background(win.icon_background)
            .child(
                Icon::new(IconName::Connections)
                    .size(15.)
                    .color(win.icon_color),
            );

        // Title over the connection it names — the canvas's two-line block, and one line on a
        // new connection, whose URL is still being typed.
        let heading = rect()
            .vertical()
            .spacing(2.)
            .width(Size::flex(1.))
            .child(Title::new(target.title()))
            .maybe_child(
                target
                    .subtitle()
                    .map(|url| Meta::new(url).color(muted).into_element()),
            );

        rect()
            .width(Size::fill())
            .height(Size::px(TITLE_BAR_HEIGHT))
            .vertical()
            .content(Content::Flex)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(12.)
                    .padding(Gaps::new(0., 16., 0., TRAFFIC_LIGHT_GUTTER))
                    .window_drag()
                    .child(mark)
                    .child(heading),
            )
            .child(Divider::horizontal().color(win.border_fill))
    }
}
