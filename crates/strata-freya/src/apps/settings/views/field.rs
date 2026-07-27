//! The **setting row** vocabulary every category pane is built from (design `Settings.dc.html`):
//! a divider-separated list of rows, each a title, a one-line description and its control.
//!
//! One shape, deliberately: title, then hint, then control — even though the canvas puts the
//! Row-density hint *below* its segmented control and gives the Theme title a wider gap than
//! the numeric ones. Once dividers separate the rows, a description that sometimes precedes
//! and sometimes follows its control leaves a reader unable to tell which setting a line of
//! subtext belongs to. The canvas is inconsistent about it within a single pane; the rows are
//! not, so the shape is fixed here and every pane gets it for free.
//!
//! The controls themselves are the app's standard components ([`Switch`], [`Input`],
//! `SegmentedToggle`) — this module is the row shell around them, plus the one control the
//! settings window has that nothing else does: [`NumberField`].

use freya::prelude::*;
use strata_core::config::Settings;

use crate::apps::settings::{SettingsCtx, SettingsThemePartial, SettingsThemePreference};
use crate::components::divider::Divider;
use crate::components::typography::{Body, Caption, InputTypography, Strong};

/// Canvas spacing (`--sp-1` / `--sp-4` / `--sp-6`): under a title, before a control, and
/// either side of the rule between two settings.
const HINT_GAP: f32 = 2.;
const CONTROL_GAP: f32 = 12.;
const SETTING_GAP: f32 = 24.;

/// The canvas's numeric field (`width: 130px`) and the gap to its unit.
const FIELD_WIDTH: f32 = 130.;
const UNIT_GAP: f32 = 8.;

/// A category pane's settings, rule-separated. The pane hands it [`Setting`]s in order and it
/// draws the gaps and the hairlines between them, so no pane spells the rhythm out itself.
#[derive(PartialEq)]
pub struct SettingList {
    children: Vec<Element>,
}

impl Default for SettingList {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingList {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }
}

impl ChildrenExt for SettingList {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Component for SettingList {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<SettingsThemePartial>,
            SettingsThemePreference,
            "settings"
        );

        let mut list = rect().width(Size::fill()).vertical();
        for (i, setting) in self.children.iter().enumerate() {
            if i > 0 {
                list = list
                    .child(rect().height(Size::px(SETTING_GAP)))
                    .child(Divider::horizontal().color(theme.border_fill))
                    .child(rect().height(Size::px(SETTING_GAP)));
            }
            list = list.child(setting.clone());
        }
        list
    }
}

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
    /// [`NumberField`], the Appearance pane's theme grid.
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
        let theme = get_theme!(
            &None::<SettingsThemePartial>,
            SettingsThemePreference,
            "settings"
        );

        // The descriptions are full sentences and the pane is narrow, so the hint wraps rather
        // than truncating — `Caption`'s default single-line cap would eat the end of half of
        // them.
        let label = rect()
            .vertical()
            .child(Strong::new(self.title.clone()))
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

/// A whole-number setting's control: a digits-only input and the unit it is measured in.
///
/// **Every accepted keystroke publishes**, clamped to the field's bounds — not just Enter or
/// leaving the field. Apply is a button press, and `Button` moves focus and calls its handler
/// in the same breath, so a value that waited for the field to be left would never reach the
/// draft the user is about to commit.
///
/// Losing focus is instead when the *text* is normalized: it re-echoes what the setting
/// actually holds, so an emptied or out-of-range field snaps back to the value it published
/// rather than sitting there disagreeing with it.
#[derive(PartialEq)]
pub struct NumberField {
    value: i64,
    min: i64,
    max: Option<i64>,
    unit: &'static str,
    on_change: EventHandler<i64>,
}

impl NumberField {
    /// `value` is what the setting holds now; `on_change` takes each clamped reading of the
    /// field. Bounded below at zero until [`min`](Self::min) says otherwise — the input takes
    /// digits only, so a negative can't be typed in the first place.
    pub fn new(value: i64, unit: &'static str, on_change: impl Into<EventHandler<i64>>) -> Self {
        Self {
            value,
            min: 0,
            max: None,
            unit,
            on_change: on_change.into(),
        }
    }

    pub fn min(mut self, min: i64) -> Self {
        self.min = min;
        self
    }

    pub fn max(mut self, max: i64) -> Self {
        self.max = Some(max);
        self
    }
}

impl Component for NumberField {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<SettingsThemePartial>,
            SettingsThemePreference,
            "settings"
        );
        // The a11y id is ours rather than the `Input`'s own, so this component can watch the
        // field's focus — losing it is the moment the text is normalized (see below).
        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);
        // The committed value as a reactive handle: `use_side_effect` builds its closure once
        // (`use_hook`), so a plainly captured `self.value` would freeze at the first render.
        let value = use_reactive(&self.value);
        let mut text = use_state(move || value.peek().to_string());

        use_side_effect(move || {
            // Only while the field is not being edited — this effect also wakes on every
            // keystroke's publish, and echoing then would overwrite what is being typed.
            if focus() == Focus::Not {
                text.set_if_modified(value().to_string());
            }
        });

        let (min, max) = (self.min, self.max);
        let on_change = self.on_change.clone();
        let input = Input::new(text)
            .a11y_id(a11y_id)
            .width(Size::px(FIELD_WIDTH))
            .on_validate(move |v: InputValidator| {
                // Digits only, so the field can never hold something that isn't a number —
                // Freya undoes a keystroke the validator rejects. Empty stays valid: it is
                // what the field passes through on the way from one value to another.
                let typed = v.text().to_string();
                let ok = typed.chars().all(|c| c.is_ascii_digit());
                v.set_valid(ok);
                if ok {
                    if let Ok(n) = typed.parse::<i64>() {
                        on_change.call(n.max(min).min(max.unwrap_or(i64::MAX)));
                    }
                }
            });

        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(UNIT_GAP)
            .child(InputTypography::mono(input))
            .child(Body::new(self.unit).color(theme.hint_color))
    }
}

/// Edit one field of the draft: takes a write guard out of the shared context and hands it to
/// `edit`.
///
/// `State` is `Copy`, so a local `mut` binding is how a handler reaches the draft at all — and
/// every control on every pane wants the same three lines around its one-line change.
pub fn edit_draft(ctx: SettingsCtx, edit: impl FnOnce(&mut Settings)) {
    let mut draft = ctx.draft;
    edit(&mut draft.write());
}
