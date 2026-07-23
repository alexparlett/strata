//! The results pane's **Chart** body (P2-07): the shared results toolbar (whose toggle is how
//! you got here) over a placeholder — the real chart (control strip + canvas, CHART_SPEC) is
//! the Chart workstream's. The switcher, per-tab mode, and this body slot are the real
//! mechanism it lands into.

use freya::components::use_theme;
use freya::prelude::*;

use super::find::FindState;
use super::toolbar::ResultsToolbar;
use crate::apps::project::state::TabId;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Prose, Title};

/// The chart body: toolbar on top, centered placeholder tile below (the empty state's dress).
#[derive(PartialEq)]
pub struct ChartView {
    tab: TabId,
    find: FindState,
}

impl ChartView {
    pub fn new(tab: TabId, find: FindState) -> Self {
        Self { tab, find }
    }
}

impl Component for ChartView {
    fn render(&self) -> impl IntoElement {
        let theme = use_theme();
        let (tile_bg, tile_border, icon_color, title_color, sub_color, background) = {
            let c = &theme.read().colors;
            (
                c.surface_tertiary,
                c.border,
                c.text_placeholder,
                c.text_secondary,
                c.text_placeholder,
                c.surface_secondary,
            )
        };

        rect()
            .width(Size::fill())
            .height(Size::fill())
            .content(Content::Flex)
            .child(ResultsToolbar::new(self.tab, self.find))
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .vertical()
                    .main_align(Alignment::Center)
                    .cross_align(Alignment::Center)
                    .spacing(12.)
                    .background(background)
                    .child(
                        rect()
                            .width(Size::px(46.))
                            .height(Size::px(46.))
                            .corner_radius(8.)
                            .background(tile_bg)
                            .border(Border::new().width(1.).fill(tile_border))
                            .center()
                            .child(Icon::new(IconName::Chart).color(icon_color).size(22.)),
                    )
                    .child(Title::new("Chart view isn't built yet").color(title_color))
                    .child(
                        Prose::new(
                            "It will chart this snapshot — the same rows the grid pages over. \
                             Switch back to Table to keep browsing them.",
                        )
                        .color(sub_color),
                    ),
            )
    }
}
