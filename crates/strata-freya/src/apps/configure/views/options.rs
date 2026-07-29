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
        let (scope, label, options) = {
            let draft = ctx.draft.read();
            (draft.format.label(), draft.options_label(), draft.options())
        };
        let has_options = !options.is_empty();

        // **One `rect()` whatever the format**, with the block as optional children rather than
        // an early return of a different node. A section that comes and goes is a child; a
        // render that returns a different *kind* of node for the same position is what crashed
        // this window once already (`Hive`, and Freya's differ unwrapping a missing scope).
        rect()
            .width(Size::fill())
            .vertical()
            .spacing(BLOCK_GAP)
            // The canvas rules this block off from the paths above it.
            .maybe_child(has_options.then(|| Divider::horizontal().color(win.border_fill)))
            .maybe_child(has_options.then(|| Eyebrow::new(label).color(form.label_color)))
            .maybe_child(has_options.then(|| {
                OptionList::new(scope, options, move |edit| {
                    ctx.edit(|draft| draft.apply(edit))
                })
            }))
    }
}

#[cfg(test)]
mod tests {
    use freya::prelude::*;
    use freya_testing::TestingRunner;

    use super::ImportOptions;
    use crate::apps::configure::model::FormatId;
    use crate::apps::configure::{ConfigureCtx, ConfigureDraft, ConfigureTarget, Status};
    use crate::theme::strata_theme;

    /// **CSV → JSON → CSV**, the switch that crashed the window in Freya's differ. The two
    /// option lists share keys in different positions and mix control *shapes* (a toggle and a
    /// text box where the other has a select), which is the part a synthetic list of identical
    /// rows never reproduces.
    #[test]
    fn switching_format_back_and_forth_does_not_break_the_tree() {
        let (mut runner, ctx) = TestingRunner::new(
            || {
                use_init_theme(|| strata_theme(&strata_core::theme::load("midnight")));
                let _ = use_consume::<ConfigureCtx>();
                ImportOptions
            },
            (600., 900.).into(),
            |r| {
                r.provide_root_context(|| ConfigureCtx {
                    draft: State::create(ConfigureDraft {
                        format: FormatId::Csv,
                        name: "t".into(),
                        sources: vec!["/data".into()],
                        ..Default::default()
                    }),
                    target: State::create(ConfigureTarget::New),
                    status: State::create(Status::Idle),
                })
            },
            1.,
        );

        for format in [
            FormatId::Csv,
            FormatId::Json,
            FormatId::Csv,
            FormatId::Parquet,
        ] {
            let mut draft = ctx.draft;
            let mut next = draft.peek().clone();
            next.format = format;
            draft.set(next);
            runner.render();
            runner.render();
        }
    }
}
