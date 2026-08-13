//! One **sidebar row** — the shared shell behind every row in the left pane: the catalog's
//! tables, views, columns and saved queries (P3-02), and the connections list (W7).
//!
//! Most of them are clickable and one is not. A connection row has no `on_press` at all (its
//! actions are the ⋮ menu, `CONNECTIONS_SPEC.md` §1), and `SideBarItem` used to announce it as
//! a focusable `Link` regardless — a tab stop with a focus ring that no key could activate.
//! Fixed **in the fork**, not around it: role and focusability now follow whether the item is
//! pressable. Hover still paints on both, because a row you can right-click is a row worth
//! marking under the pointer.
//!
//! It is a thin preset over Freya's [`SideBarItem`], not a component of our own, because that
//! already carries everything the rows genuinely share:
//!
//! - the **idle / hover / selected** background state machine, from the one `sidebar_item` theme
//!   key (so the hover fill can't drift between panes, which is exactly what happened when each
//!   row hand-rolled its own `on_pointer_enter` and its own token);
//! - **accessibility** — an a11y id, and, on a row that is actually pressable, a `Link` role,
//!   keyboard focusability and a focus ring, none of which the hand-rolled rows had.
//!
//! Selection rides Freya's [`Activable`], whose own docs name `SideBarItem` as the case it exists
//! for: it provides the `ActivableContext` that `SideBarItem::use_is_active` reads. No fork change
//! was needed.
//!
//! What is **not** shared is geometry: across the row types the height (30 / 25 / 30 / auto),
//! corner radius, padding and gap all differ, so those stay builder parameters set by the caller
//! rather than pretending to be one row.

use crate::components::metrics::{SP_1, SP_2, SP_3};
use freya::components::{Activable, SideBarItem, SideBarItemThemePartial};
use freya::prelude::*;

/// Vertical gap between consecutive rows (design `--sp-1`).
const ROW_GAP: f32 = SP_1;

#[derive(PartialEq)]
pub struct SidebarRow {
    height: Option<f32>,
    padding: Gaps,
    radius: f32,
    spacing: f32,
    selected: bool,
    active_background: Option<Color>,
    on_press: Option<EventHandler<Event<PressEventData>>>,
    on_context_menu: Option<EventHandler<Event<PressEventData>>>,
    children: Vec<Element>,
}

impl SidebarRow {
    /// A row with the catalog's default dress: fixed 30px, `--r-1` corners, `--sp-3` gaps.
    pub fn new() -> Self {
        Self {
            height: Some(30.),
            padding: Gaps::new(0., SP_2, 0., SP_3),
            radius: 6.,
            spacing: 8.,
            selected: false,
            active_background: None,
            on_press: None,
            on_context_menu: None,
            children: Vec::new(),
        }
    }

    /// Fix the row's height. Omit (via [`auto_height`](Self::auto_height)) for a row that grows
    /// with its content, like the launcher rail's two-line Settings entry.
    pub fn height(mut self, height: f32) -> Self {
        self.height = Some(height);
        self
    }

    /// Let the row hug its content instead of taking a fixed height.
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

    /// Override the **selected** fill only — the one colour that legitimately differs by role.
    /// The catalog's rows mark a *selection* and wear `sidebar_item`'s neutral `active`; the
    /// launcher's rail marks *where you are* and wears the canvas's accent tint. Idle and hover
    /// stay the shared theme's either way, which is the drift this preset exists to prevent.
    pub fn active_background(mut self, active_background: Color) -> Self {
        self.active_background = Some(active_background);
        self
    }

    pub fn on_press(mut self, on_press: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(on_press.into());
        self
    }

    /// Right-click the row (P3-06's catalog menus). Carried by a wrapper rather than by
    /// [`SideBarItem`] itself, which exposes only `on_press` — and a wrapper is where it
    /// belongs anyway: the row shell owns the affordance, so the catalog and (later)
    /// connections can't each invent their own secondary-press handling.
    pub fn on_context_menu(
        mut self,
        on_context_menu: impl Into<EventHandler<Event<PressEventData>>>,
    ) -> Self {
        self.on_context_menu = Some(on_context_menu.into());
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
        let theme = SideBarItemThemePartial::new()
            .padding(self.padding)
            .corner_radius(CornerRadius::new_all(self.radius))
            .margin(Gaps::new(0., 0., ROW_GAP, 0.));
        let theme = match self.active_background {
            Some(background) => theme.active_background(background),
            None => theme,
        };

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

        let row = Activable::new(item).active(self.selected);

        match &self.on_context_menu {
            None => row.into_element(),
            Some(handler) => {
                let handler = handler.clone();
                rect()
                    .width(Size::fill())
                    .content(Content::Fit)
                    .on_secondary_down(move |e: Event<PressEventData>| {
                        e.stop_propagation();
                        handler.call(e);
                    })
                    .child(row)
                    .into_element()
            }
        }
    }
}
