//! The **query-history** vocabulary: one completed run, as it persists to
//! `.strata/history.jsonl` (one JSON line per entry). Pure serde leaf — the live satellite
//! store is the frontend's; this is only its durable shape.

use serde::{Deserialize, Serialize};

/// One completed, successful query run — a `history.jsonl` line and a History-drawer row
/// (FEATURES §12). Only successful data runs are recorded (a failed / cancelled run and an
/// Explain never reach here), so there's no status field.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct HistoryEntry {
    pub sql: String,
    /// When it completed — epoch millis, so the view can render relative time.
    pub ts_ms: u64,
    /// Wall-clock the run took.
    pub elapsed_ms: u64,
    /// Rows the run moved — a query's result size, or the rows an intercepted statement
    /// created / inserted / exported. `0` where a statement counts nothing (a `DROP`, a `SET`).
    pub rows: u64,
}
