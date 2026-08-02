//! The frame the three drawer bodies render inside (P3-11 deferred it here, with Problems as its
//! first consumer). Deliberately just two pieces — a scroll container and a centred empty state —
//! because
//! that is all the three genuinely have in common: Problems is a pinned group header over
//! icon·message·line rows, Events is flat bottom-bordered dot·message·timestamp rows, and History
//! is a card with a meta line over a two-line SQL preview. Anything more would be one tab's shape
//! wearing a shared name.

use freya::components::ScrollView;
use freya::prelude::*;

use crate::components::icon::{Icon, IconName};
use crate::components::typography::Body;

/// The scrolling body under a drawer tab's header, with the canvas's vertical inset
/// (`padding: var(--sp-2) 0`).
#[derive(PartialEq)]
pub struct DrawerBody {
    children: Vec<Element>,
}

impl DrawerBody {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }
}

impl ChildrenExt for DrawerBody {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Component for DrawerBody {
    fn render(&self) -> impl IntoElement {
        ScrollView::new().child(
            rect()
                .width(Size::fill())
                .vertical()
                .padding((4., 0.))
                .children(self.children.clone()),
        )
    }
}

/// A drawer tab with nothing to show: its glyph over one line of copy, centred in the body.
/// The glyph's colour is the caller's, because each tab's is **semantic** — Problems' tick is the
/// sheet's `success` — and semantic colours are read off the sheet wherever they appear
/// (AGENTS.md §3).
#[derive(PartialEq)]
pub struct DrawerEmpty {
    icon: IconName,
    icon_color: Color,
    text: String,
    color: Color,
}

impl DrawerEmpty {
    pub fn new(icon: IconName, text: impl Into<String>) -> Self {
        Self {
            icon,
            icon_color: Color::WHITE,
            text: text.into(),
            color: Color::WHITE,
        }
    }

    pub fn icon_color(mut self, color: Color) -> Self {
        self.icon_color = color;
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }
}

impl Component for DrawerEmpty {
    fn render(&self) -> impl IntoElement {
        // Centred *inside a scroll view*, not centred in a bare box (P5-06): a drawer dragged to
        // its stub is shorter than the glyph plus its line of copy, and a centred box with no
        // scroll paints the pair straight through the header above it.
        ScrollView::new().child(
            rect()
                .expanded()
                .vertical()
                .main_align(Alignment::Center)
                .cross_align(Alignment::Center)
                .spacing(8.)
                .padding((0., 12.))
                .child(Icon::new(self.icon).color(self.icon_color).size(26.))
                .child(Body::new(self.text.clone()).color(self.color)),
        )
    }
}
