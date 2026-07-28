//! The app's **list-toolbar button**: a square, icon-only action over a list you are editing —
//! add a row, remove the selected one, duplicate it, paste, browse.
//!
//! Two surfaces arrived at it independently and built it twice, within days of each other:
//! Settings ▸ Engine's properties toolbar (P4-07) and the Configure window's source-path toolbar
//! (P4-11). Same 28×28 box, same 15px glyph, same convention that the *action* carries the tone —
//! add takes the accent, remove takes the destructive red, the rest recede. That is a component,
//! not a coincidence.
//!
//! **The label is a tooltip, not text.** An icon-only button has no accessible name of its own, so
//! one is required here rather than optional — which is also what stopped the two copies from
//! agreeing (one had tooltips, the other had none).
//!
//! **The variant is the one real difference.** The engine pane's toolbar sits on a panel and its
//! buttons are chrome-less; the Configure window's canvas gives them a border of their own. That
//! is a per-surface choice about the surface, not about the control, so it is a builder rather
//! than an average of the two.

use freya::prelude::*;

use crate::components::icon::{Icon, IconName};

/// The canvas's square (both toolbars draw it at 28) and the glyph inside it.
pub const TOOL_SIZE: f32 = 28.;
const TOOL_ICON: f32 = 15.;

#[derive(PartialEq)]
pub struct ToolButton {
    icon: IconName,
    label: &'static str,
    color: Option<Color>,
    enabled: bool,
    outlined: bool,
    on_press: Option<EventHandler<Event<PressEventData>>>,
}

impl ToolButton {
    /// A tool for `label`, drawn as `icon`. The label names the action for a screen reader and
    /// on hover — see the module doc on why it is required.
    pub fn new(icon: IconName, label: &'static str) -> Self {
        Self {
            icon,
            label,
            color: None,
            enabled: true,
            outlined: false,
            on_press: None,
        }
    }

    /// The action's own tone — the accent for add, the destructive red for remove. Absent means
    /// the glyph inherits, which is what the recessive tools want.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Give the button a border of its own, for a toolbar that does not sit on a panel.
    pub fn outlined(mut self) -> Self {
        self.outlined = true;
        self
    }

    pub fn on_press(mut self, on_press: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(on_press.into());
        self
    }
}

impl Component for ToolButton {
    fn render(&self) -> impl IntoElement {
        let on_press = self.on_press.clone();
        let button = Button::new()
            .maybe(self.outlined, |el| el.outline())
            .maybe(!self.outlined, |el| el.flat())
            .enabled(self.enabled)
            .width(Size::px(TOOL_SIZE))
            .height(Size::px(TOOL_SIZE))
            .map(on_press, |el, on_press| {
                el.on_press(move |e: Event<PressEventData>| on_press.call(e))
            })
            .child(
                Icon::new(self.icon)
                    .size(TOOL_ICON)
                    .map(self.color, |el, color| el.color(color)),
            );

        TooltipContainer::new(Tooltip::new(self.label))
            .position(AttachedPosition::Top)
            .child(button)
    }
}
