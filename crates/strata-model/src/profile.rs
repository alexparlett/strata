//! The **result** of a catalog profiling scan (D4) — pure data vocabulary. The scan
//! *logic* (DataFusion aggregate expressions + result decoding) lives in `strata-core`;
//! this is only the shape it produces, cached on a `CatalogTable`/`CatalogView`.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::Stat;

/// A completed profile of one catalog entry — a table or a view.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogProfile {
    /// When the scan finished — the inspector shows this as an age.
    pub at: SystemTime,
    /// Rows scanned.
    pub rows: u64,
    /// The query that produced these numbers (unparsed from the `Expr`s that ran, so it
    /// can't drift from the facts). Empty when the unparser couldn't render an expression.
    pub sql: String,
    /// Facts per column name — the same `Stat` list the free (footer) tier produces, so
    /// the inspector renders both through one path. A column the scan couldn't say
    /// anything about is simply absent, never present-but-empty.
    pub cols: BTreeMap<String, Vec<Stat>>,
}
