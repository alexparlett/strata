//! The **import (read) options** block — the format's core groups, then a collapsible
//! **ADVANCED**.
//!
//! Both lists are the shared `OptionList` (`components::form::options`); the only thing here is
//! the block's frame and the disclosure. The disclosure belongs to *this* window rather than to
//! the shared list because the export canvas deliberately folded its own away — a format's
//! advanced controls being, there, just more of that format's options.
//!
//! **Nothing at all for parquet and Arrow**, which is not an omission: `ArrowFormat` has no
//! options in DataFusion 54, and every `ParquetFormat` knob is an engine-wide setting that
//! already has a control in Settings ▸ Engine. A per-table copy would be a second place to set
//! the same key.

use freya::prelude::*;

use crate::apps::configure::ConfigureCtx;
use crate::components::divider::Divider;
use crate::components::form::OptionList;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Eyebrow;

/// The gap under the block's label, and between its two lists.
const BLOCK_GAP: f32 = 12.;
const SECTION_GAP: f32 = 20.;
/// The disclosure's chevron and the gap beside it.
const CHEVRON_SIZE: f32 = 11.;
const CHEVRON_GAP: f32 = 8.;

#[derive(PartialEq)]
pub struct ImportOptions;

impl Component for ImportOptions {
    fn render(&self) -> impl IntoElement {
        let colors = use_theme().read().colors().clone();
        let form = crate::components::form::form_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let (has_options, label, core, advanced) = {
            let draft = ctx.draft.read();
            (
                draft.has_options(),
                draft.options_label(),
                draft.core(),
                draft.advanced(),
            )
        };
        if !has_options {
            return rect();
        }

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(BLOCK_GAP)
            // The canvas rules this block off from the paths above it.
            .child(Divider::horizontal().color(colors.border))
            .child(Eyebrow::new(label).color(form.label_color))
            .child(OptionList::new(core, move |edit| {
                ctx.edit(|draft| draft.apply(edit))
            }))
            .child(Advanced { groups: advanced })
    }
}

/// The ADVANCED disclosure: a pressable header, and the rest of the format's options under it.
#[derive(PartialEq)]
struct Advanced {
    groups: Vec<crate::components::form::Group<crate::apps::configure::Edit>>,
}

impl Component for Advanced {
    fn render(&self) -> impl IntoElement {
        let form = crate::components::form::form_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let mut open = ctx.advanced_open;
        // Subscribes — the whole point of the row is that pressing it changes what is below.
        let is_open = *open.read();

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(BLOCK_GAP)
            .child(
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(CHEVRON_GAP)
                    .a11y_role(AccessibilityRole::Button)
                    .a11y_alt("Advanced options")
                    .on_press(move |_| {
                        let next = !*open.peek();
                        open.set(next);
                    })
                    .child(
                        Icon::new(match is_open {
                            true => IconName::ChevronDown,
                            false => IconName::ChevronRight,
                        })
                        .size(CHEVRON_SIZE)
                        .color(form.label_color),
                    )
                    .child(Eyebrow::new("ADVANCED").color(form.label_color)),
            )
            .maybe_child(is_open.then(|| {
                rect()
                    .width(Size::fill())
                    .padding(Gaps::new(SECTION_GAP - BLOCK_GAP, 0., 0., 0.))
                    .child(OptionList::new(self.groups.clone(), move |edit| {
                        ctx.edit(|draft| draft.apply(edit))
                    }))
            }))
    }
}
