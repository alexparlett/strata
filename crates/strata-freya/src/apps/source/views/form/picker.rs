//! **PROVIDER** — the one control that decides which of the rows below exist.

use freya::prelude::*;

use crate::apps::project::contexts::EngineCtx;
use crate::apps::source::SourceCtx;
use crate::components::form::Row;
use crate::components::typography::Control;

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

        let sources = engine.sources().registrants();
        let label = sources
            .iter()
            .find(|source| source.kind == kind)
            .map_or("Choose provider", |source| source.label);
        let mut picker = Select::new()
            .width(Size::Inner)
            .selected_item(Control::new(label));
        for source in sources {
            let picked = kind == source.kind;
            let label = source.label;
            picker = picker.child(
                MenuItem::new()
                    .selected(picked)
                    .on_press(move |_| {
                        let source = source.clone();
                        ctx.edit(move |draft| draft.adopt(&source));
                    })
                    .child(Control::new(label)),
            );
        }
        Row::new("Provider").child(picker)
    }
}
