//! The app's core **data vocabulary** — the shapes the whole app reasons in, below
//! every layer that produces or consumes them.
//!
//! These types used to be scattered: the schema/results types lived in `crate::engine`
//! (which made the engine look like their owner — it isn't; a `Stat` is read from a
//! Parquet footer *by the engine* and computed from a scan *by [`crate::profile`]*, a
//! `ColumnInfo` is produced by the engine, stored by [`crate::project`] and rendered by
//! the UI), the form/log/menu types lived in a grab-bag `state.rs`, `QueryError` was a
//! top-level file, and `Kind` was in `util`. Consolidating them into one leaf module —
//! depending on nothing app-specific — lets `engine`, `profile`, `project`, `runs` and
//! the UI all depend *down* onto one vocabulary, and breaks the `engine ↔ profile`
//! cycle the old ownership forced.
//!
//! The engine's *protocol* (`Command`, `Event`, `TableSpec`, `TableMeta`) deliberately
//! stays in `crate::engine`: that's the engine's wire format, not shared vocabulary.

mod catalog;
mod diagnostics;
mod form;
mod log;
mod profile;
mod query_error;
mod results;
mod schema;

pub use catalog::{
    CatalogKind, ColRef, RemoveKind, RemoveTarget, SavedQuery, TableDef, ViewDef,
};
pub use diagnostics::{DiagSource, Diagnostic, Severity};
pub use form::{ConfigForm, ExportForm};
pub use log::{LogEvent, LogKind, LogTab};
pub use profile::CatalogProfile;
pub use query_error::QueryError;
pub use results::{Cell, QueryOutput, SnapshotId};
pub use schema::{ColumnInfo, Kind, Stat, StatKey};
