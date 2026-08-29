//! The app's core **data vocabulary** — the shapes the whole app reasons in, below every layer
//! that produces or consumes them.
//!
//! A leaf module depending on nothing app-specific, so `engine`, `profile`, `project` and the UI
//! all depend *down* onto one vocabulary. The engine's *protocol* (`TableSpec`, `TableMeta`) stays
//! in `strata_engine`: that is the engine's wire format, not shared vocabulary.

mod catalog;
mod chart;
mod diagnostics;
mod history;
mod profile;
mod query_error;
mod results;
mod schema;
mod session;
mod source;

pub use catalog::{
    CatalogKind, ColOwner, ColRef, CsvRead, FileCompression, JsonRead, JsonShape, RemoteRef,
    RemoveKind, RemoveTarget, SavedQuery, SourceFormat, TableDef, TableOrigin, ViewDef,
};
pub use chart::{
    Axis, CapUnit, ChartBin, ChartConfig, ChartData, ChartMark, ChartPoint, ChartQuery,
    ChartSeries, ChartSort, ChartX, Trend,
};
pub use diagnostics::{Diagnostic, Severity};
pub use history::HistoryEntry;
pub use profile::CatalogProfile;
pub use query_error::QueryError;
pub use results::{Cell, PageQuery, QueryOutput, SnapshotId};
pub use schema::{ChartRole, ColumnInfo, Kind, Stat, StatKey};
pub use session::{
    expanded_drawer_h, DrawerTab, Layout, Origin, ProblemsTab, ResultsView, RightPane,
    SessionSnapshot, SidebarPane, TabId, TabSnapshot, WindowGeom,
};
pub use source::{check_catalog, check_catalog_name, SourceDef, WORKSPACE_CATALOG};
