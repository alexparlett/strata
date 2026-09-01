//! What a call **answers with** — the values the facade hands back, apart from the errors
//! beside them in [`error`](crate::error).
//!
//! One file because they are one thing: a caller reads a run's rows, pages the snapshot it
//! settled, or is told a statement performed. Nothing here decides anything; the modules that do
//! (`query`, `statements`) build these.

use strata_arrow::config::DisplayStamp;
use strata_arrow::RecordBatch;
use strata_model::{Cell, QueryOutput};

use crate::statements::StatementReport;

/// A settled query: the snapshot handle with page 1, that page still typed, and the display
/// config its cells were rendered under.
///
/// `output` holds the rows as display cells; `batch` holds the same rows typed, for a caller
/// that copies or exports them rather than showing them.
///
/// `display` is reported rather than asked for. A run renders under the config the engine is
/// running with when it is dispatched, so a caller showing these rows compares this against the
/// config it holds now to tell whether they still render the way a fresh read would.
#[derive(Debug)]
pub struct RunRows {
    /// Page 1 as display cells, with the snapshot handle and the run's own figures.
    pub output: QueryOutput,
    /// The same rows, still typed.
    pub batch: RecordBatch,
    /// The display config the cells were rendered under.
    pub display: DisplayStamp,
}

/// One page of a settled snapshot: the cells, and the same rows still typed.
///
/// [`RunRows`]'s shape, for a page after the first.
#[derive(Debug)]
pub struct SnapshotPage {
    /// The page as display cells.
    pub rows: Vec<Vec<Cell>>,
    /// The same rows, still typed.
    pub batch: RecordBatch,
}

/// Whether a config change took effect, or is waiting on a restart.
///
/// A `datafusion.runtime.*` key configures the `RuntimeEnv`, which is fixed when the engine is
/// built, so it is recorded rather than applied and the caller owes the user a restart.
#[must_use]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConfigOutcome {
    /// Every changed key is live on the session.
    Applied,
    /// At least one changed key needs a new engine to take effect.
    RestartOwed,
}

/// What a **Run** settled to ([`Workspace::run`](crate::Workspace::run)) — the two things a press can produce.
///
/// The split is the router's, not a mode the caller picks: a Run is one press, and whether it
/// produces rows or performs a statement is a property of what was typed.
pub enum RunOutcome {
    /// Exactly [`Workspace::query`](crate::Workspace::query)'s answer — the snapshot handle + page 1. Byte-for-byte the
    /// path that shipped: same supersede, same retire-on-dispatch, same pins.
    Rows(RunRows),
    /// An intercepted statement's report. **No snapshot**, and none retired: a tab that
    /// creates a table can still page the result it already had
    /// (`docs/SNAPSHOT_SPEC.md` §4 — DDL does not retire snapshots).
    Statement(StatementReport),
}
