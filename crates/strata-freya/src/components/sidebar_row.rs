//! One **sidebar row** — the shared shell behind every clickable row in the left pane: the
//! catalog's tables, views, columns and saved queries (P3-02), and the connections list (W7).
//!
//! It is a thin preset over Freya's [`SideBarItem`], not a component of our own, because that
//! already carries everything the rows genuinely share:
//!
//! - the **idle / hover / selected** background state machine, from the one `sidebar_item` theme
//!   key (so the hover fill can't drift between panes, which is exactly what happened when each
//!   row hand-rolled its own `on_pointer_enter` and its own token);
//! - **accessibility** — an a11y id, `Link` role, keyboard focusability and a focus ring, none of
//!   which the hand-rolled rows had.
//!
//! Selection rides Freya's [`Activable`], whose own docs name `SideBarItem` as the case it exists
//! for: it provides the `ActivableContext` that `SideBarItem::use_is_active` reads. No fork change
//! was needed.
//!
//! What is **not** shared is geometry: across the four row types the height (30 / 25 / 30 / auto),
//! corner radius (6 / 6 / 6 / 8), padding and gap all differ, so those stay builder parameters set
//! by the caller rather than pretending to be one row.

use freya::components::{Activable, SideBarItem, SideBarItemThemePartial};
use freya::prelude::*;

/// Vertical gap between consecutive rows (design `--sp-1`).
const ROW_GAP: f32 = 2.;

#[derive(PartialEq)]
pub struct SidebarRow {
    height: Option<f32>,
    padding: Gaps,
    radius: f32,
    spacing: f32,
    selected: bool,
    on_press: Option<EventHandler<Event<PressEventData>>>,
    children: Vec<Element>,
}

impl SidebarRow {
    /// A row with the catalog's default dress: fixed 30px, `--r-1` corners, `--sp-3` gaps.
    pub fn new() -> Self {
        Self {
            height: Some(30.),
            padding: Gaps::new(0., 4., 0., 8.),
            radius: 6.,
            spacing: 8.,
            selected: false,
            on_press: None,
            children: Vec::new(),
        }
    }

    /// Fix the row's height. Omit (via [`auto_height`](Self::auto_height)) for a row that grows
    /// with its content, like the connections list's two-line entries.
    pub fn height(mut self, height: f32) -> Self {
        self.height = Some(height);
        self
    }

    /// Let the row hug its content instead of taking a fixed height.
    #[allow(dead_code)] // Feature reservoir: the connections list's two-line rows (W7).
    pub fn auto_height(mut self) -> Self {
        self.height = None;
        self
    }

    pub fn padding(mut self, padding: impl Into<Gaps>) -> Self {
        self.padding = padding.into();
        self
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Paint the row in the selected dress (`sidebar_item`'s `active_background`).
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn on_press(mut self, on_press: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(on_press.into());
        self
    }
}

impl ChildrenExt for SidebarRow {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Component for SidebarRow {
    fn render(&self) -> impl IntoElement {
        // Geometry rides a per-instance theme override; the colours stay the shared `sidebar_item`
        // theme's, so a row can't quietly invent its own hover fill.
        let theme = SideBarItemThemePartial::new()
            .padding(self.padding)
            .corner_radius(CornerRadius::new_all(self.radius))
            .margin(Gaps::new(0., 0., ROW_GAP, 0.));

        // `Content::Flex` so a `Size::flex` child (the truncating name) actually distributes —
        // without it the row hugs its content and pushes its trailing run out of the panel.
        let content = rect()
            .width(Size::fill())
            .map(self.height, |el, h| el.height(Size::px(h)))
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(self.spacing)
            .children(self.children.clone());

        let item = SideBarItem::new()
            .theme(theme)
            .maybe(self.on_press.is_some(), |el| {
                let handler = self.on_press.clone();
                el.on_press(move |e: Event<PressEventData>| {
                    if let Some(handler) = &handler {
                        handler.call(e);
                    }
                })
            })
            .child(content);

        Activable::new(item).active(self.selected)
    }
}
