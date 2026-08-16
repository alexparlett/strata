//! **Internal tables** (ED-04, ED-05) — `CREATE TABLE` and CTAS, spooled into the project's own
//! `.strata/tables/` and registered through the funnel every other table goes through, plus the
//! two statements that then write over them: `INSERT` and `DROP TABLE`.
//! `docs/STATEMENTS_SPEC.md` §6.1 + §7.
//!
//! `DROP TABLE` works on **both** origins and is the one place a table is dropped: the catalog
//! pane's confirm and a typed statement reach [`drop_table`] through
//! [`Engine::drop_table`](crate::Engine::drop_table) and
//! [`drop_statement`] respectively. Two gestures, one implementation — because the thing that
//! differs between them is a question asked of the user, not what the drop does, and an internal
//! table's data directory has to go on both or it is silent data left on disk.
//!
//! **Not DataFusion's CTAS**, which collects the whole result into RAM as a `MemTable` and
//! registers it from a sync hook: nothing can be streamed to disk and nothing survives a restart.
//! Both are disqualifying — a table is a durable thing here, and the result may be larger than the
//! window.
//!
//! **What is still DataFusion's, deliberately.** The *plan*: the statement goes to
//! `statement_to_plan` exactly as parsed, so the query that runs is the query the user wrote by
//! construction rather than by fidelity of a round trip. Planning a `CREATE TABLE` executes
//! nothing, and it buys two things — the planner already refuses every clause it does not
//! implement, fifty-odd of them each with its own message, and already resolves a declared column
//! list against the query. And the *write*: a `CopyTo` node over that plan, `STORED AS ARROW`,
//! which streams, writes the snapshot codec and reports the exact row count.
//!
//! Ours is the part no hook can carry: where the files go, that they are published by rename, what
//! the def says, and the name semantics against the one namespace tables and views share.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use datafusion::arrow::ipc::writer::FileWriter;
use datafusion::catalog::TableProvider;
use datafusion::dataframe::DataFrame;
use datafusion::datasource::file_format::arrow::ArrowFormatFactory;
use datafusion::datasource::file_format::format_as_file_type;
use datafusion::datasource::DefaultTableSource;
use datafusion::logical_expr::dml::{DmlStatement, InsertOp, WriteOp};
use datafusion::logical_expr::{CreateMemoryTable, DdlStatement, LogicalPlan, TableType};
use datafusion::prelude::{SQLOptions, SessionContext};
use datafusion::sql::parser::Statement as DFStatement;

use crate::catalog::{dependent_views, register_external, short_type, TableSpec};
use crate::db::{self, Databases, RemoteTarget};
use crate::export::copy_row_count;
use crate::query::ipc_write_options;
use crate::sink::append_rows;
use crate::sql::{Blocked, StmtKind};
use crate::{fold_ident, InternalTables};
use strata_core::project::{internal_source, tables_dir};
use strata_core::util::{plural, temp_dir_name};
use strata_model::{SourceFormat, TableDef, TableOrigin};

use super::{
    bare_name, existing, in_database, left_invalid, read_only, remote_target, DataRoot,
    StatementOutcome, StoreEffect,
};

/// What [`bare_name`] calls the objects these statements create.
const WHAT: &str = "Tables";

/// Create an internal table from a `CREATE TABLE` / `CREATE TABLE … AS SELECT`.
///
/// One function for both kinds because DataFusion plans them into one node: a declared column
/// list with no query becomes an `EmptyRelation` carrying that schema, and the spool below then
/// writes it as a schema-carrying, zero-row Arrow file. The router still names them apart
/// (`StmtKind`) because the *report* says different things, and because a kind that classifies is
/// a kind some later task may implement differently.
pub async fn create(
    ctx: &SessionContext,
    kind: StmtKind,
    stmt: DFStatement,
    root: DataRoot,
    databases: &Databases,
) -> Result<StatementOutcome, String> {
    let plan = ctx
        .state()
        .statement_to_plan(stmt)
        .await
        .map_err(|e| e.to_string())?;
    let LogicalPlan::Ddl(DdlStatement::CreateMemoryTable(create)) = plan else {
        return Err(format!("{} did not plan as a table", kind.label()));
    };
    if let Some(refusal) = unenforced_clause(&create) {
        return Err(refusal.into());
    }
    let CreateMemoryTable {
        name,
        constraints: _,
        input,
        if_not_exists,
        or_replace,
        column_defaults: _,
        temporary: _,
    } = create;

    let mut seen = Vec::new();
    for field in input.schema().fields() {
        let folded = fold_ident(field.name());
        if seen.contains(&folded) {
            return Err(duplicate_column(field.name()));
        }
        seen.push(folded);
    }

    if let Some(at) = remote_target(ctx, &name) {
        return remote(ctx, kind, &at, databases, &input, if_not_exists, or_replace).await;
    }
    let name = bare_name(ctx, &name, WHAT)?;
    let Some(root) = root else {
        return Err(format!(
            "{} needs a project folder to store the table's data",
            kind.label()
        ));
    };

    let replacing = match existing(ctx, &name).await {
        Some(TableType::View) => return Err(format!("'{name}' is a view")),
        Some(_) if if_not_exists => {
            return Ok(StatementOutcome {
                message: format!("Table '{name}' already exists"),
                count: None,
                effect: None,
            })
        }
        Some(_) if !or_replace => return Err(format!("Table '{name}' already exists")),
        taken => taken.is_some(),
    };

    SQLOptions::new()
        .with_allow_dml(false)
        .with_allow_ddl(false)
        .with_allow_statements(false)
        .verify_plan(&input)
        .map_err(|e| e.to_string())?;

    let slug = table_slug(&name);
    let dir = tables_dir(&root).join(&slug);
    let rows = spool(ctx, &input, &tables_dir(&root), &slug).await?;

    let def = TableDef {
        name: name.clone(),
        format: SourceFormat::Arrow,
        connection: None,
        sources: vec![internal_source(&slug)],
        partition_cols: Vec::new(),
        origin: TableOrigin::Internal,
    };
    let spec = TableSpec {
        name: name.clone(),
        paths: vec![dir_path(&dir)],
        format: SourceFormat::Arrow,
        partitions: Vec::new(),
        internal: true,
    };
    let meta = match register_external(ctx, &spec).await {
        Ok(meta) => meta,
        Err(e) if !replacing => {
            let _ = fs::remove_dir_all(&dir);
            return Err(e);
        }
        Err(e) => return Err(e),
    };

    let verb = if replacing { "replaced" } else { "created" };
    Ok(StatementOutcome {
        message: format!("Table '{name}' {verb}, {}", plural(rows as usize, "row")),
        count: Some(rows),
        effect: Some(StoreEffect::TableUpserted { def, meta }),
    })
}

/// **`CREATE TABLE pg.schema.t AS SELECT …`** — a result materialized as a real server table
/// (DB-10).
///
/// The input plan is an ordinary query, so a local scan, a federated remote one and a
/// cross-source join all reach here already working; what is added is where the rows land.
///
/// **A column list is not this.** `CREATE TABLE pg.public.t (id SERIAL)` declares a schema in the
/// server's own type vocabulary, which only the server should judge — DB-11's, refused here by
/// [`in_database`] exactly as before. The two are told apart by [`StmtKind`], which is what that
/// distinction is for.
///
/// **The three name semantics are answered against the server as it is now**, by the same
/// transaction that creates — never against the connect-time enumeration, and never by a round
/// trip whose answer could go stale before the create (`db::create_table_as`). `OR REPLACE` is
/// refused for `INSERT OVERWRITE`'s reason, but **only where there is something to replace**: it
/// would drop a server table, and a statement that silently destroys one is not v1. Over a free
/// name it simply creates, exactly as the local arm does.
async fn remote(
    ctx: &SessionContext,
    kind: StmtKind,
    at: &RemoteTarget,
    databases: &Databases,
    input: &Arc<LogicalPlan>,
    if_not_exists: bool,
    or_replace: bool,
) -> Result<StatementOutcome, String> {
    let address = at.address();
    if kind == StmtKind::CreateTable {
        return Err(in_database(&address, &at.catalog));
    }
    if !db::writable(databases, &at.catalog) {
        return Err(read_only(at));
    }

    SQLOptions::new()
        .with_allow_dml(false)
        .with_allow_ddl(false)
        .with_allow_statements(false)
        .verify_plan(input)
        .map_err(|e| e.to_string())?;

    let Some(rows) = db::create_table_as(ctx, databases, at, input).await? else {
        if or_replace {
            return Err(format!(
                "CREATE OR REPLACE TABLE is not supported on '{address}'. Drop it on the server \
                 first"
            ));
        }
        if !if_not_exists {
            return Err(format!("Table '{address}' already exists"));
        }
        return Ok(StatementOutcome {
            message: format!("Table '{address}' already exists"),
            count: None,
            effect: None,
        });
    };
    Ok(StatementOutcome {
        message: format!(
            "Table '{address}' created, {}",
            plural(rows as usize, "row")
        ),
        count: Some(rows),
        effect: Some(StoreEffect::RemoteRelationsChanged),
    })
}

