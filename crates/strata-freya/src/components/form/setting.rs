//! The **settings pane**'s row: a sentence-case title, its explanation as inline subtext, and
//! the control (design `Settings.dc.html`). See the module doc for why this is not
//! [`FieldRow`](super::FieldRow).
//!
//! One shape, deliberately: title, then hint, then control — even though the canvas puts the
//! Row-density hint *below* its segmented control and gives the Theme title a wider gap than the
//! numeric ones. Once dividers separate the rows, a description that sometimes precedes and
//! sometimes follows its control leaves a reader unable to tell which setting a line of subtext
//! belongs to. The canvas is inconsistent about it within a single pane; the rows are not.

use freya::prelude::*;

use crate::components::form::{form_theme, CONTROL_GAP, HINT_GAP};
use crate::components::typography::{Caption, Strong};

/// Where a setting's control sits, which is a property of the control and not of the setting:
/// a switch is small enough to read as the row's trailing answer to its title, everything else
/// needs the row's full width and goes underneath.
#[derive(PartialEq)]
enum SettingControl {
    Stacked(Element),
    Switch {
        toggled: bool,
        on_toggle: EventHandler<()>,
    },
}

/// One setting: its title, an optional one-line description, and its control.
#[derive(PartialEq)]
pub struct Setting {
    title: String,
    hint: Option<String>,
    control: SettingControl,
}

impl Setting {
    /// A setting whose control sits under the label block — a segmented toggle, a
    /// [`NumberField`](super::NumberField), the Appearance pane's theme grid.
    pub fn stacked(title: impl Into<String>, control: impl IntoElement) -> Self {
        Self {
            title: title.into(),
            hint: None,
            control: SettingControl::Stacked(control.into_element()),
        }
    }

    /// An on/off setting: a trailing [`Switch`], with the label block as a second trigger for
    /// it so the whole row acts as one control.
    ///
    /// `on_toggle` mirrors `Switch`'s own prop rather than the app's usual
    /// `EventHandler<Event<T>>` shape: the row has two triggers (a press on the label, the
    /// switch itself) and neither carries anything a caller could act on beyond "flip it".
    pub fn switch(
        title: impl Into<String>,
        toggled: bool,
        on_toggle: impl Into<EventHandler<()>>,
    ) -> Self {
        Self {
            title: title.into(),
            hint: None,
            control: SettingControl::Switch {
                toggled,
                on_toggle: on_toggle.into(),
            },
        }
    }

    /// The one-line description under the title.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl Component for Setting {
    fn render(&self) -> impl IntoElement {
        let theme = form_theme();

        // The descriptions are full sentences and the pane is narrow, so the hint wraps rather
        // than truncating — `Caption`'s default single-line cap would eat the end of half of
        // them.
        let label = rect()
            .vertical()
            .child(Strong::new(self.title.clone()).color(theme.title_color))
            .map(self.hint.clone(), |el, hint| {
                el.child(rect().height(Size::px(HINT_GAP))).child(
                    Caption::new(hint)
                        .color(theme.hint_color)
                        .width(Size::fill())
                        .wrap(),
                )
            });

        match &self.control {
            SettingControl::Stacked(control) => rect()
                .width(Size::fill())
                .vertical()
                .child(label.width(Size::fill()))
                .child(rect().height(Size::px(CONTROL_GAP)))
                .child(control.clone())
                .into_element(),
            // The label block is a **sibling** of the switch, never its ancestor: `Switch`'s
            // own `on_press` does not stop propagation, so a pressable ancestor would take the
            // same click and toggle twice — back to where it started.
            SettingControl::Switch { toggled, on_toggle } => rect()
                .width(Size::fill())
                .horizontal()
                // Without `Content::Flex` the label block's `flex(1.)` takes its natural width
                // and pushes the switch off the pane.
                .content(Content::Flex)
                .cross_align(Alignment::Center)
                .spacing(CONTROL_GAP)
                .child(label.width(Size::flex(1.)).on_press({
                    let on_toggle = on_toggle.clone();
                    move |_: Event<PressEventData>| on_toggle.call(())
                }))
                .child(Switch::new().toggled(*toggled).on_toggle({
                    let on_toggle = on_toggle.clone();
                    move |_| on_toggle.call(())
                }))
                .into_element(),
        }
    }
}
