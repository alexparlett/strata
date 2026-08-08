//! **Internal tables** (ED-04) — `CREATE TABLE` and CTAS, spooled into the project's own
//! `.strata/tables/` and registered through the funnel every other table goes through.
//! `docs/STATEMENTS_SPEC.md` §6.1 + §7.
//!
//! # Why this is not DataFusion's CTAS
//!
//! DataFusion's own `CREATE TABLE AS` collects the whole result into RAM as a `MemTable` and
//! registers it from a **sync** hook (`context/mod.rs:868-927`), so there is no point at which a
//! result could be streamed to disk and nothing survives a restart. Both are disqualifying: a
//! table is a durable thing here, and the result may be larger than the window.
//!
//! # What is still DataFusion's, deliberately
//!
//! **The plan.** The statement is handed to `SessionState::statement_to_plan` exactly as parsed —
//! no text is re-rendered and no span is sliced, so the query that runs is the query the user
//! wrote, by construction rather than by fidelity of a round trip. Planning a `CREATE TABLE`
//! executes nothing (execution lives only in `execute_logical_plan`), and it buys two things
//! outright: DataFusion's planner already refuses every clause it does not implement — fifty-odd
//! of them, `TEMPORARY` and `LOCATION` and `PARTITION BY` included, each with its own message —
//! and it already resolves a declared column list against the query, casting and renaming to it.
//! Reimplementing that beside it would be fifty refusals we would have to keep in step.
//!
//! **The write.** The spool is a `CopyTo` node over that plan, `STORED AS ARROW` — DataFusion's
//! Arrow sink, which streams, writes LZ4-frame IPC (the snapshot codec) and reports the exact row
//! count as its single `count` column.
//!
//! What is ours is the part no hook can carry: where the files go, that they are published by
//! rename, what the def says, and the name semantics — `IF NOT EXISTS` / `OR REPLACE` /
//! plain-exists resolved against the one namespace tables and views share.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use datafusion::arrow::ipc::writer::FileWriter;
use datafusion::catalog::TableProvider;
use datafusion::dataframe::DataFrame;
use datafusion::datasource::file_format::arrow::ArrowFormatFactory;
use datafusion::datasource::file_format::format_as_file_type;
use datafusion::logical_expr::{CreateMemoryTable, DdlStatement, LogicalPlan, TableType};
use datafusion::prelude::{SQLOptions, SessionContext};
use datafusion::sql::parser::Statement as DFStatement;
use datafusion::sql::TableReference;

use crate::engine::catalog::{register_external, TableSpec};
use crate::engine::export::copy_row_count;
use crate::engine::query::ipc_write_options;
use crate::engine::sql::StmtKind;
use crate::engine::{fold_ident, CATALOG, SCHEMA};
use crate::project::{internal_source, tables_dir};
use crate::util::{plural, temp_dir_name};
use strata_model::{SourceFormat, TableDef, TableOrigin};