/// The two clauses DataFusion **plans and does not enforce**, refused by name.
///
/// It does not check a constraint even on its own `MemTable`, so accepting one would be a
/// promise nothing keeps; a column default has no meaning until something can `INSERT` without
/// naming the column, which needs a provider that applies it.
///
/// Held apart from [`create`] because [`column_type`] has to ask the same question of the same
/// planned statement: a `PRIMARY KEY` typed into the empty-table panel's type box has to be
/// refused *while it is being typed*, in the words the create would have used (IT-01).
fn unenforced_clause(create: &CreateMemoryTable) -> Option<&'static str> {
    if !create.constraints.is_empty() {
        return Some("Table constraints are not supported");
    }
    if !create.column_defaults.is_empty() {
        return Some("Column defaults are not supported");
    }
    None
}

/// What a repeated column name is refused with.
///
/// Reproduced from DataFusion's own `ensure_unique_column_names` rule rather than inherited: its
/// CTAS never writes a file, and an IPC file *would* store both columns, after which every read
/// of the table resolves the second by name onto the first.
///
/// `pub` because the empty-table panel refuses the same thing as its rows are typed (IT-01), and
/// a form that said it in its own words would be a second wording for one rule.
pub fn duplicate_column(name: &str) -> String {
    format!("Duplicate column name '{name}'. Alias one of them")
}

/// The throwaway statement [`column_type`] probes with. Never executed and never registered, so
/// the name is only ever parsed — but it is spelled to be unmistakably ours if it ever appears
/// in a planner message the user reads.
const PROBE_TABLE: &str = "__strata_probe";
/// The one column that statement declares.
const PROBE_COLUMN: &str = "c";

/// What DataFusion's planner makes of one **SQL column type** on this session — the empty-table
/// panel's per-row validation (IT-01).
///
/// **There is no Arrow → SQL inverse to author an offer from**, which is why that panel's type
/// field is free text and why this stands behind it. `convert_simple_data_type` is many-to-one
/// (`INT | INTEGER | INT4` all reach `Int32`), and the same spelling reaches *different* Arrow
/// types depending on session config: `map_string_types_to_utf8view` flips `VARCHAR` between
/// `Utf8` and `Utf8View`, and `execution.time_zone` fills the zone on `TIMESTAMP WITH TIME ZONE`.
/// So nothing is declared — the planner is asked, on the very session the create will run on, and
/// its answer (or its refusal, in its own words) is what the row shows.
///
/// The answer is [`short_type`], which is the spelling `ColumnInfo::dtype` carries and therefore
/// the one the grid header and the inspector will show once the table exists: the form promises
/// exactly what the user is about to see, rather than a second rendering of the same type.
///
/// It plans the statement the panel composes and **executes nothing** — execution lives only in
/// `execute_logical_plan` (module doc) — so this costs a parse and a plan of one empty relation.
pub async fn column_type(ctx: &SessionContext, sql_type: &str) -> Result<String, String> {
    let typed = sql_type.trim();
    if typed.is_empty() {
        return Err("Enter a column type".into());
    }
    let sql = format!("CREATE TABLE {PROBE_TABLE} ({PROBE_COLUMN} {typed})");
    let plan = ctx
        .state()
        .create_logical_plan(&sql)
        .await
        .map_err(|e| e.to_string())?;
    let LogicalPlan::Ddl(DdlStatement::CreateMemoryTable(create)) = plan else {
        return Err(not_a_column_type(typed));
    };
    if let Some(refusal) = unenforced_clause(&create) {
        return Err(refusal.into());
    }
    if !matches!(create.input.as_ref(), LogicalPlan::EmptyRelation(_)) {
        return Err(not_a_column_type(typed));
    }
    let schema = create.input.schema();
    match schema.fields().as_ref() {
        [field] => Ok(short_type(field.data_type())),
        _ => Err(not_a_column_type(typed)),
    }
}

/// What the probe says about a value that plans as something other than one column's type.
fn not_a_column_type(typed: &str) -> String {
    format!("'{typed}' is not a column type")
}

/// Append rows to an internal table from an `INSERT` (ED-05).
///
/// **Native execution behind a target gate.** The only thing intercepted is *where* the write
/// lands, because that is the one question DataFusion has no opinion about: a `ListingTable`
/// writes into whatever directory it was registered over, and Strata's rule is that a statement
/// may only write files Strata owns. Everything else is DataFusion's own INSERT path unchanged —
/// the column list, the source query, the schema check
/// (`logically_equivalent_names_and_types`, which surfaces a mismatch in its own words), and the
/// single LZ4-frame IPC file the Arrow sink appends.
///
/// **The sink is driven rather than the node executed** (DB-12), on both branches — over the
/// provider the plan already resolved, so a source inside a database connection reaches a
/// workspace table rather than a `Dml` reaching the unparser ([`crate::sink`]).
///
/// **One file per statement, and no compaction.** The sink appends rather than rewrites, so a
/// table inserted into a thousand times is a thousand files and every scan lists them all.
/// `DROP TABLE` plus a `CREATE TABLE AS SELECT * FROM t` is the compaction story until a task
/// owns one.
///
/// The gate is in two halves and the **catalog** is the first: a relation inside a database
/// connection is not a table whose data Strata could ever own, so [`remote_target`] answers for it
/// before [`InternalTables`] is consulted at all. Ownership is the wrong question to ask about it,
/// and the connection's own gate — is it writable — is the honest one there.
///
/// **The remote branch builds the provider it drives**, because the one the catalog serves is the
/// federated *read* provider, whose `insert_into` is the trait's own `not_impl_err`. The writer
/// that can is built by [`db::insert_into`], used once and dropped, so no plan ever sees it and
/// pushdown on every read is untouched.
pub async fn insert(
    ctx: &SessionContext,
    stmt: DFStatement,
    internal: &InternalTables,
    databases: &Databases,
) -> Result<StatementOutcome, String> {
    let plan = ctx
        .state()
        .statement_to_plan(stmt)
        .await
        .map_err(|e| e.to_string())?;
    let LogicalPlan::Dml(dml) = &plan else {
        return Err(format!(
            "{} did not plan as a write",
            StmtKind::Insert.label()
        ));
    };
    if !matches!(dml.op, WriteOp::Insert(InsertOp::Append)) {
        return Err(Blocked::InsertOverwrite.editor_message());
    }

    if let Some(at) = remote_target(ctx, &dml.table_name) {
        if !db::writable(databases, &at.catalog) {
            return Err(read_only(&at));
        }
        verify_insert(&plan)?;
        let rows = db::insert_into(ctx, databases, &at, &dml.input).await?;
        return Ok(StatementOutcome {
            message: format!(
                "Inserted {} into '{}'",
                plural(rows as usize, "row"),
                at.address()
            ),
            count: Some(rows),
            effect: None,
        });
    }

    let name = bare_name(ctx, &dml.table_name, WHAT)?;
    if !internal.contains(&name) {
        return Err(Blocked::InsertExternal.editor_message());
    }
    verify_insert(&plan)?;
    let rows = append_rows(ctx, target_provider(dml)?, &dml.input).await?;

    Ok(StatementOutcome {
        message: format!("Inserted {} into '{name}'", plural(rows as usize, "row")),
        count: Some(rows),
        effect: Some(StoreEffect::RescanTable { name }),
    })
}

