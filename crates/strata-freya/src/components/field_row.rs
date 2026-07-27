//! One labelled row of a form: an uppercase label, an optional hover hint, and the control
//! beneath them.
//!
//! The canvas draws every settings-style surface this way — the export window's options, the
//! config modal's fields, the Settings panes — so the label's type, the gap under it and the
//! hint's ⓘ affordance belong in one place rather than being re-typed per surface.
//!
//! **The hint is a tooltip, not a line of grey text.** That was a deliberate design pass (the
//! canvas swept every inline explainer into a hover tip), and it is the sort of thing that
//! silently reverts the first time someone adds a field by copying a nearby one — which is the
//! argument for this being a component rather than a convention.
//!
//! It carries the label and the hint only. What the row *controls* is the caller's child, so a
//! row wraps a [`ValueField`](crate::components::value_field::ValueField), a `Switch`, a
//! `Select` or anything else without this knowing which.

use freya::components::use_theme;
use freya::prelude::*;

use crate::components::icon::{Icon, IconName};
use crate::components::typography::Eyebrow;

define_theme!(
    %[component]
    pub FieldRow {
        %[fields]
        /// The row's uppercase label.
        label_color: Color,
        /// The ⓘ that carries the hint.
        hint_color: Color,
    }
);

/// The gap between a row's label and its control (canvas `var(--sp-3)`), and between the label
/// and its hint glyph.
const LABEL_GAP: f32 = 8.;
const HINT_SIZE: f32 = 12.;

#[derive(PartialEq)]
pub struct FieldRow {
    label: String,
    hint: Option<String>,
    children: Vec<Element>,
    theme: Option<FieldRowThemePartial>,
}

impl FieldRow {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            hint: None,
            children: Vec::new(),
            theme: None,
        }
    }

    /// The explanation this row's ⓘ carries. Absent = no glyph, rather than an empty tooltip.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn theme(mut self, theme: FieldRowThemePartial) -> Self {
        self.theme = Some(theme);
        self
    }
}

impl ChildrenExt for FieldRow {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Component for FieldRow {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&self.theme, FieldRowThemePreference, "field_row");

        let header = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(LABEL_GAP)
            .child(Eyebrow::new(self.label.clone()).color(theme.label_color))
            .maybe_child(self.hint.clone().map(|hint| {
                TooltipContainer::new(Tooltip::new(hint))
                    .position(AttachedPosition::Top)
                    .child(
                        Icon::new(IconName::Info)
                            .size(HINT_SIZE)
                            .color(theme.hint_color),
                    )
            }));

        let mut row = rect()
            .width(Size::fill())
            .vertical()
            .spacing(LABEL_GAP)
            .child(header);
        for child in &self.children {
            row = row.child(child.clone());
        }
        row
    }
}

/// A form's explanatory block — a statement where a control would go.
///
/// Not a disabled control and not a hint: it is what a row says when there is nothing to set
/// (the export window's Arrow format, which has no write options at all). An empty row would
/// read as "still loading".
#[derive(PartialEq)]
pub struct FieldNote {
    text: String,
}

impl FieldNote {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl Component for FieldNote {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<FieldRowThemePartial>,
            FieldRowThemePreference,
            "field_row"
        );
        // Semantic surfaces, not row tokens: a note sits on the same inset panel every other
        // boxed thing in a form does, and that is the sheet's, so it stays right on a surface
        // this component has never seen.
        let (background, border) = {
            let base = use_theme();
            let base = base.read();
            (base.colors().surface_primary, base.colors().border)
        };
        rect()
            .width(Size::fill())
            .padding((12., 12.))
            .corner_radius(6.)
            .background(background)
            .border(Border::new().width(1.).fill(border))
            .child(
                crate::components::typography::Prose::new(self.text.clone())
                    .color(theme.label_color)
                    .wrap(),
            )
    }
}
