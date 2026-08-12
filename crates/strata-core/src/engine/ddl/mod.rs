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
//! **Every arm is one call into a funnel that already exists.** Typed `CREATE VIEW` runs
//! [`views::create`] — the body [`Engine::create_view`](crate::engine::Engine::create_view) runs
//! for ⌘S; typed `CREATE EXTERNAL TABLE` and a CTAS's spooled output are both
//! `catalog::register_external`. ED-02 shipped the dispatch and the vocabulary and each arm was
//! filled by the task that owned its capability; ED-10 was the last of them, so there is no stub
//! refusal left and the `match` below is exhaustive on `StmtKind` with every arm real.

mod copy;
mod external;
mod functions;
mod session;
mod tables;
mod views;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use datafusion::catalog::TableProvider;
use datafusion::logical_expr::TableType;
use datafusion::prelude::SessionContext;
use datafusion::sql::parser::Statement as DFStatement;
use datafusion::sql::TableReference;

use crate::engine::catalog::{TableMeta, ViewMeta};
use crate::engine::functions::Functions;
use crate::engine::sql::StmtKind;
use crate::engine::{fold_ident, Connections, InternalTables, CATALOG, SCHEMA};
use crate::util::plural;
use strata_model::{TableDef, ViewDef};

/// The statement family's completion vocabulary (ED-11) — the format words `STORED AS`
/// takes and the per-format `OPTIONS` key tables, owned by the module whose arms they
/// mirror and read by `sql::complete` so the offer and the arm set are one table.
pub(crate) use external::{option_keys_for, OptionKind, STORED_AS_FORMATS};
/// DataFusion's own seam for `CREATE FUNCTION` (ED-09) — installed on every engine by
/// `build_context`, which is what makes the statement dispatchable at all.
pub(super) use functions::StrataFunctionFactory;
/// The `SET` overlay's key fence (ED-08) — also the `SET` key pool's filter (ED-11), so
/// what completion offers and what dispatch accepts cannot drift.
pub(crate) use session::refuse_reserved_key;
/// The session state a statement can move (ED-08) — held by the engine, reached by the arms.
pub use session::SessionScope;
/// A table drop's own words — see [`tables::drop_intent`]. Re-exported here because the
/// catalog pane says them too, and `ddl` is the vocabulary module the app already reads.
pub use tables::drop_intent;
pub(super) use tables::drop_table;
pub(super) use views::{create as create_view, drop as drop_view};

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
    /// The session's prepared statements moved (ED-08) — a `PREPARE` or a `DEALLOCATE`. Nothing
    /// persists either, and for the same reason it is still an effect: `EXECUTE p` resolves now
    /// and did not a moment ago, so both the language service's snapshot and every tab's
    /// diagnostics have to be re-derived against the session the engine now holds.
    PreparedChanged,
}

/// Where an intercepted statement may write, and what it may write **relative to**.
///
/// The **project folder**, not `.strata/tables` — because a statement that creates an internal
/// table produces two things from it: an absolute path to spool into, and the project-relative
/// source path the def stores, which is what makes the def portable
/// ([`internal_source`](crate::project::internal_source)). Handing down only the data directory
/// would leave the def naming an absolute path on the machine that ran the statement.
///
/// `None` is an engine with no project behind it — the agent's headless workspaces before a
/// project is opened, and every test fixture. Nothing that only reads notices; the arms that
/// write refuse politely.
pub type DataRoot = Option<PathBuf>;

/// What an intercepted statement can reach **of the engine**, gathered once in
/// [`Engine::run`](crate::engine::Engine::run).
///
/// Every member is a copy — a handle where the state is shared, a clone where it is a value — for
/// one reason: the arms run inside the task `Engine::bookkeep` spawned, and that task must not
/// hold the engine, because the engine's `Drop` is what aborts it. `internal`, `scope` and
/// `functions` hold values only, so they outlive an engine harmlessly; `root` and `baseline` are
/// snapshots taken at dispatch, which is the moment they are true for.
///
/// One value rather than a parameter list because it is one thing — the engine, minus everything
/// an arm may not touch — and it gains a member for each capability this workstream lifts.
pub struct Dispatch {
    /// Where an internal table's data may be written (ED-04).
    pub root: DataRoot,
    /// Which registered tables Strata owns the data of (ED-04/05).
    pub internal: InternalTables,
    /// Which object stores this project has a connection to (ED-10) — what a typed
    /// `CREATE EXTERNAL TABLE`'s `LOCATION` may name.
    pub connections: Connections,
    /// The `SET` overlay and the prepared-statement mirror (ED-08).
    pub scope: SessionScope,
    /// The function catalog and the names this session created (ED-09).
    pub functions: Functions,
    /// The engine's `datafusion.*` overrides — what a `RESET` puts a key back to
    /// (`session::reset`), which is the Settings baseline rather than DataFusion's default.
    pub baseline: BTreeMap<String, String>,
}