use super::{DataRoot, StatementOutcome, StoreEffect};

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
) -> Result<StatementOutcome, String> {
    let Some(root) = root else {
        // Only reachable on an engine with no project behind it. Polite rather than internal:
        // the statement is perfectly good, there is just nowhere to put the table.
        return Err(format!(
            "{} needs a project folder to store the table's data",
            kind.label()
        ));
    };

    // DataFusion's planner is the clause gate (module doc). Its `not_impl` wording reaches the
    // user as written, which is right: those are its clauses, described in its terms.
    let plan = ctx
        .state()
        .statement_to_plan(stmt)
        .await
        .map_err(|e| e.to_string())?;
    let LogicalPlan::Ddl(DdlStatement::CreateMemoryTable(create)) = plan else {
        // The router classified this statement as one of the two kinds above, and DataFusion
        // plans both into `CreateMemoryTable`. Anything else is the two disagreeing.
        return Err(format!("{} did not plan as a table", kind.label()));
    };
    let CreateMemoryTable {
        name,
        constraints,
        input,
        if_not_exists,
        or_replace,
        column_defaults,
        // `TEMPORARY` never reaches here — DataFusion's own arm refuses it while planning
        // ("Temporary tables not supported"), which is one refusal in one place rather than
        // two that can disagree.
        temporary: _,
    } = create;

    // The two clauses DataFusion *plans* and does not enforce. It does not check a constraint
    // even on its own `MemTable`, so accepting one would be a promise nothing keeps; a column
    // default has no meaning until something can `INSERT` without naming the column, which is
    // ED-05's and needs a provider that applies it.
    if !constraints.is_empty() {
        return Err("Table constraints are not supported".into());
    }
    if !column_defaults.is_empty() {
        return Err("Column defaults are not supported".into());
    }

    let name = bare_name(&name)?;
    // Reproduced from DataFusion's own `ensure_unique_column_names` rule rather than inherited:
    // its CTAS never writes a file, and an IPC file *would* store both columns, after which
    // every read of the table resolves the second by name onto the first.
    let mut seen = Vec::new();
    for field in input.schema().fields() {
        let folded = fold_ident(field.name());
        if seen.contains(&folded) {
            return Err(format!(
                "Duplicate column name '{}'. Alias one of them",
                field.name()
            ));
        }
        seen.push(folded);
    }

    // The one namespace, asked of the engine that owns it. `Reg::Failed` defs are invisible here
    // by construction — a def the engine refused has no provider — so a create over a broken
    // external def's name succeeds and the fold replaces that def. That is the honest outcome:
    // the user named a table they wanted to exist, the row visibly changes kind, and nothing on
    // their disk is touched.
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
        // `OR REPLACE` over a name that is *free* creates; the report has to say which happened,
        // and the clause on its own does not know.
        taken => taken.is_some(),
    };

    // Defense in depth behind the router's classification, per spec §4: the inner plan is a
    // query and nothing else. `verify_plan` visits subqueries, so smuggled DDL dies here even
    // though the classification in front of `Engine::run` already refused it.
    SQLOptions::new()
        .with_allow_dml(false)
        .with_allow_ddl(false)
        .with_allow_statements(false)
        .verify_plan(&input)
        .map_err(|e| e.to_string())?;

    let slug = slug(&fold_ident(&name));
    let rows = spool(ctx, &input, &tables_dir(&root), &slug).await?;

    let def = TableDef {
        name: name.clone(),
        format: SourceFormat::Arrow,
        // Never a connection: this table's data is Strata's own, spooled into the project's
        // `.strata/tables/` a few lines above. What a remote source names is the user's bucket.
        connection: None,
        sources: vec![internal_source(&slug)],
        partition_cols: Vec::new(),
        origin: TableOrigin::Internal,
    };
    let spec = TableSpec {
        name: name.clone(),
        // Absolute for the engine; the def above keeps the portable form.
        paths: vec![dir_path(&tables_dir(&root).join(&slug))],
        format: SourceFormat::Arrow,
        partitions: Vec::new(),
        internal: true,
    };
    let meta = match register_external(ctx, &spec).await {
        Ok(meta) => meta,
        // **Only on a create.** On a create nothing will ever name this directory — the def is
        // not returned, so it never reaches `project.json` and no later pass registers it — and
        // leaving it would be litter with no sweeper. On a *replace* the opposite holds: a def
        // under this name is already in the store pointing right here, the data it points at is
        // the data we just wrote, and removing it would turn a failed registration into the loss
        // of the table. The user's recovery there is Refresh, which needs the files to exist.
        Err(e) if !replacing => {
            let _ = fs::remove_dir_all(tables_dir(&root).join(&slug));
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

/// The table `name` resolves to in the engine's one schema, and what kind it is — `None` when
/// the name is free.
///
/// Through `table_provider`, not `table`: the latter builds a `DataFrame`, which for a view
/// means planning its whole body just to ask whether the name is taken.
async fn existing(ctx: &SessionContext, name: &str) -> Option<TableType> {
    let provider: Arc<dyn TableProvider> = ctx.table_provider(name).await.ok()?;
    Some(provider.table_type())
}

/// The bare table name a `CREATE TABLE` targets.
///
/// Strata has exactly one catalog and one schema (`engine::providers`), so a qualified name is
/// either a longer spelling of the same place or a place that does not exist — and registration
/// takes a bare name, so an unrecognised qualifier would otherwise be silently dropped and the
/// table created somewhere the user did not ask for.
fn bare_name(name: &TableReference) -> Result<String, String> {
    let ok = match name {
        TableReference::Bare { .. } => true,
        TableReference::Partial { schema, .. } => schema.as_ref() == SCHEMA,
        TableReference::Full {
            catalog, schema, ..
        } => catalog.as_ref() == CATALOG && schema.as_ref() == SCHEMA,
    };
    match ok {
        true => Ok(name.table().to_string()),
        false => Err(format!(
            "Strata has one schema, '{SCHEMA}'. Tables cannot be created elsewhere"
        )),
    }
}

/// Write `input`'s result under `tables/<slug>/`, returning the rows written.
///
/// **Published by rename**, the discipline the snapshot writer already keeps: the files are
/// spooled into a `.tmp-…` sibling and the whole directory is moved into place in one step, so a
/// crash mid-spool leaves nothing but a temp directory the next `.strata` write sweeps
/// (`project::tidy_strata_dir`) rather than a half-written table registered under a real name.
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
/// [`sweep_stale_temp_dirs`](crate::util::sweep_stale_temp_dirs) deliberately never touches this
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
    // `rename` will not replace a non-empty directory, and a replace has to leave the *new*
    // data in place — so the old directory goes first. The window between the two is the whole
    // exposure, and the data being moved in is already complete.
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

    use crate::engine::sql::Blocked;
    use crate::engine::{Engine, RunOutcome, RunTag, StatementReport, WsId};
    use crate::project::{load_defs, save_defs, ProjectDefs};
    use crate::register::{register_project, table_spec, RegOutcome};

    use super::*;

    /// A scratch project folder of our own, per test — the tag is load-bearing because these
    /// run concurrently in one process and each engine re-LISTs its own sources.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = env::temp_dir().join(format!("strata_internal_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // A real project, so `save_defs` scaffolds the `.strata` the spool writes under.
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
        // Project-relative, so the def travels with `project.json`.
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

        // `OR REPLACE` over a name that is free **creates**, and the report has to say so — the
        // clause on its own does not know whether anything was there.
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
        // A *projection* with two `n`s never reaches us — DataFusion's planner refuses it. A
        // join does: `SELECT *` over two relations that each have an `i` builds a plan whose
        // schema carries both, qualified apart in the plan and identical in an IPC file.
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
        // What the app's fold persists, done here so the replay reads a real project file.
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
        // Exactly what a clone has: the def, and no `.strata/tables`.
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
        // Small enough that the spool is several batches whatever the partitioning.
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

        // **A table is a directory of files, not a file** — the Arrow sink writes one per output
        // partition, so any table big enough to parallelise is multi-file from the first CTAS.
        // The listing is over the directory, so the count above is a sum across every file *and*
        // every batch inside each; a single-file assertion here would pass only by accident of
        // `target_partitions` on the machine running it.
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
        let ctx = crate::engine::build_context(&BTreeMap::new());
        let tables = tables_dir(&root);
        fs::create_dir_all(&tables).unwrap();

        // A plan big enough that the spool is still writing at its first await.
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
        // Never a name the temp-directory sweep would claim, whatever the user typed.
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
}