/// The provider the plan's own target resolved to — asked of the plan rather than of the catalog
/// a second time, so what was gated is what is written to.
///
/// The SQL planner wraps a registered provider in a `DefaultTableSource` on the way into a plan,
/// so the downcast holds for anything a typed statement can reach; stated rather than unwrapped
/// because a hand-built plan could carry another `TableSource`, which DataFusion's own arm
/// answers too.
fn target_provider(dml: &DmlStatement) -> Result<Arc<dyn TableProvider>, String> {
    dml.target
        .downcast_ref::<DefaultTableSource>()
        .map(|source| Arc::clone(&source.table_provider))
        .ok_or_else(|| format!("'{}' cannot be written to", dml.table_name))
}

/// The `SQLOptions` floor an `INSERT` runs under — one call, so the local arm and the remote one
/// cannot come to run under different ones.
fn verify_insert(plan: &LogicalPlan) -> Result<(), String> {
    SQLOptions::new()
        .with_allow_dml(true)
        .with_allow_ddl(false)
        .with_allow_statements(false)
        .verify_plan(plan)
        .map_err(|e| e.to_string())
}

/// The table a typed `DROP TABLE` names, dropped — the statement half of [`drop_table`] (ED-05).
pub async fn drop_statement(
    ctx: &SessionContext,
    root: &DataRoot,
    internal: &InternalTables,
    stmt: DFStatement,
) -> Result<StatementOutcome, String> {
    let plan = ctx
        .state()
        .statement_to_plan(stmt)
        .await
        .map_err(|e| e.to_string())?;
    let LogicalPlan::Ddl(DdlStatement::DropTable(drop)) = plan else {
        return Err(format!(
            "{} did not plan as a table drop",
            StmtKind::DropTable.label()
        ));
    };
    drop_table(
        ctx,
        root,
        internal,
        &bare_name(ctx, &drop.name, WHAT)?,
        drop.if_exists,
    )
    .await
}

/// Drop the registered table `name`: deregister the provider, delete the data **if the data is
/// ours**, and answer with the sentence the user reads and the effect the app folds.
///
/// **Deregister first.** No plan built after that can resolve the name, while a scan already
/// running holds its own provider — it finishes against open files, or fails exactly as cleanly
/// as one whose snapshot was retired. The other order would delete the files under a plan still
/// allowed to go looking for them. And because the destroying step comes second, a failure there
/// puts the provider back rather than reporting a drop that failed *after* half of it landed.
///
/// **No cascade.** Dependent views are *named*, never dropped: a `ViewTable`'s plan was inlined
/// when it was created and goes on executing until reload, so nothing is stale yet — and the
/// catalog epoch the fold bumps makes every tab's diagnostics re-derive at once, which is the
/// surface that actually tells the user.
pub async fn drop_table(
    ctx: &SessionContext,
    root: &DataRoot,
    internal: &InternalTables,
    name: &str,
    if_exists: bool,
) -> Result<StatementOutcome, String> {
    let origin = match existing(ctx, name).await {
        Some(TableType::View) => return Err(format!("'{name}' is a view. Use DROP VIEW")),
        Some(_) if internal.contains(name) => TableOrigin::Internal,
        Some(_) => TableOrigin::External,
        None if if_exists => {
            return Ok(StatementOutcome {
                message: format!("Table '{name}' does not exist"),
                count: None,
                effect: None,
            })
        }
        None => return Err(format!("Table '{name}' does not exist")),
    };
    let data = match origin {
        TableOrigin::Internal => {
            let root = root.as_ref().ok_or_else(|| {
                format!("Table '{name}' has no project folder to delete its data from")
            })?;
            Some((tables_dir(root), table_dir(root, name)))
        }
        TableOrigin::External => None,
    };
    let dependents = dependent_views(ctx, name).await;

    let provider = ctx.deregister_table(name).map_err(|e| e.to_string())?;
    if let Some((tables, dir)) = data.filter(|(_, dir)| dir.exists()) {
        if let Err(e) = discard(&tables, &dir) {
            if let Some(provider) = provider {
                if let Err(put_back) = ctx.register_table(name, provider) {
                    tracing::error!(
                        "could not re-register '{name}' after a failed drop: {put_back}"
                    );
                }
            }
            return Err(e);
        }
    }

    Ok(StatementOutcome {
        message: drop_report(name, origin, &dependents),
        count: None,
        effect: Some(StoreEffect::TableRemoved {
            name: name.to_string(),
            dependents,
        }),
    })
}

/// Destroy the internal table directory `dir`, a child of `tables` — **by rename first**.
///
/// The spool publishes by rename; this discards by rename, and for the mirror-image reason. A
/// `remove_dir_all` walks the directory in place, so anything that interrupts it — a killed
/// process, a permission failure partway down, a window torn down while the delete runs on a
/// background thread — leaves a half-emptied directory under the table's *real* name, which
/// nothing collects: the def naming it is already gone, and `project::tidy_strata_dir` sweeps
/// only `.tmp-…`. The rename is a single atomic step within one directory, so the moment it
/// returns the data is unreachable under that name whatever happens next, and whatever is left
/// is exactly what the sweep already exists to collect.
///
/// **The rename is the operation; the delete is housekeeping.** A failure to remove the moved
/// directory is litter, not a failed drop — the table is gone either way — so it is logged and
/// not reported, or the app would tell the user a drop failed that plainly succeeded.
fn discard(tables: &Path, dir: &Path) -> Result<(), String> {
    let aside = tables.join(temp_dir_name());
    fs::rename(dir, &aside).map_err(|e| format!("{}: {e}", dir.display()))?;
    if let Err(e) = fs::remove_dir_all(&aside) {
        tracing::warn!(
            "could not remove {} after dropping its table ({e}); the .strata sweep will",
            aside.display()
        );
    }
    Ok(())
}

/// What dropping a table of this origin **will** do — the catalog confirm's body copy.
///
/// Beside [`drop_report`], which is the same fact in the past tense, because the two must never
/// disagree about whether the files go: the confirm is asking permission for exactly what the
/// report then describes, and the moment an internal table could be dropped from the pane, a
/// fixed "the source files are not deleted" became a reassurance offered at the one moment the
/// action is destructive.
pub fn drop_intent(origin: TableOrigin) -> &'static str {
    match origin {
        TableOrigin::Internal => {
            "Removes the table from this project. Its data files under '.strata' are deleted \
             with it."
        }
        TableOrigin::External => {
            "Unregisters the table from this project. The source files on disk are not deleted."
        }
    }
}

/// What a completed drop reports.
fn drop_report(name: &str, origin: TableOrigin, dependents: &[String]) -> String {
    let message = match origin {
        TableOrigin::Internal => format!("Table '{name}' and its data were deleted"),
        TableOrigin::External => {
            format!("Table '{name}' removed from the catalog. Source files were not deleted")
        }
    };
    message + &left_invalid(dependents)
}

/// The directory name `name`'s data lives in — the name→directory mapping, in one place so a
/// create and a drop cannot disagree about where a table's files are.
fn table_slug(name: &str) -> String {
    slug(&fold_ident(name))
}

/// The directory holding `name`'s data, absolute — [`table_slug`] under the project's `tables/`.
/// What a drop deletes, reached from a name because that is all a `DROP TABLE` carries.
fn table_dir(root: &Path, name: &str) -> PathBuf {
    tables_dir(root).join(table_slug(name))
}

/// Write `input`'s result under `tables/<slug>/`, returning the rows written.
///
/// **Published by rename**, the discipline the snapshot writer already keeps: the files are
/// spooled into a `.tmp-…` sibling and the whole directory is moved into place in one step, so a
/// crash mid-spool leaves nothing but a temp directory the next `.strata` write sweeps
/// (`project::tidy_strata_dir`) rather than a half-written table registered under a real name.
///
/// The destination is a **slug under `tables`** rather than a path of the caller's choosing,
/// because that is what makes the publish a rename at all: the staging directory is a sibling, so
/// the move is within one filesystem and atomic. A caller free to name any destination could ask
/// for one across a mount point and lose the whole spool to `EXDEV` at the last step.
async fn spool(
    ctx: &SessionContext,
    input: &Arc<LogicalPlan>,
    tables: &Path,
    slug: &str,
) -> Result<u64, String> {
    fs::create_dir_all(tables).map_err(|e| format!("{}: {e}", tables.display()))?;
    let mut staging = Staging::open(tables)?;

    let rows = publish(ctx, input, &staging.dir, &tables.join(slug)).await?;
    staging.published();
    Ok(rows)
}

