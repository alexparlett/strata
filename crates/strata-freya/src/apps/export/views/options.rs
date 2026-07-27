//! The option list — **one component for every option**, in any format.
//!
//! This is D6's actual ask: the Dioxus modal reached the same screen through hardcoded `match`
//! arms per format, so adding an option meant editing a component. Here
//! [`ExportDraft::groups`] hands over a `Vec<Group>` and this renders whatever it is given —
//! label, optional hint, control — so a new option is a row in a table.
//!
//! The list is **flat**: there is no ADVANCED disclosure. The canvas folded it away on the
//! grounds that a format's advanced controls are just more of that format's options.
//!
//! Every control writes through the [`Edit`] it was built holding, so nothing here knows which
//! field it is editing — which is why no control can write the wrong one.
//!
//! **Each control shape is its own `Component`**, not a helper fn. The group list changes
//! length with the format (and the Parquet level group comes and goes with the codec), so
//! rendering the stateful ones inline would call a *variable* number of hooks per render and
//! corrupt hook order. A component per shape gives each its own scope, which is also what lets
//! a text field keep an edit buffer at all.
//!
//! [`ExportDraft::groups`]: crate::apps::export::ExportDraft::groups

use freya::prelude::*;

use crate::apps::export::{
    Choice, Control, Edit, ExportCtx, ExportThemePartial, ExportThemePreference, Group, Make,
    TextField,
};
use crate::components::icon::{Icon, IconName};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::typography::{Eyebrow, InputTypography, MonoValue, Prose};

/// Field boxes, from the canvas: a one-character field, a short text field, a number, the
/// custom box beside a segmented control, and a select (the one control the canvas draws 32
/// tall rather than 30).
const CHAR_WIDTH: f32 = 48.;
const TEXT_WIDTH: f32 = 120.;
const NUM_WIDTH: f32 = 72.;
const CUSTOM_WIDTH: f32 = 62.;
const SELECT_WIDTH: f32 = 180.;
const SELECT_HEIGHT: f32 = 32.;
const FIELD_HEIGHT: f32 = 30.;

/// Write one control's edit into the draft.
///
/// Takes the context by value — the caller consumed it during its own render, so this is safe
/// to call from an event handler, where there is no scope to read one from.
fn apply(ctx: ExportCtx, edit: Edit) {
    ctx.edit(|draft| draft.apply(edit));
}

#[derive(PartialEq)]
pub struct Options;

impl Component for Options {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ExportCtx>();
        // Both reads subscribe: a format switch or any edit rebuilds the list, which is the
        // point — the level group appears and disappears with the codec.
        let groups = ctx.draft.read().groups(&ctx.target.read());

        let mut list = rect().width(Size::fill()).vertical().spacing(20.);
        for group in groups {
            let key = group.label.clone();
            list = list.child(
                OptionGroup {
                    group,
                    key: DiffKey::None,
                }
                .key(key),
            );
        }
        list
    }
}

/// One labelled group: the uppercase label (+ its hint), then the control under it.
#[derive(PartialEq)]
struct OptionGroup {
    group: Group,
    key: DiffKey,
}

impl KeyExt for OptionGroup {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for OptionGroup {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");

        // The label row: the eyebrow, and the ⓘ carrying the hint as a tooltip — the canvas
        // swept every inline grey explainer into a hover tip.
        let header = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.)
            .child(Eyebrow::new(self.group.label.clone()).color(theme.label_color))
            .maybe_child(self.group.hint.map(|hint| {
                TooltipContainer::new(Tooltip::new(hint))
                    .position(AttachedPosition::Top)
                    .child(Icon::new(IconName::Info).size(12.).color(theme.hint_color))
            }));

        // One component per shape — see the module doc on why this isn't a helper fn.
        let control: Element = match self.group.control.clone() {
            Control::Seg { options, custom } => SegControl { options, custom }.into(),
            Control::Toggle { on, edit } => ToggleControl { on, edit }.into(),
            Control::Text(field) => FieldControl {
                field,
                width: TEXT_WIDTH,
                height: FIELD_HEIGHT,
                align: TextAlign::Left,
            }
            .into(),
            Control::Char(field) => FieldControl {
                field,
                width: CHAR_WIDTH,
                height: FIELD_HEIGHT,
                align: TextAlign::Center,
            }
            .into(),
            Control::Num {
                value,
                min,
                max,
                make,
            } => NumControl {
                value,
                min,
                max,
                make,
            }
            .into(),
            Control::Select { options } => SelectControl { options }.into(),
            Control::Note(text) => NoteControl { text }.into(),
        };

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(8.)
            .child(header)
            .child(control)
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// A pill of mutually exclusive values, with the custom field beside it when one is offered.
#[derive(PartialEq)]
struct SegControl {
    options: Vec<Choice>,
    custom: Option<TextField>,
}

impl Component for SegControl {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ExportCtx>();

        // The canvas's form control, not the compact toolbar one: roomier segments, gaps
        // instead of dividers, on the recessed surface.
        let mut pill = SegmentedToggle::new().form();
        for choice in &self.options {
            let edit = choice.edit.clone();
            pill = pill.child(
                ToggleSegment::text(choice.label.clone())
                    .selected(choice.selected)
                    .on_press(move |_| apply(ctx, edit.clone())),
            );
        }

        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.)
            .child(pill)
            // The box beside a segmented control is built to the **pill's** height, not the
            // 30px every other field uses: they sit side by side in one row, so a box that is
            // nine pixels short of its neighbour reads as a mistake whatever the canvas says.
            // Narrow and centred with it — it holds a token like `\N`, not a sentence.
            .maybe_child(self.custom.clone().map(|field| FieldControl {
                field,
                width: CUSTOM_WIDTH,
                height: SegmentedToggle::FORM_HEIGHT,
                align: TextAlign::Center,
            }))
    }
}

