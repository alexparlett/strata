//! The **import (read) options** block — the format's options, as one flat list.
//!
//! The list is the shared `OptionList` (`components::form::options`); the only thing here is the
//! block's frame.
//!
//! **There is no ADVANCED disclosure**, although this window's canvas draws one. The export
//! window's canvas folded its own away on the grounds that a format's advanced controls are just
//! more of that format's options, and that reasoning does not stop being true here — the split
//! would only be one more thing to open before a CSV's quote character can be reached, in a
//! window whose entire subject is how a file is read. Both windows are now the same shape, which
//! is worth more than either canvas's local choice.
//!
//! **Nothing at all for parquet and Arrow**, which is not an omission: `ArrowFormat` has no
//! options in DataFusion 54, and every `ParquetFormat` knob is an engine-wide setting that
//! already has a control in Settings ▸ Engine. A per-table copy would be a second place to set
//! the same key.

use freya::prelude::*;

use crate::apps::configure::ConfigureCtx;
use crate::components::divider::Divider;
use crate::components::form::OptionList;
use crate::components::typography::Eyebrow;
use crate::components::window::window_theme;

/// The gap under the block's label.
const BLOCK_GAP: f32 = 12.;

#[derive(PartialEq)]
pub struct ImportOptions;

impl Component for ImportOptions {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let form = crate::components::form::form_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let (label, options) = {
            let draft = ctx.draft.read();
            (draft.options_label(), draft.options())
        };

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(BLOCK_GAP)
            // The canvas rules this block off from the paths above it.
            .child(Divider::horizontal().color(win.border_fill))
            .child(Eyebrow::new(label).color(form.label_color))
            .child(OptionList::new(options, move |edit| {
                ctx.edit(|draft| draft.apply(edit))
            }))
    }
}