/// The `.tmp-…` directory a spool fills, removed on **every** way out that is not a successful
/// rename — an error, and a **cancel**.
///
/// The cancel is why this is a guard rather than an `if published.is_err()`: a CTAS is registered
/// as the workspace's in-flight call, so `Engine::cancel` and a re-press both abort the task, and
/// an aborted task's future is *dropped* at its next await — no error path runs. Without this,
/// every cancelled CTAS would leave its partial spool behind, and
/// [`sweep_stale_temp_dirs`](strata_core::util::sweep_stale_temp_dirs) deliberately never touches this
/// process's own directories, so nothing would clear them for the life of the window. Cancelling
/// a large CTAS twice is enough to notice. The snapshot writer has the same rule from the other
/// side (`Engine::query` retires again once its handle reports cancelled).
struct Staging {
    dir: PathBuf,
    armed: bool,
}

impl Staging {
    fn open(tables: &Path) -> Result<Staging, String> {
        let dir = tables.join(temp_dir_name());
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Staging { dir, armed: true })
    }

    /// The directory was renamed into place, so it is no longer ours to remove.
    fn published(&mut self) {
        self.armed = false;
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

/// Fill `tmp` and move it onto `dest`.
async fn publish(
    ctx: &SessionContext,
    input: &Arc<LogicalPlan>,
    tmp: &Path,
    dest: &Path,
) -> Result<u64, String> {
    let rows = write_into(ctx, input, tmp).await?;
    if dest.exists() {
        fs::remove_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    fs::rename(tmp, dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    Ok(rows)
}

/// Drive the `COPY … TO <dir> STORED AS ARROW` that does the writing, and guarantee the
/// directory holds a file afterwards.
///
/// The sink creates a file per output partition **when a batch arrives**, so a query that
/// produces no rows — and a `CREATE TABLE t (a INT)`, whose plan is an empty relation — writes
/// nothing at all, and a `ListingTable` over an empty directory cannot infer a schema. One empty
/// IPC file closes that: Arrow IPC self-describes, so the table's columns come back on replay
/// from the file rather than from a schema copied into the def.
async fn write_into(
    ctx: &SessionContext,
    input: &Arc<LogicalPlan>,
    dir: &Path,
) -> Result<u64, String> {
    use datafusion::logical_expr::dml::CopyTo;

    let copy = CopyTo::new(
        Arc::clone(input),
        dir_path(dir),
        Vec::new(),
        format_as_file_type(Arc::new(ArrowFormatFactory::new())),
        HashMap::new(),
    );
    let batches = DataFrame::new(ctx.state(), LogicalPlan::Copy(copy))
        .collect()
        .await
        .map_err(|e| e.to_string())?;
    let rows = copy_row_count(&batches) as u64;

    if !holds_a_file(dir) {
        let path = dir.join("part-0.arrow");
        let file = fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let schema = input.schema().inner().clone();
        let mut writer = FileWriter::try_new_with_options(file, &schema, ipc_write_options()?)
            .map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())?;
    }
    Ok(rows)
}

/// Whether the spool wrote anything into `dir`.
fn holds_a_file(dir: &Path) -> bool {
    fs::read_dir(dir)
        .map(|mut entries| entries.any(|e| e.is_ok_and(|e| e.path().is_file())))
        .unwrap_or(false)
}

/// A directory as the writer and the reader both have to name it: with a trailing separator.
/// Without it `ListingTableUrl::parse` reads the path as a single **file**, which turns a
/// directory sink into one file called `<slug>` and a directory listing into a miss.
fn dir_path(dir: &Path) -> String {
    format!("{}/", dir.display())
}

/// The directory name that holds `name`'s data — the folded table name where that is already a
/// safe file name, and a sanitized form plus a short hash of the original where it is not.
///
/// The hash is what keeps the mapping injective: `sales eu` and `sales/eu` both sanitize to
/// `sales_eu`, and two tables sharing a directory would overwrite each other's data. It is only
/// paid by a name that needed sanitizing, so the ordinary table's directory is simply its name —
/// which matters, because that path is written into `project.json` and read by people.
///
/// **Injective within each half, and the halves can in principle meet**: `sales eu` slugs to
/// `sales_eu-<hash>`, and a table literally named `sales_eu-<that same hash>` is all legal
/// characters, so it takes the shortcut and lands in the same directory. Hashing safe names that
/// *look* hashed would close that, and it is deliberately not done — this function's answer is
/// the directory an **existing** table's data is already in, and `table_dir` re-derives it from
/// the name on every drop. Changing the rule would therefore move the slug of tables already on
/// disk, whose drop would then delete a path that does not exist and orphan the real one forever
/// (the ED-05 failure the one-drop funnel exists to prevent). A collision that needs a user to
/// name one table the hash of another is the smaller hazard, and it is the one that stays.
fn slug(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| match c {
            c if c.is_ascii_alphanumeric() => c,
            '_' | '-' => c,
            _ => '_',
        })
        .collect();
    if safe == name && !name.is_empty() {
        return safe;
    }
    format!("{safe}-{:08x}", hash32(name))
}

/// FNV-1a, folded to 32 bits — a **stable** hash, which `DefaultHasher` is not: its seed is an
/// implementation detail of the standard library, and a slug is written into `project.json` and
/// has to name the same directory next year.
fn hash32(s: &str) -> u32 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h ^ (h >> 32)) as u32
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::{env, process};

    use crate::register::{register_project, table_spec, RegOutcome};
    use crate::sql::Blocked;
    use crate::{Engine, RunOutcome, RunTag, StatementReport, WsId};
    use strata_core::project::{load_defs, save_defs, ProjectDefs};

    use super::*;

    /// A scratch project folder of our own, per test — the tag is load-bearing because these
    /// run concurrently in one process and each engine re-LISTs its own sources.
    fn scratch(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata_internal_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        save_defs(&dir, &ProjectDefs::default()).unwrap();
        dir
    }

    /// An engine pointed at `root`, exactly as a host points one (`Engine::set_data_dir`).
    fn engine(root: &Path, overrides: BTreeMap<String, String>) -> Engine {
        let eng = Engine::new(overrides);
        eng.set_data_dir(root);
        eng
    }

    /// Run one statement and take its report — anything else is a test that asked the wrong
    /// question.
    async fn statement(eng: &Engine, sql: &str) -> Result<StatementReport, String> {
        match eng.run(WsId(1), RunTag(1), sql.into(), 10).await? {
            RunOutcome::Statement(report) => Ok(report),
            RunOutcome::Rows(..) => panic!("{sql} ran as a query"),
        }
    }

    /// The values `name` holds now, as text, through an ordinary query — which is the point:
    /// a created table has to be readable the way any other table is.
    async fn read(eng: &Engine, sql: &str) -> Vec<Vec<String>> {
        let RunOutcome::Rows(output, _) = eng
            .run(WsId(2), RunTag(2), sql.into(), 100)
            .await
            .expect("query")
        else {
            panic!("{sql} did not return rows");
        };
        output
            .rows
            .into_iter()
            .map(|row| row.into_iter().map(|cell| cell.text).collect())
            .collect()
    }

    /// **The end to end shape.** A CTAS spools its result under `.strata/tables/`, registers it,
    /// and answers with the pair the app folds: a portable, internal def and the meta that lands
    /// on its row. The rows are exact because the Arrow footer says so, and the table is
    /// queryable in the same breath.
    #[tokio::test]
    async fn a_ctas_writes_registers_and_reports_an_internal_table() {
        let root = scratch("ctas");
        let eng = engine(&root, BTreeMap::new());

        let report = statement(
            &eng,
            "CREATE TABLE daily AS SELECT * FROM (VALUES (1, 'a'), (2, 'b'), (3, 'c')) AS t(n, w)",
        )
        .await
        .expect("created");

        assert_eq!(report.message, "Table 'daily' created, 3 rows");
        assert_eq!(report.count, Some(3));
        let Some(StoreEffect::TableUpserted { def, meta }) = report.effect else {
            panic!("{:?}", report.effect);
        };
        assert_eq!(def.origin, TableOrigin::Internal);
        assert_eq!(def.format, SourceFormat::Arrow);
        assert_eq!(def.sources, vec![".strata/tables/daily/".to_string()]);
        assert_eq!(meta.rows, Some(3), "read from the IPC footer");
        assert_eq!(
            meta.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["n", "w"]
        );
        assert!(eng.is_internal("daily"), "a write statement may target it");

        assert_eq!(
            read(&eng, "SELECT w FROM daily ORDER BY n").await,
            vec![vec!["a"], vec!["b"], vec!["c"]]
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A **column list with no query** is a table with a schema and no rows — which needs a
    /// file all the same, because the schema comes back on replay from the IPC and nowhere
    /// else. Same for a query that matches nothing.
    #[tokio::test]
    async fn an_empty_table_still_carries_its_schema() {
        let root = scratch("empty");
        let eng = engine(&root, BTreeMap::new());

        let report = statement(&eng, "CREATE TABLE blank (a INT, b VARCHAR)")
            .await
            .expect("created");
        assert_eq!(report.message, "Table 'blank' created, 0 rows");
        let Some(StoreEffect::TableUpserted { meta, .. }) = &report.effect else {
            panic!("{:?}", report.effect);
        };
        assert_eq!(meta.rows, Some(0));
        assert_eq!(
            meta.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(read(&eng, "SELECT a FROM blank").await.is_empty());

        statement(&eng, "CREATE TABLE nothing AS SELECT 1 AS n WHERE false")
            .await
            .expect("created");
        assert!(read(&eng, "SELECT n FROM nothing").await.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    /// The three name semantics, on one namespace: `IF NOT EXISTS` reports and changes
    /// nothing, a plain create over a taken name errors, `OR REPLACE` replaces the data as
    /// well as the def, and a **view**'s name is refused outright rather than quietly turned
    /// into a table.
    #[tokio::test]
    async fn existing_names_resolve_before_anything_is_written() {
        let root = scratch("names");
        let eng = engine(&root, BTreeMap::new());
        statement(&eng, "CREATE TABLE t AS SELECT 1 AS n")
            .await
            .expect("created");

        let noop = statement(&eng, "CREATE TABLE IF NOT EXISTS t AS SELECT 2 AS n")
            .await
            .expect("reported");
        assert_eq!(noop.message, "Table 't' already exists");
        assert_eq!(noop.effect, None, "nothing for the store to fold");
        assert_eq!(read(&eng, "SELECT n FROM t").await, vec![vec!["1"]]);

        assert_eq!(
            statement(&eng, "CREATE TABLE t AS SELECT 2 AS n")
                .await
                .expect_err("taken"),
            "Table 't' already exists"
        );

        let replaced = statement(&eng, "CREATE OR REPLACE TABLE t AS SELECT 2 AS n, 3 AS m")
            .await
            .expect("replaced");
        assert_eq!(replaced.message, "Table 't' replaced, 1 row");
        assert_eq!(read(&eng, "SELECT n, m FROM t").await, vec![vec!["2", "3"]]);

        let fresh = statement(&eng, "CREATE OR REPLACE TABLE brand_new AS SELECT 1 AS n")
            .await
            .expect("created");
        assert_eq!(fresh.message, "Table 'brand_new' created, 1 row");

        eng.create_view("v".into(), "SELECT 1 AS n".into())
            .await
            .expect("view");
        assert_eq!(
            statement(&eng, "CREATE OR REPLACE TABLE v AS SELECT 1 AS n")
                .await
                .expect_err("a view is not a table"),
            "'v' is a view"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// The clauses that are refused, each for its own reason: DataFusion's planner answers for
    /// its own unimplemented ones, and the two it *plans* without enforcing are ours.
    #[tokio::test]
    async fn unsupported_clauses_refuse_before_anything_is_written() {
        let root = scratch("clauses");
        let eng = engine(&root, BTreeMap::new());

        let temporary = statement(&eng, "CREATE TEMPORARY TABLE t AS SELECT 1 AS n")
            .await
            .expect_err("refused");
        assert!(temporary.contains("Temporary"), "{temporary}");

        assert_eq!(
            statement(&eng, "CREATE TABLE t (a INT PRIMARY KEY)")
                .await
                .expect_err("refused"),
            "Table constraints are not supported"
        );
        assert_eq!(
            statement(&eng, "CREATE TABLE t (a INT DEFAULT 1)")
                .await
                .expect_err("refused"),
            "Column defaults are not supported"
        );
        assert_eq!(
            statement(
                &eng,
                "CREATE TABLE t AS SELECT * FROM (VALUES (1)) AS a(i) \
                 JOIN (VALUES (1)) AS b(i) ON a.i = b.i"
            )
            .await
            .expect_err("refused"),
            "Duplicate column name 'i'. Alias one of them"
        );

        assert!(
            !tables_dir(&root).join("t").exists(),
            "a refusal writes nothing"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **The type probe answers what the create will actually produce** (IT-01) — in the
    /// spelling every other surface shows a type in, on this session's own config, and with the
    /// planner's refusal verbatim where there is one.
    ///
    /// The two config-dependent answers are the reason the offer cannot be a static table: the
    /// same `TIMESTAMP WITH TIME ZONE` reaches a different Arrow type depending on
    /// `execution.time_zone`, and the empty-table panel promises what the user is about to get.
    #[tokio::test]
    async fn the_type_probe_answers_with_the_planners_own_arrow_type() {
        let ctx = crate::build_context(&BTreeMap::new());

        assert_eq!(column_type(&ctx, "INT").await.unwrap(), "Int32");
        assert_eq!(column_type(&ctx, "INTEGER").await.unwrap(), "Int32");
        assert_eq!(column_type(&ctx, " double ").await.unwrap(), "Float64");
        assert_eq!(column_type(&ctx, "BYTEA").await.unwrap(), "Binary");

        let refused = column_type(&ctx, "FLOAT64").await.expect_err("refused");
        assert!(refused.contains("FLOAT64"), "{refused}");

        assert_eq!(
            column_type(&ctx, "INT PRIMARY KEY")
                .await
                .expect_err("refused"),
            "Table constraints are not supported"
        );
        assert_eq!(
            column_type(&ctx, "INT DEFAULT 1")
                .await
                .expect_err("refused"),
            "Column defaults are not supported"
        );

        assert_eq!(
            column_type(&ctx, "INT, b INT")
                .await
                .expect_err("not one type"),
            "'INT, b INT' is not a column type"
        );
        let smuggled = "INT) AS SELECT 1 --";
        assert_eq!(
            column_type(&ctx, smuggled)
                .await
                .expect_err("carries a query"),
            format!("'{smuggled}' is not a column type")
        );
        assert!(column_type(&ctx, "  ").await.is_err(), "nothing to ask");
    }

    /// **The probe's answer is the type the table then carries.** Two readings of one session,
    /// which is the whole promise the panel makes: what its row said is what the inspector shows.
    #[tokio::test]
    async fn the_probe_and_the_created_table_agree() {
        let root = scratch("probe");
        let eng = engine(
            &root,
            BTreeMap::from([(
                "datafusion.execution.time_zone".to_string(),
                "Europe/London".to_string(),
            )]),
        );

        let probed = eng
            .column_type("TIMESTAMP WITH TIME ZONE".into())
            .await
            .expect("planned");
        let report = statement(&eng, "CREATE TABLE t (\"at\" TIMESTAMP WITH TIME ZONE)")
            .await
            .expect("created");
        let Some(StoreEffect::TableUpserted { meta, .. }) = &report.effect else {
            panic!("{:?}", report.effect);
        };
        assert_eq!(meta.columns[0].dtype, probed);
        let _ = fs::remove_dir_all(&root);
    }

    /// **The reserved namespace, from both directions.** The router refuses a typed
    /// `__snap_`-named target, and the registration funnel refuses a def that arrived any other
    /// way — the same class of error, so a hand-edited `project.json` cannot do what a
    /// statement cannot.
    #[tokio::test]
    async fn a_reserved_name_is_refused_by_the_router_and_by_the_funnel() {
        let root = scratch("reserved");
        let eng = engine(&root, BTreeMap::new());
        let reserved = Blocked::ReservedName.editor_message();

        assert_eq!(
            statement(&eng, "CREATE TABLE __snap_1 (a INT)")
                .await
                .expect_err("refused"),
            reserved
        );
        assert_eq!(
            eng.register(TableSpec {
                name: "__snap_1".into(),
                paths: vec![root.display().to_string()],
                format: SourceFormat::Arrow,
                partitions: Vec::new(),
                internal: true,
            })
            .await
            .expect_err("refused"),
            reserved
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **Replay is the ordinary pass.** The def written by a CTAS goes into `project.json`,
    /// and a cold engine — the headless host's, here — registers it back with no code of its
    /// own, schema and row count read from the IPC files.
    #[tokio::test]
    async fn an_internal_def_replays_through_the_registration_pass() {
        let root = scratch("replay");
        let report = {
            let eng = engine(&root, BTreeMap::new());
            statement(
                &eng,
                "CREATE TABLE kept AS SELECT * FROM (VALUES (1), (2)) AS t(n)",
            )
            .await
            .expect("created")
        };
        let Some(StoreEffect::TableUpserted { def, .. }) = report.effect else {
            panic!("{:?}", report.effect);
        };
        save_defs(
            &root,
            &ProjectDefs {
                tables: vec![def],
                ..Default::default()
            },
        )
        .unwrap();

        let cold = Engine::new(BTreeMap::new());
        let defs = load_defs(&root).unwrap();
        let mut out = Vec::new();
        register_project(&cold, &root, &defs, |o| out.push(o)).await;

        match &out[..] {
            [RegOutcome::Table {
                name,
                result: Ok(meta),
            }] => {
                assert_eq!(name, "kept");
                assert_eq!(meta.rows, Some(2));
                assert_eq!(meta.columns[0].name, "n");
            }
            other => panic!("{other:?}"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// **A clone without the data is honest about why.** `tables/` is gitignored, so a
    /// colleague gets the def and no files — and the external vocabulary ("no source at …")
    /// would send them looking for a path to repair that was never theirs.
    #[tokio::test]
    async fn a_project_copied_without_its_table_data_names_the_real_cause() {
        let root = scratch("clone");
        let eng = engine(&root, BTreeMap::new());
        let report = statement(&eng, "CREATE TABLE gone AS SELECT 1 AS n")
            .await
            .expect("created");
        let Some(StoreEffect::TableUpserted { def, .. }) = report.effect else {
            panic!("{:?}", report.effect);
        };
        fs::remove_dir_all(tables_dir(&root)).unwrap();

        let cold = Engine::new(BTreeMap::new());
        let error = cold
            .register(table_spec(&root, &def))
            .await
            .expect_err("no data");

        assert_eq!(
            error,
            "Table 'gone' has no data in this copy of the project. An internal table's data is \
             local to the machine that created it."
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **The two counts have to agree.** The sink reports what it wrote and the footers report
    /// what is there, and the row count on the catalog row is the second — so a spool split
    /// across several files and several batches per file must still total exactly. (The footer
    /// arithmetic itself is unit-tested in `engine::arrow_stats`, over a file whose batch
    /// boundaries are chosen rather than whatever the planner picked.)
    #[tokio::test]
    async fn the_sinks_count_and_the_footers_agree() {
        let root = scratch("batches");
        let eng = engine(
            &root,
            BTreeMap::from([(
                "datafusion.execution.batch_size".to_string(),
                "2".to_string(),
            )]),
        );

        let report = statement(
            &eng,
            "CREATE TABLE many AS SELECT * FROM (VALUES (1),(2),(3),(4),(5),(6),(7)) AS t(n)",
        )
        .await
        .expect("created");

        let Some(StoreEffect::TableUpserted { meta, .. }) = &report.effect else {
            panic!("{:?}", report.effect);
        };
        assert_eq!(report.count, Some(7), "the sink's own count");
        assert_eq!(meta.rows, Some(7), "and the footers agree with it");

        let files = fs::read_dir(tables_dir(&root).join("many"))
            .unwrap()
            .count();
        assert!(files >= 1, "the spool wrote its files into the directory");
        assert_eq!(
            read(&eng, "SELECT count(*) AS c FROM many").await,
            vec![vec!["7"]],
            "and a scan reads every one of the {files} files"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// Nothing is published until the whole spool is written: the staging directory is a
    /// `.tmp-` sibling, so a killed process leaves something the `.strata` housekeeping sweeps
    /// rather than a half-written table registered under a real name.
    #[tokio::test]
    async fn a_finished_spool_leaves_no_staging_directory_behind() {
        let root = scratch("staging");
        let eng = engine(&root, BTreeMap::new());
        statement(&eng, "CREATE TABLE t AS SELECT 1 AS n")
            .await
            .expect("created");

        assert_eq!(entries(&tables_dir(&root)), ["t"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// **A cancelled CTAS takes its staging directory with it.** A cancel aborts the task, so the
    /// future is *dropped* mid-await and no error path runs — and the sweep never touches this
    /// process's own `.tmp-` directories, so anything left here would sit under `.strata/tables`
    /// for the life of the window. Cancelling a large CTAS a few times is all it takes.
    ///
    /// Driven by dropping the future rather than by racing a real cancel, because that is exactly
    /// what `tokio`'s abort does to it and it is the state under test.
    #[tokio::test]
    async fn a_cancelled_spool_takes_its_staging_directory_with_it() {
        let root = scratch("cancelled");
        let ctx = crate::build_context(&BTreeMap::new());
        let tables = tables_dir(&root);
        fs::create_dir_all(&tables).unwrap();

        let plan = Arc::new(
            ctx.sql("SELECT * FROM generate_series(1, 5000000)")
                .await
                .expect("plan")
                .logical_plan()
                .clone(),
        );
        let mut spooling = Box::pin(spool(&ctx, &plan, &tables, "big"));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(
            std::future::Future::poll(spooling.as_mut(), &mut cx).is_pending(),
            "the spool has started and not finished"
        );
        assert_eq!(entries(&tables).len(), 1, "its staging directory is there");

        drop(spooling);

        assert!(
            entries(&tables).is_empty(),
            "and dropping the future removed it: {:?}",
            entries(&tables)
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A directory's entries, sorted.
    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// An ordinary name is its own directory — the path lands in a committed `project.json` and
    /// is meant to be readable. Anything a file name cannot carry is sanitized *and* hashed, so
    /// two names that sanitize alike cannot land in one directory.
    #[test]
    fn a_slug_is_the_name_where_it_can_be_and_never_collides_where_it_cannot() {
        assert_eq!(slug("orders"), "orders");
        assert_eq!(slug("daily_totals_2024"), "daily_totals_2024");
        assert_eq!(slug("sales-eu"), "sales-eu");

        assert_ne!(slug("sales eu"), slug("sales/eu"), "both sanitize alike");
        assert!(slug("sales eu").starts_with("sales_eu-"));
        assert!(!slug(".tmp-1-0").starts_with('.'));
    }

    /// The hash is stable by construction, not by whatever the standard library seeds today:
    /// the slug is written into `project.json`, so a value that moved between builds would
    /// orphan every internal table in every existing project.
    #[test]
    fn the_slug_hash_is_pinned() {
        assert_eq!(hash32(""), 0x4fd0_bfc1);
        assert_eq!(slug("sales eu"), "sales_eu-2dc32f1b");
    }

    /// Register `name` as an **external** table over `paths` — the user's own files, as far as
    /// the engine is concerned.
    ///
    /// Built from an internal table's own directory rather than from a written fixture: the
    /// point of an external table here is only that Strata does not own it, and the origin is a
    /// flag on the registration, so the cheapest honest one is a real directory of real files
    /// registered a second time under another name.
    async fn external(eng: &Engine, name: &str, dir: &Path) {
        eng.register(TableSpec {
            name: name.into(),
            paths: vec![dir_path(dir)],
            format: SourceFormat::Arrow,
            partitions: Vec::new(),
            internal: false,
        })
        .await
        .expect("registered");
    }

    /// **The whole ED-05 shape, end to end.** A table is created, inserted into twice — each
    /// statement appending its own file — read back as the union, and then dropped, taking its
    /// data directory with it.
    #[tokio::test]
    async fn inserts_append_files_and_a_drop_takes_the_data_with_it() {
        let root = scratch("insert");
        let eng = engine(&root, BTreeMap::new());
        statement(&eng, "CREATE TABLE t AS SELECT * FROM (VALUES (1)) AS v(n)")
            .await
            .expect("created");
        let dir = tables_dir(&root).join("t");
        let after_create = entries(&dir).len();

        let first = statement(&eng, "INSERT INTO t VALUES (2)")
            .await
            .expect("inserted");
        assert_eq!(first.message, "Inserted 1 row into 't'");
        assert_eq!(first.count, Some(1));
        assert_eq!(
            first.effect,
            Some(StoreEffect::RescanTable { name: "t".into() }),
            "the row count is the scan driver's to re-read, never the store's to add up"
        );

        let second = statement(
            &eng,
            "INSERT INTO t SELECT * FROM (VALUES (3), (4)) AS v(n)",
        )
        .await
        .expect("inserted");
        assert_eq!(second.message, "Inserted 2 rows into 't'");

        assert_eq!(
            entries(&dir).len(),
            after_create + 2,
            "one file per statement, appended: {:?}",
            entries(&dir)
        );
        assert_eq!(
            read(&eng, "SELECT n FROM t ORDER BY n").await,
            vec![vec!["1"], vec!["2"], vec!["3"], vec!["4"]],
            "and a scan reads the union of every file"
        );

        let dropped = statement(&eng, "DROP TABLE t").await.expect("dropped");
        assert_eq!(dropped.message, "Table 't' and its data were deleted");
        assert_eq!(dropped.count, None, "a drop moves no rows");
        assert_eq!(
            dropped.effect,
            Some(StoreEffect::TableRemoved {
                name: "t".into(),
                dependents: Vec::new(),
            })
        );
        assert!(!dir.exists(), "the data directory went with it");
        assert!(
            entries(&tables_dir(&root)).is_empty(),
            "{:?}",
            entries(&tables_dir(&root))
        );
        assert!(!eng.is_internal("t"), "and it is no longer a write target");
        let _ = fs::remove_dir_all(&root);
    }

    /// **The data is unreachable under the table's name before anything is deleted.** The
    /// discard renames the directory into a `.tmp-…` sibling and only then walks it, so an
    /// interruption — a killed process, a permission failure partway down, a window torn down
    /// while the delete runs — leaves something `project::tidy_strata_dir` collects rather than
    /// a half-emptied directory under a real table name that nothing ever will. Deleting in
    /// place had no such point: the def is gone by then, so nothing would point at the remains.
    ///
    /// **A drop that reports a failure has not half-happened.** The deregister comes first so
    /// nothing can plan against a table whose files are going, which means the one step that can
    /// still fail runs after it — and a `discard` that cannot even start (its first act is the
    /// rename) has destroyed nothing, so the provider goes back.
    ///
    /// Without the restore the user is told the drop failed while the table is already gone from
    /// the session: the def stays in `project.json` and on the sidebar, and every query against
    /// it answers "table not found" until a re-scan nobody has a reason to run.
    ///
    /// A read-only `tables/` is what makes the rename fail — renaming a child needs write on its
    /// parent — which is the fault the sibling test above could not reach by locking the child.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_drop_that_cannot_discard_leaves_the_table_exactly_as_it_was() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("discard-refused");
        let eng = engine(&root, BTreeMap::new());
        statement(&eng, "CREATE TABLE t AS SELECT 1 AS n")
            .await
            .expect("created");
        let tables = tables_dir(&root);
        fs::set_permissions(&tables, fs::Permissions::from_mode(0o500)).unwrap();

        let error = statement(&eng, "DROP TABLE t")
            .await
            .expect_err("the data could not be moved out of the way");

        fs::set_permissions(&tables, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(error.contains("Permission denied"), "{error}");
        assert_eq!(
            read(&eng, "SELECT n FROM t").await,
            vec![vec!["1"]],
            "the table the drop said it could not drop is still there"
        );
        assert!(eng.is_internal("t"), "…and still a write target");
        assert!(tables.join("t").exists(), "…with its data where it was");
        let _ = fs::remove_dir_all(&root);
    }

    /// Driven at [`discard`] directly, with the *removal* made to fail while the rename can
    /// still land — a read-only directory refuses `unlink` of what it holds, and the rename
    /// needs write on `tables/` only. That is the shape of every interruption this exists for:
    /// the rename landed, the walk did not.
    #[cfg(unix)]
    #[test]
    fn a_discard_that_cannot_finish_still_takes_the_table_out_of_the_way() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("discard");
        let tables = tables_dir(&root);
        let dir = tables.join("t");
        let locked = dir.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("part-0.arrow"), b"x").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();

        discard(&tables, &dir).expect("the rename is the operation, and it landed");

        let left = entries(&tables);
        for name in &left {
            let _ = fs::set_permissions(
                tables.join(name).join("locked"),
                fs::Permissions::from_mode(0o700),
            );
        }

        assert!(!dir.exists(), "gone from under the table's own name");
        assert!(
            !left.is_empty() && left.iter().all(|name| name.starts_with(".tmp-")),
            "and what survives is only ever a temp the sweep collects: {left:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **An append does not disturb the views above the table, so nothing has to repair them.**
    ///
    /// A `ViewTable` captures its sources by `Arc` when it is created and never re-resolves them
    /// (D10/D11), which is why every path that *re-registers* a table re-creates the views over
    /// it. An `INSERT` replaces no provider, and could not invalidate one if it did: the sink
    /// schema-checks before it writes, so the shape a view captured is the shape still there, and
    /// the provider re-LISTs per scan (this engine runs no `ListFilesCache`) so it finds the new
    /// file on its own. Hence [`Engine::table_meta`] rather than a re-registration — this pins
    /// both halves: the view is right without being touched, and the row count is right without
    /// the table being rebuilt.
    #[tokio::test]
    async fn an_append_reaches_a_view_that_was_never_re_created() {
        let root = scratch("insert-view");
        let eng = engine(&root, BTreeMap::new());
        statement(&eng, "CREATE TABLE t AS SELECT * FROM (VALUES (1)) AS v(n)")
            .await
            .expect("created");
        eng.create_view("reader".into(), "SELECT n FROM t".into())
            .await
            .expect("view");
        assert_eq!(read(&eng, "SELECT n FROM reader").await, vec![vec!["1"]]);

        statement(&eng, "INSERT INTO t VALUES (2)")
            .await
            .expect("inserted");

        assert_eq!(
            read(&eng, "SELECT n FROM reader ORDER BY n").await,
            vec![vec!["1"], vec!["2"]],
            "the view sees the appended row through the provider it captured"
        );
        assert_eq!(
            eng.table_meta("t".into()).await.expect("meta").rows,
            Some(2),
            "and the row count is re-read without re-registering the table"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **A statement's names are the planner's, not the def's** — and a re-scan request is
    /// therefore not a def name. A quoted `CREATE TABLE` keeps its case in the def; an unquoted
    /// `INSERT INTO` naming the same table folds, so the effect names `mytable` where the def
    /// says `MyTable`. Pinned here because the store is what has to reconcile them
    /// (`ProjectState::same_name`, `plan_scan`), and an exact match there planned an empty pass:
    /// the insert landed and the sidebar's row count silently never moved.
    #[tokio::test]
    async fn a_statement_names_a_table_as_the_planner_folds_it() {
        let root = scratch("insert-fold");
        let eng = engine(&root, BTreeMap::new());
        let created = statement(&eng, "CREATE TABLE \"MyTable\" AS SELECT 1 AS n")
            .await
            .expect("created");
        let Some(StoreEffect::TableUpserted { def, .. }) = &created.effect else {
            panic!("{:?}", created.effect);
        };
        assert_eq!(
            def.name, "MyTable",
            "a quoted name keeps its case in the def"
        );

        let inserted = statement(&eng, "INSERT INTO MyTable VALUES (2)")
            .await
            .expect("inserted");
        assert_eq!(
            inserted.effect,
            Some(StoreEffect::RescanTable {
                name: "mytable".into()
            }),
            "the effect names the table the planner resolved, not the def"
        );
        assert_eq!(
            read(&eng, "SELECT n FROM \"MyTable\" ORDER BY n").await,
            vec![vec!["1"], vec!["2"]],
            "and the write landed on the one table either spelling means"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **A table's per-file statistics land in the runtime's cache**, one entry per file, so a
    /// later scan of an unchanged file is served from it instead of re-reading its footer.
    ///
    /// The wiring is a single `with_cache` on the registration, and dropping it is invisible:
    /// every number stays right and every read costs a metadata round trip again. That is worse
    /// than it sounds here — statistics are collected per **scan** (`free_stats` reaches them
    /// through `list_files_for_scan`) and again per registration, and an `INSERT` asks for a
    /// re-scan, so the *k*th write re-read *k* footers. This asserts the entries exist at all,
    /// which is exactly what the wiring buys; whether a cached entry is then reused is
    /// DataFusion's own contract (`is_valid_for`, on size and mtime).
    #[tokio::test]
    async fn a_tables_footer_statistics_are_cached_per_file() {
        let root = scratch("stats-cache");
        let eng = engine(&root, BTreeMap::new());
        let cache = eng
            .ctx
            .runtime_env()
            .cache_manager
            .get_file_statistic_cache()
            .expect(
                "the runtime builds one by default — unlike the list-files cache, which \
                     ENGINE_KEYS deliberately zeroes",
            );
        assert_eq!(cache.len(), 0, "nothing registered yet");

        statement(&eng, "CREATE TABLE t AS SELECT 1 AS n")
            .await
            .expect("created");
        let after_create = cache.len();
        assert!(
            after_create > 0,
            "the registration's footer reads were cached"
        );

        statement(&eng, "INSERT INTO t VALUES (2)")
            .await
            .expect("inserted");
        assert_eq!(read(&eng, "SELECT n FROM t ORDER BY n").await.len(), 2);
        assert_eq!(
            cache.len(),
            after_create + 1,
            "the scan added the new file and nothing else"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **A write only ever reaches files Strata owns.** An external table and a view are the
    /// same refusal — neither is a directory a `CREATE TABLE` wrote — and nothing is written on
    /// the way to saying so.
    #[tokio::test]
    async fn an_insert_into_anything_strata_does_not_own_is_refused() {
        let root = scratch("insert-gate");
        let eng = engine(&root, BTreeMap::new());
        statement(
            &eng,
            "CREATE TABLE owned AS SELECT * FROM (VALUES (1)) AS v(n)",
        )
        .await
        .expect("created");
        external(&eng, "theirs", &tables_dir(&root).join("owned")).await;
        eng.create_view("v".into(), "SELECT n FROM owned".into())
            .await
            .expect("view");

        for target in ["theirs", "v"] {
            assert_eq!(
                statement(&eng, &format!("INSERT INTO {target} VALUES (2)"))
                    .await
                    .expect_err("refused"),
                Blocked::InsertExternal.editor_message(),
                "{target}"
            );
        }
        assert_eq!(
            read(&eng, "SELECT n FROM theirs").await,
            vec![vec!["1"]],
            "and the refusal wrote nothing"
        );

        assert_eq!(
            statement(&eng, "REPLACE INTO owned VALUES (2)")
                .await
                .expect_err("refused"),
            Blocked::InsertOverwrite.editor_message()
        );
        assert_eq!(read(&eng, "SELECT n FROM owned").await, vec![vec!["1"]]);
        let _ = fs::remove_dir_all(&root);
    }

    /// A row the table cannot take is **DataFusion's** refusal, surfaced as the run error in its
    /// own words — reimplementing the shape check beside its own writer would be a second
    /// opinion about the same file, and one that has to be kept in step with its coercions.
    #[tokio::test]
    async fn an_insert_the_table_cannot_take_fails_with_datafusions_own_check() {
        let root = scratch("insert-schema");
        let eng = engine(&root, BTreeMap::new());
        statement(&eng, "CREATE TABLE t AS SELECT 1 AS n")
            .await
            .expect("created");

        let error = statement(&eng, "INSERT INTO t VALUES (1, 'extra')")
            .await
            .expect_err("refused");
        assert!(error.contains("Inconsistent data length"), "{error}");
        assert_eq!(
            read(&eng, "SELECT n FROM t").await,
            vec![vec!["1"]],
            "and the refusal wrote nothing"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **An external drop takes the def and leaves the files.** The report says so in as many
    /// words — it is the one thing the user is really asking — and the views left reading a
    /// table that is gone are *named*, never dropped with it.
    #[tokio::test]
    async fn dropping_an_external_table_leaves_its_files_and_names_its_readers() {
        let root = scratch("drop-external");
        let eng = engine(&root, BTreeMap::new());
        statement(
            &eng,
            "CREATE TABLE owned AS SELECT * FROM (VALUES (1)) AS v(n)",
        )
        .await
        .expect("created");
        let files = tables_dir(&root).join("owned");
        external(&eng, "theirs", &files).await;
        eng.create_view("direct".into(), "SELECT n FROM theirs".into())
            .await
            .expect("view");
        eng.create_view("nested".into(), "SELECT n FROM direct".into())
            .await
            .expect("view");

        let report = statement(&eng, "DROP TABLE theirs").await.expect("dropped");
        assert_eq!(
            report.message,
            "Table 'theirs' removed from the catalog. Source files were not deleted. \
             2 views are left invalid: 'direct', 'nested'"
        );
        assert_eq!(
            report.effect,
            Some(StoreEffect::TableRemoved {
                name: "theirs".into(),
                dependents: vec!["direct".into(), "nested".into()],
            })
        );
        assert!(files.exists(), "the files were never Strata's to delete");
        assert_eq!(
            read(&eng, "SELECT n FROM owned").await,
            vec![vec!["1"]],
            "and the table that does own them is untouched"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// The two answers for a name that is not there, and the one for a name that is not a
    /// table. `IF EXISTS` reports a no-op with nothing for the store to fold; a plain drop
    /// fails; a view says which statement drops it.
    #[tokio::test]
    async fn a_drop_resolves_its_target_before_it_touches_anything() {
        let root = scratch("drop-names");
        let eng = engine(&root, BTreeMap::new());
        eng.create_view("v".into(), "SELECT 1 AS n".into())
            .await
            .expect("view");

        let noop = statement(&eng, "DROP TABLE IF EXISTS ghost")
            .await
            .expect("reported");
        assert_eq!(noop.message, "Table 'ghost' does not exist");
        assert_eq!(noop.effect, None, "nothing for the store to fold");

        assert_eq!(
            statement(&eng, "DROP TABLE ghost")
                .await
                .expect_err("missing"),
            "Table 'ghost' does not exist"
        );
        assert_eq!(
            statement(&eng, "DROP TABLE v").await.expect_err("a view"),
            "'v' is a view. Use DROP VIEW"
        );
        assert_eq!(
            read(&eng, "SELECT n FROM v").await,
            vec![vec!["1"]],
            "the view is still there"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **The catalog pane's drop and the editor's leave the same state** — the whole point of
    /// the shared funnel, and the one thing a per-surface implementation would pass its own
    /// tests without delivering. Two internal tables, dropped through the two entry points, and
    /// the assertion is on disk.
    #[tokio::test]
    async fn both_entry_points_leave_the_same_state_on_disk() {
        let root = scratch("drop-parity");
        let eng = engine(&root, BTreeMap::new());
        for name in ["typed", "pressed"] {
            statement(&eng, &format!("CREATE TABLE {name} AS SELECT 1 AS n"))
                .await
                .expect("created");
        }

        let typed = statement(&eng, "DROP TABLE typed").await.expect("dropped");
        let pressed = eng
            .drop_table("pressed".into(), true)
            .await
            .expect("dropped");

        assert_eq!(typed.message, "Table 'typed' and its data were deleted");
        assert_eq!(pressed.message, "Table 'pressed' and its data were deleted");
        assert!(
            entries(&tables_dir(&root)).is_empty(),
            "neither left its data behind: {:?}",
            entries(&tables_dir(&root))
        );
        for name in ["typed", "pressed"] {
            assert!(!eng.is_internal(name), "{name} is no longer a write target");
            assert!(
                eng.run(WsId(3), RunTag(3), format!("SELECT n FROM {name}"), 10)
                    .await
                    .is_err(),
                "{name} still resolves"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }
}