/// A switch — Freya's own, not a hand-rolled track and knob.
#[derive(PartialEq)]
struct ToggleControl {
    on: bool,
    edit: Edit,
}

impl Component for ToggleControl {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ExportCtx>();
        let edit = self.edit.clone();
        Switch::new()
            .toggled(self.on)
            .on_toggle(move |_| apply(ctx, edit.clone()))
    }
}

/// A free-text field (the delimiter, a quote character, the custom null text).
///
/// It keeps its own edit buffer rather than binding the draft directly: the draft holds the
/// *resolved* intent and this holds what is being typed, so a half-typed `\t` isn't repeatedly
/// re-resolved under the cursor.
#[derive(PartialEq)]
struct FieldControl {
    field: TextField,
    width: f32,
    /// Normally [`FIELD_HEIGHT`]; the box beside a segmented control matches that pill instead.
    height: f32,
    /// The canvas centres the one- and few-character boxes and left-aligns the wider ones.
    align: TextAlign,
}

impl Component for FieldControl {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ExportCtx>();
        // `Input` writes its bound state directly (there is no on-change prop), so the buffer
        // is the input's and this effect carries it into the draft. No sync-back effect is
        // needed: the group list is keyed by label, so a format switch unmounts these controls
        // outright and the next mount re-seeds from the draft.
        let text = use_state({
            let initial = self.field.value.clone();
            move || initial
        });

        let make = self.field.make;
        let max_len = self.field.max_len;
        use_side_effect(move || {
            // The canvas's `maxlength`, enforced on the **box** and not just on the way out:
            // truncating only the draft would show "ab" in a one-character field and quote with
            // "a", which is a control disagreeing with the file it produces.
            let raw = text.read().clone();
            let capped: String = raw.chars().take(max_len).collect();
            if capped != raw {
                let mut text = text;
                text.set(capped.clone());
            }
            // Applied unconditionally: `ExportCtx::edit` is idempotent, so a no-op edit costs
            // nothing — and comparing here against a captured value is precisely the bug this
            // replaces (`use_side_effect` builds its closure once, so the capture froze at the
            // first render and typing a field back to its original value wrote nothing).
            apply(ctx, make.edit(capped));
        });

        rect()
            .width(Size::px(self.width))
            .height(Size::px(self.height))
            .child(
                InputTypography::mono(
                    Input::new(text)
                        .placeholder(self.field.placeholder)
                        .width(Size::fill())
                        .text_align(self.align)
                        .compact(),
                )
                .width(Size::fill()),
            )
    }
}

/// A bounded number (the Parquet compression level).
#[derive(PartialEq)]
struct NumControl {
    value: u32,
    min: u32,
    max: u32,
    make: Make<u32>,
}

impl Component for NumControl {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ExportCtx>();
        let value = self.value;
        let text = use_state(move || value.to_string());

        let (make, min, max) = (self.make, self.min, self.max);
        use_side_effect(move || {
            let typed = text.read().trim().to_string();
            // Clamp rather than accept: the engine clamps too, and a control that disagrees
            // with the file it produces is worse than one that corrects itself. Applied
            // unconditionally — `ExportCtx::edit` is idempotent, and the comparison this
            // replaces was against a value frozen at the first render, so setting the level
            // back to its starting number wrote nothing.
            //
            // An empty or half-typed box is left alone: the draft keeps the last good value
            // until something parseable arrives.
            if let Ok(parsed) = typed.parse::<u32>() {
                apply(ctx, make.edit(parsed.clamp(min, max)));
            }
        });

        rect()
            .width(Size::px(NUM_WIDTH))
            .height(Size::px(FIELD_HEIGHT))
            .child(
                InputTypography::mono(Input::new(text).width(Size::fill()).compact())
                    .width(Size::fill()),
            )
    }
}

/// A dropdown — the app-standard `Select`, never a hand-rolled lookalike.
#[derive(PartialEq)]
struct SelectControl {
    options: Vec<Choice>,
}

impl Component for SelectControl {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ExportCtx>();
        let current = self
            .options
            .iter()
            .find(|c| c.selected)
            .map(|c| c.label.clone())
            .unwrap_or_default();

        rect()
            .width(Size::px(SELECT_WIDTH))
            .height(Size::px(SELECT_HEIGHT))
            .child(
                Select::new()
                    .selected_item(MonoValue::new(current))
                    .children(
                        self.options
                            .iter()
                            .map(|choice| {
                                let edit = choice.edit.clone();
                                MenuItem::new()
                                    .selected(choice.selected)
                                    .on_press(move |_| apply(ctx, edit.clone()))
                                    .child(MonoValue::new(choice.label.clone()))
                                    .into()
                            })
                            .collect::<Vec<Element>>(),
                    ),
            )
    }
}

/// A statement rather than a control — the Arrow explainer. Not an empty list: silence would
/// read as "options are still loading".
#[derive(PartialEq)]
struct NoteControl {
    text: &'static str,
}

impl Component for NoteControl {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        rect()
            .width(Size::fill())
            .padding((12., 12.))
            .corner_radius(6.)
            .background(theme.panel_background)
            .border(Border::new().width(1.).fill(theme.border_fill))
            .child(Prose::new(self.text).color(theme.label_color).wrap())
    }
}
