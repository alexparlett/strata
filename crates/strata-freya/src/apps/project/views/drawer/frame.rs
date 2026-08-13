//! The frame the three drawer bodies render inside (P3-11 deferred it here, with Problems as its
//! first consumer). Deliberately just two pieces — a scroll container and a centred empty state —
//! because
//! that is all the three genuinely have in common: Problems is a pinned group header over
//! icon·message·line rows, Events is flat bottom-bordered dot·message·timestamp rows, and History
//! is a card with a meta line over a two-line SQL preview. Anything more would be one tab's shape
//! wearing a shared name.

use freya::components::ScrollView;
use freya::prelude::*;

use super::{DrawerThemePartial, DrawerThemePreference};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_2, SP_3, SP_4};
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
                .padding((SP_2, 0.))
                .children(self.children.clone()),
        )
    }
}

/// A drawer tab with nothing to show: its glyph over one line of copy, centred in the body.
/// Both paint the drawer theme's `empty_color`; the glyph alone stays overridable because a
/// tab's can be **semantic** — Problems' tick is the shared ramp's `ok` — and semantic colours
/// follow the app-wide ramp wherever they appear (AGENTS.md §3).
#[derive(PartialEq)]
pub struct DrawerEmpty {
    icon: IconName,
    icon_color: Option<Color>,
    text: String,
}

impl DrawerEmpty {
    pub fn new(icon: IconName, text: impl Into<String>) -> Self {
        Self {
            icon,
            icon_color: None,
            text: text.into(),
        }
    }

    pub fn icon_color(mut self, color: Color) -> Self {
        self.icon_color = Some(color);
        self
    }
}

impl Component for DrawerEmpty {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<DrawerThemePartial>, DrawerThemePreference, "drawer");
        let icon_color = self.icon_color.unwrap_or(theme.empty_color);
        let color = theme.empty_color;
        ScrollView::new().child(
            rect()
                .expanded()
                .vertical()
                .main_align(Alignment::Center)
                .cross_align(Alignment::Center)
                .spacing(SP_3)
                .padding((0., SP_4))
                .child(Icon::new(self.icon).color(icon_color).size(26.))
                .child(Body::new(self.text.clone()).color(color)),
        )
    }
}
