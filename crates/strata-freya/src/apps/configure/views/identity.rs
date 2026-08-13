//! **TABLE NAME** and **FORMAT** — the canvas's first row, two controls side by side.
//!
//! The name is the table's identity (tables and views share one SQL namespace), so it is the
//! one field marked `REQUIRED` alongside the source paths — a [`Row::required`], not a label
//! this window draws for itself.

use freya::prelude::*;

use crate::apps::configure::model::FormatId;
use crate::apps::configure::ConfigureCtx;
use crate::components::form::{Row, ValueField, FIELD_HEIGHT};
use crate::components::metrics::SP_4;
use crate::components::typography::MonoValue;

/// The canvas's `width: 128px` on the format picker, and the gap between the two controls.
///
/// Both are the **layout's**, applied by [`Identity`] to the columns it divides — the two rows
/// inside know nothing about how wide they are. Neither states a *height*: every control in
/// this window stands at the form's own `FIELD_HEIGHT`, so a form never has two field heights
/// in it. (The canvas draws this pair a few pixels taller than the rest; matching that would
/// mean this row diverging from every other form in the app, which is the worse trade.)
const FORMAT_WIDTH: f32 = 128.;
const COLUMN_GAP: f32 = SP_4;

#[derive(PartialEq)]
pub struct Identity;

impl Component for Identity {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConfigureCtx>();
        let internal = use_memo(move || ctx.draft.read().internal());
        let internal = internal();

        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::End)
            .spacing(COLUMN_GAP)
            .child(rect().width(Size::flex(1.)).child(NameField))
            .maybe_child(
                (!internal).then(|| rect().width(Size::px(FORMAT_WIDTH)).child(FormatPicker)),
            )
    }
}

#[derive(PartialEq)]
struct NameField;

impl Component for NameField {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConfigureCtx>();
        let text = use_state({
            let initial = ctx.draft.peek().name.clone();
            move || initial
        });
        use_side_effect(move || {
            let name = text.read().clone();
            ctx.edit(move |draft| draft.name = name);
        });

        Row::new("TABLE NAME").required().child(
            ValueField::new(text)
                .width(Size::fill())
                .placeholder("my_table"),
        )
    }
}

/// The reader picker — four formats, not the canvas's five: there is no Avro in this build.
///
/// A def whose format has no reader ([`FormatId::Unknown`]) shows what it really says and is
/// not in the list, so picking anything is a deliberate change rather than a silent one. Save
/// stays blocked until it happens.
#[derive(PartialEq)]
struct FormatPicker;

impl Component for FormatPicker {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ConfigureCtx>();
        let current = ctx.draft.read().format.clone();
        let options: Vec<Element> = FormatId::OFFERED
            .iter()
            .map(|format| {
                let format = format.clone();
                MenuItem::new()
                    .selected(format == current)
                    .on_press({
                        let format = format.clone();
                        move |_| {
                            let format = format.clone();
                            ctx.edit(move |draft| draft.format = format);
                        }
                    })
                    .child(MonoValue::new(format.label()))
                    .into()
            })
            .collect();

        Row::new("FORMAT").child(
            rect()
                .width(Size::fill())
                .height(Size::px(FIELD_HEIGHT))
                .child(
                    Select::new()
                        .selected_item(MonoValue::new(current.label()))
                        .children(options),
                ),
        )
    }
}
