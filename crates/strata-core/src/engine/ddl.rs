//! Statement **execution** — the `Verdict::Intercept` half of the router (ED-02,
//! `docs/STATEMENTS_SPEC.md` §4 + §7).
//!
//! [`Engine::run`](crate::engine::Engine::run) classifies once, in front of dispatch; a
//! statement the editor implements itself lands here as its [`StmtKind`], and comes back as a
//! [`StatementReport`] — what to say, how many rows it moved, and the [`StoreEffect`] the app
//! folds into `ProjectState`. Nothing here returns rows and nothing here touches the snapshot
//! lifecycle: DDL never retires a snapshot (`docs/SNAPSHOT_SPEC.md` §4), so a tab that creates a
//! table can still page the result it had.
//!
//! **The store learns from the returned value, never by introspection.** That is the whole
//! reason lifecycle is intercepted rather than left to DataFusion's provider traits (spec §3):
//! `SchemaProvider::register_table` cannot say who called it or await anything, so an accreted
//! native-DDL state would have to be *read back* — the `FetchCatalog` refetch the catalog
//! invariant forbids — or pushed out through a channel, which is the message-passing
//! architecture the direct-call facade deleted.
//!
//! **Every arm is one call into a funnel that already exists.** Typed `CREATE VIEW` is
//! [`Engine::create_view`](crate::engine::Engine::create_view) — the same call ⌘S makes; typed
//! `CREATE EXTERNAL TABLE` and a CTAS's spooled output are both `catalog::register_external`.
//! ED-02 ships the dispatch and the vocabulary; each arm is filled by the task that owns its
//! capability, and until then answers with its stub refusal.

use std::time::Instant;

use datafusion::prelude::SessionContext;
use datafusion::sql::parser::Statement as DFStatement;

use crate::engine::catalog::{TableMeta, ViewMeta};
use crate::engine::sql::StmtKind;
use strata_model::{TableDef, ViewDef};

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
    /// output (ED-04) or a typed `CREATE EXTERNAL TABLE` (ED-10). The def is the durable,
    /// shareable half; the meta is the answer that lands on its row.
    TableUpserted { def: TableDef, meta: TableMeta },
    /// A table def is gone and its provider deregistered (ED-05). `dependents` are the views
    /// left reading it — **named, never cascaded**: a `ViewTable`'s inlined plan goes on
    /// executing until reload, and the epoch bump makes diagnostics re-derive immediately,
    /// which is the surface that matters. They go `Reg::Failed` honestly on the next pass.
    TableRemoved {
        name: String,
        dependents: Vec<String>,
    },
    /// A view def arrived or was rewritten, already created (ED-06) — the same pair ⌘S folds.
    ViewUpserted { def: ViewDef, meta: ViewMeta },
    /// A view def is gone and the view dropped (ED-06).
    ViewRemoved { name: String },
    /// The table's *data* moved but its def did not — an `INSERT` appending a file (ED-05).
    /// A re-scan is what refreshes `TableMeta.rows`, because a row count is something the
    /// scan driver reads, never something the store adds up for itself.
    RescanTable { name: String },
    /// The session's function catalog moved (ED-09). Nothing persists — functions are
    /// session-scoped (spec §8) — but names that did not resolve a moment ago now do, so the
    /// catalog epoch has to move with them.
    FunctionsChanged,
}

/// Execute one intercepted statement and report what it did.
///
/// The timer and the kind are stamped here rather than in the arms, so a report can never
/// disagree with the statement that produced it.
#[allow(unused_variables)] // …until each arm's own ED task fills it in (module doc).
pub async fn execute(
    ctx: &SessionContext,
    kind: StmtKind,
    stmt: DFStatement,
    sql: String,
) -> Result<StatementReport, String> {
    let start = Instant::now();
    // Exhaustive on `StmtKind` with no wildcard, so a kind the router learns to intercept is a
    // compile error here rather than a statement that classifies and then falls through.
    // The arms are grouped by the task that owns each capability, which is also how they will
    // stop being stubs — one task, one arm, one funnel behind it.
    let outcome: StatementOutcome = match kind {
        // ED-04 — internal tables: spool the inner query to `.strata/tables/<slug>/`, register
        // the resulting Arrow def through `register_external`.
        StmtKind::CreateTable | StmtKind::Ctas => Err(unimplemented(kind)),
        // ED-05 — writes and removal over the internal-name set.
        StmtKind::Insert | StmtKind::DropTable => Err(unimplemented(kind)),
        // ED-06 — typed view DDL onto `Engine::create_view` / `Engine::drop_view`.
        StmtKind::CreateView | StmtKind::DropView => Err(unimplemented(kind)),
        // ED-07 — editor `COPY … TO`, behind the pre-flight NULL-partition gate.
        StmtKind::Copy => Err(unimplemented(kind)),
        // ED-08 — the session overlay and prepared statements.
        StmtKind::Set | StmtKind::Reset => Err(unimplemented(kind)),
        StmtKind::Prepare | StmtKind::Deallocate => Err(unimplemented(kind)),
        // ED-09 — the function factory and the swappable function catalog.
        StmtKind::CreateFunction | StmtKind::DropFunction => Err(unimplemented(kind)),
        // ED-10 — the typed form of Table Config's registration.
        StmtKind::CreateExternalTable => Err(unimplemented(kind)),
    }?;
    Ok(StatementReport {
        kind,
        message: outcome.message,
        count: outcome.count,
        elapsed_ms: start.elapsed().as_millis(),
        effect: outcome.effect,
    })
}

/// An intercepted kind whose implementation has not landed yet. A refusal, in the same register
/// as every other one — the statement classified, so the editor drew no squiggle, and the run
/// has to say plainly why it did nothing.
fn unimplemented(kind: StmtKind) -> String {
    format!("{} is not implemented yet", kind.label())
}
