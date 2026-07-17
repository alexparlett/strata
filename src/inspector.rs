//! The column inspector's **selection** — which catalog column is being inspected. Set
//! from the catalog sidebar, read by the inspector panel and the sidebar (tree
//! highlight). Per-window runtime UI state; not persisted. Split out of the old central
//! app state (F7 B9), a small `dioxus-stores` `Store` like [`crate::layout`].
//!
//! A column is identified by its **path**, not its name: `["address", "city"]`. A name
//! alone can't say *which* `city` — the top-level one or the one inside `address` — and
//! the sidebar renders both. A path is a `Vec`, not a dotted `"address.city"`, because
//! column names are whatever the user's files say and may themselves contain dots (the
//! same hazard that makes `ident` mandatory over `col` in [`crate::profile`]).
//!
//! A top-level column is simply a one-segment path.

use dioxus::prelude::*;
use dioxus_stores::*;

/// Per-window column-inspector selection.
#[derive(Store, Clone, PartialEq, Default)]
pub struct Inspector {
    /// The selected catalog column as `(table, path)`, or `None`.
    pub selected: Option<(String, Vec<String>)>,
}

/// This window's inspector selection (per-window, like [`crate::layout`]).
pub static INSPECTOR: GlobalStore<Inspector> = Global::new(|| Inspector::default());

fn store() -> Store<Inspector> {
    INSPECTOR.resolve()
}

/// The selected `(table, path)`, if any (read by the inspector panel + the sidebar).
pub fn selected() -> Option<(String, Vec<String>)> {
    store().selected().cloned()
}

/// Select a catalog column for inspection, by its path within `table`.
pub fn select(table: String, path: Vec<String>) {
    store().selected().set(Some((table, path)));
}
