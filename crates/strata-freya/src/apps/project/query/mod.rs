//! The window's **query layer** (state-arch §6): freya-query capabilities over the
//! engine facade. Owned by the results element — no runs store, no query state on the
//! session.
//!
//! The page-read side (`FetchSnapshotPage` and friends) is consumed by the grid's paging and
//! sort; the chart read ([`chart`]) is the third capability, on the page read's terms — see its
//! module note.

mod chart;
mod profile;
mod run_query;

pub use chart::{ChartSpec, TrendSpec};
pub use profile::{use_profile, ScanId};
pub use run_query::{
    PageSpec, QueryMode, QueryOutcome, QueryPage, QuerySpec, RunId, RunQuery, DEFAULT_PAGE_SIZE,
};
