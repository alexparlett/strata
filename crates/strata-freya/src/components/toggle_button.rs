//! The toggle button (design `.icon-btn.plain` + `.on`; theme `toggle_button`): a
//! chrome-less press-to-flip button whose `on` state wears the accent-soft tint — matching
//! the segmented toggle's selected look. First used as the plan view's Raw/Tree switch
//! (P2-05), but any mode toggle wears it. The content is the caller's children (usually an
//! `Icon`), inheriting the dress via the ambient colour; rest, hover, active and focus-ring
//! dress are all `toggle_button` rows of the mapping table (`theme/components.rs`).

use freya::prelude::*;

use crate::components::metrics::{R_2, SP_3};

/// Data of a Change event — a stateful control reporting the value it just changed to.
/// App-defined: `Event<D>` is generic, so the toggle maps its press event into this with
/// `Event::map` (propagation and default travel with it) — no fork vocabulary needed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChangeEventData {
    pub value: bool,
}

impl ChangeEventData {
    pub fn new(value: bool) -> Self {
        Self { value }
    }
}

define_theme!(
    %[component]
    pub ToggleButton {
        %[fields]
        background: Color,
        color: Color,
        /// The comp's plain-icon-button hover: a soft text-colour wash under a brightened
        /// glyph. A field rather than an alpha computed off a role, so a theme can retune the
        /// wash without every icon button agreeing on the same 7%.
        hover_background: Color,
        hover_color: Color,
        active_background: Color,
        active_color: Color,
        /// The keyboard focus ring. Pointer focus paints nothing, so the ring is the answer to
        /// "where is the keyboard" and not a second press affordance.
        focus_border_fill: Color,
    }
);

/// The state is the [`toggle`] prop, bridged with `use_reactive` (the fork `Button`'s
/// `enabled` recipe): passing a different value programmatically flips it, a press flips it
/// optimistically and reports the new value through [`on_change`] as an
/// `Event<ChangeEventData>` (mapped from the originating press, so propagation travels
/// with it) — the caller echoes it back via `toggle` and never computes the flip itself.
///
/// [`toggle`]: ToggleButton::toggle
/// [`on_change`]: ToggleButton::on_change
#[derive(PartialEq)]
pub struct ToggleButton {
    elements: Vec<Element>,
    toggle: bool,
    title: Option<String>,
    on_change: Option<EventHandler<Event<ChangeEventData>>>,
    theme: Option<ToggleButtonThemePartial>,
    width: Option<Size>,
    height: Option<Size>,
}

impl Default for ToggleButton {
    fn default() -> Self {
        Self::new()
    }
}

impl ChildrenExt for ToggleButton {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.elements
    }
}

impl ToggleButton {
    pub fn new() -> Self {
        Self {
            elements: Vec::new(),
            toggle: false,
            title: None,
            on_change: None,
            theme: None,
            width: None,
            height: None,
        }
    }

    /// The toggle's state (default: off). Pass a different value to flip it
    /// programmatically — presses report theirs through [`on_change`](Self::on_change).
    pub fn toggle(mut self, on: impl Into<bool>) -> Self {
        self.toggle = on.into();
        self
    }

    /// Fix the toggle's width (default: hug the content, at least square). Like the Freya
    /// `Button`'s `.width`, so a larger control (e.g. the activity rail's 40×38 buttons) sizes
    /// itself rather than relying on the 28px default.
    pub fn width(mut self, width: impl Into<Size>) -> Self {
        self.width = Some(width.into());
        self
    }

    /// Fix the toggle's height (default: 28px).
    pub fn height(mut self, height: impl Into<Size>) -> Self {
        self.height = Some(height.into());
        self
    }

    /// The tooltip this toggle wears (the comp's `title=`).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Called after every flip with the new state (`ChangeEventData::value`).
    pub fn on_change(mut self, on_change: impl Into<EventHandler<Event<ChangeEventData>>>) -> Self {
        self.on_change = Some(on_change.into());
        self
    }
}

impl Component for ToggleButton {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&self.theme, ToggleButtonThemePreference, "toggle_button");
        let mut hovered = use_state(|| false);
        let mut on = use_reactive(&self.toggle);
        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);

        let (background, color) = if on() {
            (theme.active_background, theme.active_color)
        } else if hovered() {
            (theme.hover_background, theme.hover_color)
        } else {
            (theme.background, theme.color)
        };
        let focus_ring = (focus() == Focus::Keyboard).then(|| {
            Border::new()
                .fill(theme.focus_border_fill)
                .width(2.)
                .alignment(BorderAlignment::Inner)
        });
        let on_change = self.on_change.clone();
        let button = rect()
            .height(self.height.clone().unwrap_or(Size::px(28.)))
            .corner_radius(R_2)
            .center()
            .background(background)
            .color(color)
            .border(focus_ring)
            .a11y_id(a11y_id)
            .a11y_focusable(true)
            .a11y_role(AccessibilityRole::Button)
            .map(self.title.clone(), AccessibilityExt::a11y_alt)
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set(false))
            .on_press(move |e: Event<PressEventData>| {
                a11y_id.request_focus();
                let v = !*on.peek();
                on.set(v);
                if let Some(on_change) = &on_change {
                    on_change.call(e.map(|_| ChangeEventData::new(v)));
                }
            })
            .map(self.width.clone(), ContainerSizeExt::width)
            .maybe(self.width.is_none(), |el| {
                el.min_width(Size::px(28.)).padding((0., SP_3))
            })
            .children(self.elements.clone());
        match &self.title {
            Some(title) => TooltipContainer::new(Tooltip::new_text(title.clone()))
                .position(AttachedPosition::Bottom)
                .child(button)
                .into_element(),
            None => button.into_element(),
        }
    }
}
