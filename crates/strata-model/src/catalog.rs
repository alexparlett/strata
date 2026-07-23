//! Catalog references and row descriptors: which [`CatalogKind`] section a row is in,
//! what a pending removal ([`RemoveKind`] / [`RemoveTarget`]) targets, and a [`ColRef`]
//! that names one column. Also the persisted **catalog definitions** ([`TableDef`] /
//! [`ViewDef`] / [`SavedQuery`]) — exactly what `.strata/project.json` stores, nothing
//! more. What registration *learns* about a def (columns, row counts, status, profiles)
//! is runtime state and lives with the UI's project store, wrapped around these — not
//! here as skipped fields.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

// ---------------------------------------------------------------------------
// Catalog definitions (persisted to `.strata/project.json`)
// ---------------------------------------------------------------------------

/// Accept partition columns as either the legacy name-only `["year","month"]`
/// (→ typed `Utf8`) or the current typed `[["year","Int32"], …]` form, so old project
/// files keep loading. Serialization always emits the typed form.
fn de_partition_cols<'de, D>(d: D) -> Result<Vec<(String, String)>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Col {
        Named(String),
        Typed(String, String),
    }
    Ok(Vec::<Col>::deserialize(d)?
        .into_iter()
        .map(|c| match c {
            Col::Named(n) => (n, "Utf8".to_string()),
            Col::Typed(n, t) => (n, t),
        })
        .collect())
}

/// One logical table definition (a DataFusion `ListingTable` over many source paths).
/// `sources` are stored project-relative where they sit inside the project folder.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct TableDef {
    pub name: String,
    pub format: String,
    pub sources: Vec<String>,
    /// Hive partition columns as `(name, arrow_type)` — the persisted source of truth for
    /// deterministic reload (types aren't re-detected).
    #[serde(default, deserialize_with = "de_partition_cols")]
    pub partition_cols: Vec<(String, String)>,
}

/// A saved, query-backed catalog view definition (a real DataFusion `CREATE VIEW`).
/// Views are addressed by `name` — that *is* their engine/SQL identity.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct ViewDef {
    pub name: String,
    pub sql: String,
}

/// A named SQL snippet stored in the project — distinct from a [`ViewDef`] (which is a
/// real DataFusion view). Re-opened in a query tab, not queryable by name — so unlike a
/// view, its `name` is only a label, and identity is the stable `id` (what a tab's
/// save-target origin holds; renaming can't dangle it). Files written before ids get one
/// minted per load; it sticks on the next save.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct SavedQuery {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    pub sql: String,
    pub meta: String,
}
