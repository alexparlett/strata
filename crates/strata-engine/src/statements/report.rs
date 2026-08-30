//! What one intercepted statement did, as a value the caller folds.
//!
//! An arm answers with a [`StatementOutcome`]; [`execute`](super::arms::execute) stamps the kind
//! and the clock onto it and hands back a [`StatementReport`], so a report can never disagree
//! with the statement that produced it. The catalog mutation rides along as a [`StoreEffect`].

use strata_model::{TableDef, ViewDef};

use crate::catalog::{TableMeta, ViewMeta};
use crate::statements::StmtKind;

/// What one intercepted statement did — the `RunOutcome::Statement` the results pane renders
/// as a status row and the app folds into its stores.
#[derive(Clone, Debug, PartialEq)]
pub struct StatementReport {
    /// Which statement ran. The results pane's label and the log's subject come off
    /// [`StmtKind::label`], so the kind travels rather than a second spelling of it.
    pub kind: StmtKind,
    /// The sentence the user reads, in the app's IDE register — and the one place a
    /// session-scoped outcome says so ("for this session"), since `SET`, prepared statements
    /// and created functions die with the engine (spec §8).
    pub message: String,
    /// Rows created / inserted / exported, where the statement moved any. `None` is *not
    /// applicable* — a `DROP` or a `SET` counts nothing, which is a different fact from
    /// counting zero.
    pub count: Option<u64>,
    pub elapsed_ms: u128,
    /// What the app folds into `ProjectState`. `None` where the statement changed nothing the
    /// catalog holds; deliberately not a `StoreEffect::None` variant beside it, which would be
    /// a second way to say the same thing and a second arm every fold has to remember.
    pub effect: Option<StoreEffect>,
}

/// What an arm answers with — [`StatementReport`] minus the two fields `execute` owns. An arm
/// therefore cannot mislabel itself or forget to stamp the clock.
pub struct StatementOutcome {
    pub message: String,
    pub count: Option<u64>,
    pub effect: Option<StoreEffect>,
}

/// The catalog mutation a statement leaves behind, as a **value the app applies** — the
/// `save_view` fold generalized (spec §7): store upsert on the matching `ProjChan` → the
/// persist funnel → `catalog_settled` → the event log.
///
/// The store stays the catalog authority, so nothing here is a request to go and look: an
/// effect carries the def *and* what registration learned about it, exactly as the load-time
/// pass hands both to the same row.
#[derive(Clone, Debug, PartialEq)]
pub enum StoreEffect {
    /// A table def arrived or was rewritten, already registered — an internal table's CTAS
    /// output or a typed `CREATE EXTERNAL TABLE`. The def is the durable,
    /// shareable half; the meta is the answer that lands on its row.
    TableUpserted { def: TableDef, meta: TableMeta },
    /// A table def is gone and its provider deregistered. `dependents` are the views
    /// left reading it — **named, never cascaded**: a `ViewTable`'s inlined plan goes on
    /// executing until reload, and the epoch bump makes diagnostics re-derive immediately,
    /// which is the surface that matters. They go `Reg::Failed` honestly on the next pass.
    TableRemoved {
        name: String,
        dependents: Vec<String>,
    },
    /// A view def arrived or was rewritten, already created — the same pair ⌘S folds.
    ViewUpserted { def: ViewDef, meta: ViewMeta },
    /// A view def is gone and the view dropped.
    ViewRemoved { name: String },
    /// The table's *data* moved but its def did not — an `INSERT` appending a file.
    /// A re-scan is what refreshes `TableMeta.rows`, because a row count is something the
    /// scan driver reads, never something the store adds up for itself.
    RescanTable { name: String },
    /// The session's function catalog moved. Nothing persists — functions are
    /// session-scoped (spec §8) — but names that did not resolve a moment ago now do, so the
    /// catalog epoch has to move with them.
    FunctionsChanged,
    /// The session's prepared statements moved — a `PREPARE` or a `DEALLOCATE`. Nothing
    /// persists either, and for the same reason it is still an effect: `EXECUTE p` resolves now
    /// and did not a moment ago, so both the language service's snapshot and every tab's
    /// diagnostics have to be re-derived against the session the engine now holds.
    PreparedChanged,
    /// A data source holds a relation it did not a moment ago — a remote CTAS.
    /// The store has no row for a remote relation and never will (*discovery gets catalogs*), so
    /// there is nothing to upsert; what has to move is the catalog epoch, which the tree,
    /// completion and every tab's diagnostics already key on. The `FunctionsChanged` shape, for
    /// the same reason.
    RemoteRelationsChanged,
}
