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

use crate::apps::export::{Choice, Control, Edit, ExportCtx, Group, Make, TextField};

use crate::components::form::{
    FieldNote, FieldRow, FormList, NumberField, ValueField, FIELD_HEIGHT,
};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::typography::MonoValue;

/// Field boxes, from the canvas: a one-character field, a short text field, a number, the
/// custom box beside a segmented control, and a select (the one control the canvas draws 32
/// tall rather than 30).
const CHAR_WIDTH: f32 = 48.;
const TEXT_WIDTH: f32 = 120.;
const NUM_WIDTH: f32 = 72.;
const CUSTOM_WIDTH: f32 = 62.;
const SELECT_WIDTH: f32 = 180.;
const SELECT_HEIGHT: f32 = 32.;

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

        // The shared form list, so the rhythm between rows is the app's and not this
        // window's. Spaced rather than divided — the Settings panes are the divided one.
        let mut list = FormList::new();
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
            Control::Note(text) => FieldNote::new(text).into(),
        };

        // The label, its hint and the gap under them are the shared form row's — this window
        // contributes only which control goes in it.
        FieldRow::new(self.group.label.clone())
            .map(self.group.hint, |row, hint| row.hint(hint))
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
            // The box beside a segmented control is built to the **buttons'** height, not the
            // 30px every other field uses: they sit side by side in one row, so a box that is
            // short of its neighbours reads as a mistake whatever the canvas says.
            // Narrow and centred with them — it holds a token like `\N`, not a sentence.
            .maybe_child(self.custom.clone().map(|field| FieldControl {
                field,
                width: CUSTOM_WIDTH,
                height: SegmentedToggle::FORM_SEGMENT_HEIGHT,
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
/// The box itself is the shared [`ValueField`] — its height, its length cap and its mono dress
/// are the app's, not this window's. What is left here is the only export-specific part: the
/// edit buffer, and carrying what is typed into the draft.
#[derive(PartialEq)]
struct FieldControl {
    field: TextField,
    width: f32,
    /// Normally [`FIELD_HEIGHT`]; the box beside a segmented control matches those buttons.
    height: f32,
    /// The canvas centres the one- and few-character boxes and left-aligns the wider ones.
    align: TextAlign,
}

impl Component for FieldControl {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ExportCtx>();
        // `Input` writes its bound state directly (there is no on-change prop), so the buffer
        // is the field's and this effect carries it into the draft. No sync-back effect is
        // needed: the group list is keyed by label, so a format switch unmounts these controls
        // outright and the next mount re-seeds from the draft.
        let text = use_state({
            let initial = self.field.value.clone();
            move || initial
        });

        let make = self.field.make;
        use_side_effect(move || {
            // Applied unconditionally: `ExportCtx::edit` is idempotent, so a no-op edit costs
            // nothing — and comparing here against a captured value is precisely the bug this
            // replaces (`use_side_effect` builds its closure once, so the capture froze at the
            // first render and typing a field back to its original value wrote nothing).
            // `ValueField` has already trimmed the state to `max_len`, so this reads what the
            // box shows.
            apply(ctx, make.edit(text.read().clone()));
        });

        ValueField::new(text)
            .width(Size::px(self.width))
            .height(Size::px(self.height))
            .max_len(self.field.max_len)
            .align(self.align)
            .placeholder(self.field.placeholder)
    }
}

/// A bounded number (the Parquet compression level) — the shared [`NumberField`], bound to the
/// draft. The parse, the clamp and the buffer are the component's; the only thing here is what
/// a new value *means*.
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
        let make = self.make;
        NumberField::new(self.value, self.min, self.max)
            .width(Size::px(NUM_WIDTH))
            .on_change(move |value: u32| apply(ctx, make.edit(value)))
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
