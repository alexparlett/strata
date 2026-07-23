//! The icon segmented toggle (design `segmented_toggle`): a general two/three-option
//! accent-tint segmented control — glyphs in one bordered pill with 1px dividers, the active
//! segment an accent-tint fill + accent icon. First used as the results Table/Chart switcher
//! (P2-07), but not specific to it — hence the shared component + its own theme component.
//!
//! Shaped like Freya's built-in `SegmentedButton`/`ButtonSegment`: the pill is a container,
//! each [`ToggleSegment`] child carries its own `selected` + `on_press` — the caller owns the
//! selection state.

use freya::components::use_theme;
use freya::prelude::*;

use crate::components::icon::{Icon, IconName};

define_theme!(
    %[component]
    pub SegmentedToggle {
        %[fields]
        background: Color,
        border_fill: Color,
        divider_fill: Color,
        item_color: Color,
        item_active_background: Color,
        item_active_color: Color,
    }
);

/// The pill: bordered, clipped container that interleaves a 1px divider between its
/// [`ToggleSegment`] children.
#[derive(PartialEq)]
pub struct SegmentedToggle {
    children: Vec<Element>,
    theme: Option<SegmentedToggleThemePartial>,
}

impl Default for SegmentedToggle {
    fn default() -> Self {
        Self::new()
    }
}

impl SegmentedToggle {
    pub fn new() -> Self {
        Self { children: Vec::new(), theme: None }
    }
}

impl ChildrenExt for SegmentedToggle {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Component for SegmentedToggle {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&self.theme, SegmentedToggleThemePreference, "segmented_toggle");

        let mut pill = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .corner_radius(8.)
            .background(theme.background)
            .border(Border::new().width(1.).fill(theme.border_fill))
            .overflow(Overflow::Clip);
        for (i, segment) in self.children.iter().enumerate() {
            if i > 0 {
                pill = pill.child(
                    rect().width(Size::px(1.)).height(Size::px(24.)).background(theme.divider_fill),
                );
            }
            pill = pill.child(segment.clone());
        }
        pill
    }
}

/// One 32×24 segment: a glyph wearing its tooltip `title`, the active dress (accent tint +
/// accent glyph) when `selected`, and the comp's soft hover (a 7% text-colour overlay,
/// semantic — read from the palette) otherwise.
#[derive(PartialEq)]
pub struct ToggleSegment {
    icon: IconName,
    title: Option<String>,
    selected: bool,
    on_press: Option<EventHandler<Event<PressEventData>>>,
    theme: Option<SegmentedToggleThemePartial>,
}

impl ToggleSegment {
    pub fn new(icon: IconName) -> Self {
        Self { icon, title: None, selected: false, on_press: None, theme: None }
    }

    /// The tooltip this segment wears (the comp's `title=`).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn on_press(mut self, on_press: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(on_press.into());
        self
    }
}

impl Component for ToggleSegment {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&self.theme, SegmentedToggleThemePreference, "segmented_toggle");
        let hover = use_theme().read().colors.text_primary.with_a(18);
        let mut hovered = use_state(|| false);

        let background = if self.selected {
            theme.item_active_background
        } else if hovered() {
            hover
        } else {
            Color::TRANSPARENT
        };
        let on_press = self.on_press.clone();
        let segment = rect()
            .width(Size::px(32.))
            .height(Size::px(24.))
            .center()
            .background(background)
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set(false))
            .on_press(move |e| {
                if let Some(on_press) = &on_press {
                    on_press.call(e);
                }
            })
            .child(
                Icon::new(self.icon)
                    .color(if self.selected { theme.item_active_color } else { theme.item_color })
                    .size(15.),
            );
        match &self.title {
            Some(title) => TooltipContainer::new(Tooltip::new(title.clone()))
                .position(AttachedPosition::Bottom)
                .child(segment)
                .into_element(),
            None => segment.into_element(),
        }
    }
}
