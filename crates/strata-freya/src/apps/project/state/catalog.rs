//! The **inspected column** — which catalog column the right-hand inspector is describing.
//!
//! A context signal, not a Radio store (state-arch §8, the `LayoutCtx` / `LogCtx` shape): one
//! small value, written by the catalog sidebar and read by the inspector across the shell. It is
//! deliberately *not* on [`ProjectState`](super::ProjectState) — that store is the project's
//! durable defs plus what registration learned, and a transient "what am I looking at" pointer
//! would wake every catalog subscriber on each click.
//!
//! The value is a [`ColRef`] — `{ kind, owner, path }` — because a name alone can't say *which*
//! `city`, the top-level one or the one inside `address`, and the sidebar renders both.

use freya::prelude::{consume_context, use_provide_context, State};
use strata_model::ColRef;

/// The selected column, or `None` when nothing is inspected. `State` is `Copy`, so consumers hold
/// it by value.
pub type CatalogSelection = State<Option<ColRef>>;

/// Provide this window's inspected-column slot. Call once in the window root, above the shell.
pub fn use_init_catalog_selection() -> CatalogSelection {
    use_provide_context(|| State::create(None::<ColRef>))
}

/// This window's inspected-column slot, from context.
pub fn use_catalog_selection() -> CatalogSelection {
    consume_context::<CatalogSelection>()
}
