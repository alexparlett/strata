//! The column inspector's **selection** — which catalog column `(table, column)` is
//! being inspected. Set from the catalog sidebar, read by the inspector panel and the
//! sidebar (tree highlight). Per-window runtime UI state; not persisted. Split out of
//! the old central app state (F7 B9), a small `dioxus-stores` `Store` like [`crate::layout`].

use dioxus::prelude::*;
use dioxus_stores::*;

/// Per-window column-inspector selection.
#[derive(Store, Clone, PartialEq, Default)]
pub struct Inspector {
    /// The selected catalog column `(table, column)`, or `None`.
    pub selected: Option<(String, String)>,
}

/// This window's inspector selection (per-window, like [`crate::layout`]).
pub static INSPECTOR: GlobalStore<Inspector> = Global::new(|| Inspector::default());

fn store() -> Store<Inspector> {
    INSPECTOR.resolve()
}

/// The selected `(table, column)`, if any (read by the inspector panel + the sidebar).
pub fn selected() -> Option<(String, String)> {
    store().selected().cloned()
}

/// Select a catalog column for inspection.
pub fn select(table: String, column: String) {
    store().selected().set(Some((table, column)));
}