/// Execute one intercepted statement and report what it did.
///
/// The timer and the kind are stamped here rather than in the arms, so a report can never
/// disagree with the statement that produced it.
pub async fn execute(
    ctx: &SessionContext,
    kind: StmtKind,
    stmt: DFStatement,
    engine: Dispatch,
) -> Result<StatementReport, String> {
    let Dispatch {
        root,
        internal,
        connections,
        scope,
        functions: registry,
        baseline,
    } = engine;
    let start = Instant::now();
    // Exhaustive on `StmtKind` with no wildcard, so a kind the router learns to intercept is a
    // compile error here rather than a statement that classifies and then falls through.
    // The arms are grouped by the task that owns each capability, which is also how they will
    // stop being stubs — one task, one arm, one funnel behind it.
    let outcome: StatementOutcome = match kind {
        // ED-04 — internal tables: spool the inner query to `.strata/tables/<slug>/`, register
        // the resulting Arrow def through `register_external`.
        StmtKind::CreateTable | StmtKind::Ctas => tables::create(ctx, kind, stmt, root).await,
        // ED-05 — writes and removal over the internal-name set.
        StmtKind::Insert => tables::insert(ctx, stmt, &internal).await,
        StmtKind::DropTable => tables::drop_statement(ctx, &root, &internal, stmt).await,
        // ED-06 — typed view DDL onto the body the save-view funnel already runs.
        StmtKind::CreateView => views::create_statement(ctx, stmt).await,
        StmtKind::DropView => views::drop_statement(ctx, stmt).await,
        // ED-07 — editor `COPY … TO`, behind the pre-flight NULL-partition gate.
        StmtKind::Copy => copy::copy_to(ctx, stmt, &root).await,
        // ED-08 — the session overlay and prepared statements.
        StmtKind::Set => session::set(ctx, stmt, &scope).await,
        StmtKind::Reset => session::reset(ctx, stmt, &scope, &baseline).await,
        StmtKind::Prepare => session::prepare(ctx, stmt, &scope).await,
        StmtKind::Deallocate => session::deallocate(ctx, stmt, &scope).await,
        // ED-09 — SQL-bodied scalar functions, over the factory `build_context` installed.
        StmtKind::CreateFunction => functions::create(ctx, stmt, &registry).await,
        StmtKind::DropFunction => functions::drop(ctx, stmt, &registry).await,
        // ED-10 — the typed form of Table Config's registration.
        StmtKind::CreateExternalTable => {
            external::create(ctx, stmt, &root, &internal, &connections).await
        }
    }?;
    Ok(StatementReport {
        kind,
        message: outcome.message,
        count: outcome.count,
        elapsed_ms: start.elapsed().as_millis(),
        effect: outcome.effect,
    })
}

/// What `name` resolves to in the engine's one schema, and what kind it is — `None` when the
/// name is free. The one existence question every arm asks, because tables and views share that
/// namespace and a create has to know which of them it is standing on.
///
/// Through `table_provider`, not `table`: the latter builds a `DataFrame`, which for a view means
/// planning its whole body just to ask whether the name is taken. Addressed as a **bare, folded**
/// reference for the reason [`Engine::create_view`](crate::engine::Engine::create_view) gives —
/// `impl Into<TableReference> for &str` parses, and a name that needed quoting does not survive a
/// parse, so it would be looked up under a name nothing ever registered.
pub(super) async fn existing(ctx: &SessionContext, name: &str) -> Option<TableType> {
    let provider: Arc<dyn TableProvider> = ctx
        .table_provider(TableReference::bare(fold_ident(name)))
        .await
        .ok()?;
    Some(provider.table_type())
}

/// The bare name a statement targets, and `what` those statements create — `"Tables"`,
/// `"Views"`.
///
/// Strata has exactly one catalog and one schema (`engine::providers`), so a qualified name is
/// either a longer spelling of the same place or a place that does not exist — and registration
/// takes a bare name, so an unrecognised qualifier would otherwise be silently dropped and the
/// object created somewhere the user did not ask for.
pub(super) fn bare_name(name: &TableReference, what: &str) -> Result<String, String> {
    let ok = match name {
        TableReference::Bare { .. } => true,
        TableReference::Partial { schema, .. } => schema.as_ref() == SCHEMA,
        TableReference::Full {
            catalog, schema, ..
        } => catalog.as_ref() == CATALOG && schema.as_ref() == SCHEMA,
    };
    match ok {
        true => Ok(name.table().to_string()),
        false => Err(elsewhere(what)),
    }
}

/// The wording for a name that points outside Strata's single schema — held apart from
/// [`bare_name`] because a caller that parses the name itself has to be able to refuse the forms
/// a `TableReference` cannot even represent, in the same words (`views::definition`).
pub(super) fn elsewhere(what: &str) -> String {
    format!("Strata has one schema, '{SCHEMA}'. {what} cannot be created elsewhere")
}

/// What a drop leaves behind, appended to its report — empty when it leaves nothing.
///
/// One wording for both drops, because "left invalid" is one fact: a dependent's plan was inlined
/// when it was created and goes on executing until reload, so nothing is stale yet and nothing is
/// cascaded. Shared so a table drop and a view drop cannot describe the same consequence two ways.
pub(super) fn left_invalid(dependents: &[String]) -> String {
    if dependents.is_empty() {
        return String::new();
    }
    let names: Vec<String> = dependents.iter().map(|v| format!("'{v}'")).collect();
    let verb = match dependents.len() {
        1 => "is",
        _ => "are",
    };
    format!(
        ". {} {verb} left invalid: {}",
        plural(dependents.len(), "view"),
        names.join(", ")
    )
}
