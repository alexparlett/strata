//! Catalog references and row descriptors: which [`CatalogKind`] section a row is in,
//! what a pending removal ([`RemoveKind`] / [`RemoveTarget`]) targets, and a [`ColRef`]
//! that names one column. Also the persisted **catalog definitions** ([`CatalogTable`] /
//! [`CatalogView`]) — durable in `.strata/project.json`, with runtime-only fields
//! (`columns`/`status`/`profile`/…) `#[serde(skip)]`-ped and re-derived on registration.

use serde::{Deserialize, Serialize};

use crate::{CatalogProfile, ColumnInfo};

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

/// Registration lifecycle of a catalog table (runtime, not persisted).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum RegStatus {
    /// A freshly-loaded or -added table, awaiting engine registration.
    #[default]
    Loading,
    Ready,
    Failed,
}

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

/// One logical table (a DataFusion `ListingTable` over many source paths). Only
/// *definitions* are durable; the runtime fields below are `#[serde(skip)]` and re-derived
/// when the engine re-registers a project on open.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct CatalogTable {
    pub name: String,
    #[serde(skip)]
    pub meta: String,
    pub format: String,
    pub sources: Vec<String>,
    /// Hive partition columns as `(name, arrow_type)` — the persisted source of truth for
    /// deterministic reload (types aren't re-detected).
    #[serde(default, deserialize_with = "de_partition_cols")]
    pub partition_cols: Vec<(String, String)>,
    #[serde(skip)]
    pub columns: Vec<ColumnInfo>,
    /// The source's own row count, when it reports one (Parquet footer does; CSV/JSON
    /// don't). Runtime like `columns`: re-read on every registration, never stored.
    #[serde(skip)]
    pub rows: Option<u64>,
    /// The last full-scan profile (D4), or `None` if never profiled. Cached on the row on
    /// purpose: the row is the unit replaced when the engine re-registers a table, so a
    /// config edit through `upsert_table` drops the profile with it.
    #[serde(skip)]
    pub profile: Option<CatalogProfile>,
    /// A profile scan is in flight for this table — keyed by entry, so several run at once.
    #[serde(skip)]
    pub profiling: bool,
    #[serde(skip)]
    pub open: bool,
    #[serde(skip)]
    pub status: RegStatus,
    #[serde(skip)]
    pub error: Option<String>,
}

/// A saved, query-backed catalog view (a real DataFusion `CREATE VIEW`).
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct CatalogView {
    pub name: String,
    pub sql: String,
    #[serde(skip)]
    pub meta: String,
    #[serde(skip)]
    pub columns: Vec<ColumnInfo>,
    /// The base tables this view reads (D10) — resolved by the planner at registration, so
    /// it sees through nested views and subqueries and never parses SQL itself. Runtime,
    /// like `columns`: re-derived on registration so it can't drift.
    #[serde(skip)]
    pub deps: Vec<String>,
    /// The **views** this view reads (D10) — transitive, since the planner inlines each hop
    /// and the walk collects every one.
    #[serde(skip)]
    pub view_deps: Vec<String>,
    /// The last full-scan profile (D4), or `None` if never profiled. A view has no footer,
    /// so a scan is the only way its inspector learns more than a column's type.
    #[serde(skip)]
    pub profile: Option<CatalogProfile>,
    /// A profile scan is in flight for this view.
    #[serde(skip)]
    pub profiling: bool,
    /// A **hard** registration failure — the view's SQL didn't plan (syntax/type error, or
    /// a base table missing at creation). `Some` = the row exists as a definition but there
    /// is no working view behind it.
    #[serde(skip)]
    pub error: Option<String>,
    #[serde(skip)]
    pub open: bool,
}
