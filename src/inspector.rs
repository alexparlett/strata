//! The column inspector's **selection** — which catalog column is being inspected. Set
//! from the catalog sidebar, read by the inspector panel and the sidebar (tree
//! highlight). Per-window runtime UI state; not persisted. Split out of the old central
//! app state (F7 B9), a small `dioxus-stores` `Store` like [`crate::layout`].
//!
//! A column is identified by a [`ColRef`]: **what kind of thing owns it, its owner's
//! name, and its path within it**. Each part earns its place:
//!
//! - **kind** — tables and views are separate collections. Without it, resolving a
//!   selection means searching both and hoping the name only lands in one.
//! - **path**, not a name — `["address", "city"]`. A name alone can't say *which*
//!   `city`, the top-level one or the one inside `address`, and the sidebar renders
//!   both. Keying by name meant a nested column resolved to an unrelated top-level one
//!   and showed *its* facts.
//!
//! It's a struct rather than a `"view::orders.address.city"` URN for the same reason
//! the path is a `Vec`: names come from the user's files and may contain dots, `::`, or
//! anything else. A string that has to be parsed back is a bug waiting to be
//! rediscovered (cf. `ident` vs `col` in [`crate::profile`]).

use dioxus::prelude::*;
use dioxus_stores::*;

use crate::state::CatalogKind;

/// A reference to one column in the catalog.
#[derive(Clone, PartialEq, Debug)]
pub struct ColRef {
    /// `Table` or `View` — says which collection owns it, so resolving is one lookup.
    pub kind: CatalogKind,
    /// The owning table or view.
    pub owner: String,
    /// Path within the owner. A top-level column is a one-segment path.
    pub path: Vec<String>,
}

impl ColRef {
    /// A nested *field* — a struct's child. A position, not a type: a top-level column
    /// whose type is a struct is not one.
    pub fn is_child(&self) -> bool {
        self.path.len() > 1
    }

    /// The leaf's own name. The path is how it's found, not what it's called.
    pub fn name(&self) -> &str {
        self.path.last().map(String::as_str).unwrap_or_default()
    }
}

/// Per-window column-inspector selection.
#[derive(Store, Clone, PartialEq, Default)]
pub struct Inspector {
    /// The selected column, or `None`.
    pub selected: Option<ColRef>,
}

/// This window's inspector selection (per-window, like [`crate::layout`]).
pub static INSPECTOR: GlobalStore<Inspector> = Global::new(|| Inspector::default());

fn store() -> Store<Inspector> {
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
