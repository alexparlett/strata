//! Strata's **Arrow-level vocabulary** — the layer between the serde-only [`strata_model`] and the
//! DataFusion boundary `strata_engine`.
//!
//! Everything here answers a question about a value, a column or a setting, and answers it without
//! planning or executing anything: the [`column`](mod@column) row an Arrow field becomes, the [`value_tree`] a
//! nested cell expands into, the [`serialize`] writers behind Copy and the record view, the
//! [`plan`] model an EXPLAIN is read into, and the two written-down catalogues — DataFusion's
//! [`config`] keys and `object_store`'s [`client`] options — that a settings editor offers from and
//! the engine applies.
//!
//! **No DataFusion**, which is the point: a surface that formats a cell or offers a config key
//! should not compile a query planner to do it. The claim is kept by this crate's `Cargo.toml`
//! rather than by care. Arrow is named directly here and reached through `datafusion::arrow` in the
//! engine; the workspace pins one arrow so those are the same types.
//!
//! The identifier renderers deliberately stay in the engine: `fold_ident`'s body *is*
//! `TableReference::parse_str`, and `quote_ident` reads the lexer's reserved-word tables.

pub mod chart;
pub mod client;
pub mod column;
pub mod config;
pub mod plan;
pub mod profile;
pub mod serialize;
pub mod value_tree;

/// A column's vocabulary row is derived from an Arrow field in exactly one place, and anything
/// building a column — a fixture included — should go through it rather than hand-writing a row
/// whose `kind` and `role` are then a second opinion about the same type.
pub use column::{chart_role, column_info};

/// The bin cap the histogram read clamps to — at the root so the control offering a bin count is
/// bounded by the same number rather than a second copy of it.
pub use chart::MAX_BINS;

/// The Arrow batch type engine results carry (the type-aware source for Copy/Export),
/// re-exported so frontends can name it without their own Arrow dependency.
pub use arrow::record_batch::RecordBatch;

/// The Arrow schema type, re-exported for the same reason — code (and tests) holding a
/// [`RecordBatch`] sometimes needs to name its schema.
pub use arrow::datatypes::Schema;
