//! Catalog references and row descriptors: which [`CatalogKind`] section a row is in,
//! what a pending removal ([`RemoveKind`] / [`RemoveTarget`]) targets, and a [`ColRef`]
//! that names one column.

/// What a pending removal targets — drives the confirm dialog's wording and the
/// engine command sent on confirm.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RemoveKind {
    Table,
    View,
}

#[derive(Clone)]
pub struct RemoveTarget {
    pub kind: RemoveKind,
    pub name: String,
}

/// Which catalog section a right-clicked row belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CatalogKind {
    Table,
    View,
    Query,
}

/// A reference to one column in the catalog — **what kind of thing owns it, its owner's
/// name, and its path within it**. Each part earns its place:
///
/// - **kind** — tables and views are separate collections. Without it, resolving a
///   reference means searching both and hoping the name only lands in one.
/// - **path**, not a name — `["address", "city"]`. A name alone can't say *which* `city`,
///   the top-level one or the one inside `address`, and the sidebar renders both. Keying
///   by name meant a nested column resolved to an unrelated top-level one.
///
/// A struct rather than a `"view::orders.address.city"` URN for the same reason the path
/// is a `Vec`: names come from the user's files and may contain dots, `::`, or anything
/// else. A string that has to be parsed back is a bug waiting to be rediscovered (cf.
/// `ident` vs `col` in [`crate::profile`]).
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
