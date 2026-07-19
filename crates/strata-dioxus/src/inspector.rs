//! The column inspector's **selection** — which catalog column is being inspected. Set
//! from the catalog sidebar, read by the inspector panel and the sidebar (tree
//! highlight). Per-window runtime UI state; not persisted. Split out of the old central
//! app state (F7 B9), a small `dioxus-stores` `Store` like [`crate::layout`].
//!
//! A column is named by a [`crate::model::ColRef`] (kind + owner + path); this module is
//! just the store that remembers which one is selected.
//!
//! **Read rule:** call sites read via the `pub fn` accessors below (`inspector::field()`),
//! never inline `store().field()` — accessors return owned values (no temporary-value
//! dance, no stray non-subscribing `.peek()` in render); binding `store()` is for writes /
//! module-internal use only.

use dioxus::prelude::*;
use dioxus_stores::*;

use crate::model::ColRef;

/// Per-window column-inspector selection.
#[derive(Store, Clone, PartialEq, Default)]
pub struct Inspector {
    /// The selected column, or `None`.
    pub selected: Option<ColRef>,
}

/// This window's inspector selection (per-window, like [`crate::layout`]).
pub static INSPECTOR: GlobalStore<Inspector> = Global::new(|| Inspector::default());

pub fn store() -> Store<Inspector> {
    INSPECTOR.resolve()
}

/// The selected column, if any (read by the inspector panel + the sidebar).
pub fn selected() -> Option<ColRef> {
    store().selected().cloned()
}

/// Select a catalog column for inspection.
pub fn select(col: ColRef) {
    store().selected().set(Some(col));
}
