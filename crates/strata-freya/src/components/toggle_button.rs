//! The toggle button (design `.icon-btn.plain` + `.on`; theme `toggle_button`): a
//! chrome-less press-to-flip button whose `on` state wears the accent-soft tint — matching
//! the segmented toggle's selected look. First used as the plan view's Raw/Tree switch
//! (P2-05), but any mode toggle wears it. The content is the caller's children (usually an
//! `Icon`), inheriting the dress via the ambient colour; rest and active dress come wholly
//! from the theme file's `components.toggle_button`, and the hover is the comp's soft
//! semantic overlay (the same palette-derived recipe as `ToggleSegment`).

use freya::components::use_theme;
use freya::prelude::*;

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
        active_background: Color,
        active_color: Color,
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
        }
    }

    /// The toggle's state (default: off). Pass a different value to flip it
    /// programmatically — presses report theirs through [`on_change`](Self::on_change).
    pub fn toggle(mut self, on: impl Into<bool>) -> Self {
        self.toggle = on.into();
        self
    }

    /// The tooltip this toggle wears (the comp's `title=`).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Called after every flip with the new state (`ChangeEventData::value`).
    pub fn on_change(
        mut self,
        on_change: impl Into<EventHandler<Event<ChangeEventData>>>,
    ) -> Self {
        self.on_change = Some(on_change.into());
        self
    }
}

impl Component for ToggleButton {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&self.theme, ToggleButtonThemePreference, "toggle_button");
        // The comp's plain-icon-button hover (semantic — read from the palette): a 7%
        // text-colour overlay under a brightened glyph. The `on` dress wins over hover.
        let hover = use_theme().read().colors.text_primary;
        let mut hovered = use_state(|| false);
        let mut on = use_reactive(&self.toggle);

        let (background, color) = if on() {
            (theme.active_background, theme.active_color)
        } else if hovered() {
            (hover.with_a(18), hover)
        } else {
            (theme.background, theme.color)
        };
        let on_change = self.on_change.clone();
        // 28px tall, at least square, hugging wider content — the caller's children inherit
        // the state's colour ambiently (icons via `currentColor`, labels via the parent).
        let button = rect()
            .height(Size::px(28.))
            .min_width(Size::px(28.))
            .padding((0., 6.))
            .corner_radius(8.)
            .center()
            .background(background)
            .color(color)
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set(false))
            .on_press(move |e: Event<PressEventData>| {
                let v = !*on.peek();
                on.set(v);
                if let Some(on_change) = &on_change {
                    on_change.call(e.map(|_| ChangeEventData::new(v)));
                }
            })
            .children(self.elements.clone());
        match &self.title {
            Some(title) => TooltipContainer::new(Tooltip::new(title.clone()))
                .position(AttachedPosition::Bottom)
                .child(button)
                .into_element(),
            None => button.into_element(),
        }
    }
}
