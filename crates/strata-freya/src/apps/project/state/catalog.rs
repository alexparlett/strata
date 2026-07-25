//! The catalog's two **context signals**: the **inspected column** the right-hand inspector is
//! describing, and whether a catalog **scan** is in flight.
//!
//! Both are context signals, not Radio stores (state-arch §8, the `LayoutCtx` / `LogCtx` shape):
//! one small value each, written in one place and read in another across the shell. Neither is on
//! [`ProjectState`](super::ProjectState) — that store is the project's durable defs plus what
//! registration learned, and a transient "what am I looking at" pointer would wake every catalog
//! subscriber on each click.
//!
//! The selection's value is a [`ColRef`] — `{ kind, owner, path }` — because a name alone can't say
//! *which* `city`, the top-level one or the one inside `address`, and the sidebar renders both.

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

/// Whether a catalog scan is in flight (P3-03) — the registration pass at project open, or a
/// press of the sidebar's ↻. Set by [`scan_catalog`](super::hooks::scan_catalog), the one routine
/// that runs a pass; read by the sidebar header, which spins its refresh button and disables it
/// for the duration.
///
/// This is about the *act of scanning*, not about any row — every row already carries its own
/// `Reg::Loading`, and a bool that flips twice per pass has no business waking catalog
/// subscribers. The initial pass sets it too, so ↻ can't start a second scan on top of the first.
pub type CatalogScan = State<bool>;

/// Provide this window's scan flag. Called by `use_init_project`, which owns the registration
/// pass that sets it.
pub fn use_init_catalog_scan() -> CatalogScan {
    use_provide_context(|| State::create(false))
}

/// This window's scan flag, from context.
pub fn use_catalog_scan() -> CatalogScan {
    consume_context::<CatalogScan>()
}
