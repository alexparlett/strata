//! **PROVIDER** — the one control that decides which of the rows below exist.

use freya::prelude::*;

use crate::apps::project::contexts::EngineCtx;
use crate::apps::source::SourceCtx;
use crate::components::form::Row;
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};

/// **PROVIDER** — explicit, never inferred from a typed URL scheme (spec §1). The one control
/// that decides which rows exist below it.
///
/// One segment per **registered source**, and nothing else — which is the engine's answer, not
/// this crate's, so a source an embedder registered is offered on exactly the terms a shipped one
/// is: badged in its own word, and carrying its own declaration into the draft. A build serving
/// no source offers nothing, which is the honest form of "this build cannot make a data source".
#[derive(PartialEq)]
pub(super) struct ProviderPicker {
    pub(super) key: DiffKey,
}

impl KeyExt for ProviderPicker {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ProviderPicker {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SourceCtx>();
        let engine = use_consume::<EngineCtx>();
        let kind = ctx.draft.read().kind.clone();

        let mut pill = SegmentedToggle::new().form();
        for source in engine.sources().registrants() {
            let picked = kind == source.kind;
            pill = pill.child(ToggleSegment::text(source.badge).selected(picked).on_press(
                move |_| {
                    let source = source.clone();
                    ctx.edit(move |draft| draft.adopt(&source));
                },
            ));
        }
        Row::new("PROVIDER").child(pill)
    }
}
