//! **The grammar** — what one parsed statement *is*, before any question of who is asking.
//!
//! [`classify_stmt`] is a pure function of the parsed statement: it matches the AST rather than
//! sniffing a leading keyword, and it answers with a [`Form`] plus, where the statement itself is
//! at fault, a [`Fault`]. The capability axis lives one file over ([`grants`](super::grants)) and
//! is applied by [`pipeline::classify`](super::pipeline::classify), which is the only place the
//! two meet.
//!
//! **A fault is held rather than raised**, and that ordering is the whole reason this module
//! answers in two parts. A caller the policy phase refuses the form to outright is owed *that*
//! sentence — a read-only agent asking for `INSERT OVERWRITE` hears "INSERT is not supported",
//! not a note about `OVERWRITE` on a statement it may not write at all. So the pipeline asks
//! policy first and reaches the fault only for a caller permitted the form.

use std::ops::ControlFlow;
use std::slice;

use datafusion::sql::parser::{CopyToSource, Statement as DFStatement};
use datafusion::sql::planner::object_name_to_table_reference;
use datafusion::sql::sqlparser::ast::{
    visit_relations, ObjectName, ObjectType, Statement as SqlStatement, Visit,
};

use crate::query::{is_snapshot_name, is_snapshot_ref, ReadPolicy};
use crate::sql::unwrap_statement;

/// What an intercepted statement *is* — [`Form::Statement`]'s payload, and the arm the dispatcher
/// (`engine::ddl::execute`) switches on. Each kind is an engine method rather than a `ctx.sql`
/// passthrough because each has an outcome the catalog store has to fold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StmtKind {
    CreateExternalTable,
    CreateTable,
    Ctas,
    Insert,
    DropTable,
    CreateView,
    DropView,
    Copy,
    Set,
    Reset,
    Prepare,
    Deallocate,
    CreateFunction,
    DropFunction,
    /// Remote-only, so the arm refuses a workspace target in its own words; intercepted rather
    /// than refused here because whose catalog the target is in is not something the parsed
    /// statement says.
    Update,
    /// Remote-only, for [`Update`](StmtKind::Update)'s reason.
    Delete,
}

impl StmtKind {
    /// The statement's SQL name — what a stub refusal, a report and the results pane's
    /// statement row all call it. One table, because three surfaces naming the same kind in
    /// three spellings is the drift a shared vocabulary exists to prevent.
    pub fn label(self) -> &'static str {
        match self {
            StmtKind::CreateExternalTable => "CREATE EXTERNAL TABLE",
            StmtKind::CreateTable => "CREATE TABLE",
            StmtKind::Ctas => "CREATE TABLE AS",
            StmtKind::Insert => "INSERT",
            StmtKind::DropTable => "DROP TABLE",
            StmtKind::CreateView => "CREATE VIEW",
            StmtKind::DropView => "DROP VIEW",
            StmtKind::Copy => "COPY",
            StmtKind::Set => "SET",
            StmtKind::Reset => "RESET",
            StmtKind::Prepare => "PREPARE",
            StmtKind::Deallocate => "DEALLOCATE",
            StmtKind::CreateFunction => "CREATE FUNCTION",
            StmtKind::DropFunction => "DROP FUNCTION",
            StmtKind::Update => "UPDATE",
            StmtKind::Delete => "DELETE",
        }
    }
}

/// The **form** a statement takes — the grammar's whole answer about what it is.
///
/// `Execute` is a read the engine runs and is still its own form, because it is the one read that
/// moves nothing but reaches session state: it belongs to the `Session` grant family, and a
/// caller that may not `PREPARE` may not `EXECUTE` either. How a read *plans* is a different
/// question with a different answer — see [`read_policy`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// A read — the snapshot pipeline, unchanged.
    Read,
    /// `EXECUTE` of a prepared statement.
    Execute,
    /// The engine implements it as `kind`; the store folds the outcome.
    Statement(StmtKind),
}

/// A fault in the **statement itself** — a refusal the grammar mints, in the same words on every
/// surface, because no capability makes a malformed statement well-formed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// `CREATE DATABASE` / `CREATE SCHEMA` — hard-blocked, no owning surface. The
    /// `CatalogProviderList` has no way to refuse a registration, so this is its gate.
    CreateDatabase,
    /// `DROP` of anything that is not a table or a view.
    Drop,
    /// Every other DDL/DML form — the sqlparser wildcard, so default stays deny.
    Unsupported,
    /// An `INSERT` that replaces rows rather than appending — `INSERT OVERWRITE` (refused here,
    /// off the parsed statement) and `REPLACE INTO` (refused at dispatch, since only the plan
    /// names it). DataFusion folds both onto the one thing the Arrow sink has no implementation
    /// for, so they are one refusal.
    InsertOverwrite,
    /// `PREPARE` of a non-query body: `verify_plan` cannot see through the later `EXECUTE`, so
    /// the fence is here.
    PrepareNonQuery,
    /// A `__snap_`-prefixed identifier in the workspace catalog, read or written.
    ReservedName,
}

/// The wording of a `DROP` the engine has no arm for — **one literal**, because it is both a
/// grammar refusal (a `DROP SCHEMA`, which no capability enables) and the policy sentence a
/// caller refused `DROP TABLE` reads. Two spellings of one fact is the drift a shared table
/// exists to prevent.
pub(super) const DROP_UNSUPPORTED: &str =
    "DROP is not supported in the editor. Deregister tables from the catalog";

/// The wording for a form the engine has no arm for, and for every statement family a restricted
/// caller may not reach — shared for [`DROP_UNSUPPORTED`]'s reason.
pub(super) const UNSUPPORTED: &str =
    "This statement is not supported in the editor. Only SELECT, EXPLAIN, SHOW and DESCRIBE can \
     run here";

impl Fault {
    /// The sentence the user reads: IDE register, naming the surface that owns the capability
    /// where there is one. The validator's policy diagnostics are this, verbatim.
    pub fn message(self) -> String {
        match self {
            Fault::CreateDatabase => "CREATE DATABASE and CREATE SCHEMA are not supported",
            Fault::Drop => DROP_UNSUPPORTED,
            Fault::Unsupported => UNSUPPORTED,
            Fault::InsertOverwrite => {
                "An INSERT that replaces rows is not supported. Drop the table and recreate it \
                 with CREATE TABLE AS"
            }
            Fault::PrepareNonQuery => "PREPARE supports queries only",
            Fault::ReservedName => "Names starting with '__snap_' are reserved for query results",
        }
        .into()
    }
}

/// The grammar's whole answer for one statement: what it is, and what is wrong with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Classified {
    pub form: Form,
    /// A fault the statement carries, reached only by a caller the policy phase permits the form
    /// to — see this module's header for why the ordering is that way round.
    pub fault: Option<Fault>,
}

/// **The classifier.** What `stmt` is, or the fault that refuses it on every surface.
///
/// Wildcard-free over `DFStatement`, so a new DataFusion statement variant is a compile error
/// rather than a form that slips through; the sqlparser match below it has the one deliberate
/// wildcard, landing [`Fault::Unsupported`], which is what keeps default-deny true.
pub fn classify_stmt(stmt: &DFStatement) -> Result<Classified, Fault> {
    let form = form_of(stmt)?;
    let fault = form_fault(stmt).or_else(|| names_reserved(stmt).then_some(Fault::ReservedName));
    Ok(Classified { form, fault })
}

/// The form `stmt` takes — the match that has to stay total.
fn form_of(stmt: &DFStatement) -> Result<Form, Fault> {
    let s = match stmt {
        DFStatement::CreateExternalTable(_) => {
            return Ok(Form::Statement(StmtKind::CreateExternalTable))
        }
        DFStatement::CopyTo(_) => return Ok(Form::Statement(StmtKind::Copy)),
        DFStatement::Reset(_) => return Ok(Form::Statement(StmtKind::Reset)),
        DFStatement::Explain(_) => return Ok(Form::Read),
        DFStatement::Statement(s) => s.as_ref(),
    };
    Ok(match s {
        SqlStatement::Query(_)
        | SqlStatement::Explain { .. }
        | SqlStatement::ExplainTable { .. }
        | SqlStatement::ShowTables { .. }
        | SqlStatement::ShowColumns { .. }
        | SqlStatement::ShowFunctions { .. }
        | SqlStatement::ShowVariable { .. }
        | SqlStatement::ShowVariables { .. }
        | SqlStatement::ShowDatabases { .. }
        | SqlStatement::ShowSchemas { .. } => Form::Read,
        SqlStatement::Execute { .. } => Form::Execute,
        SqlStatement::CreateView(_) => Form::Statement(StmtKind::CreateView),
        SqlStatement::Drop { object_type, .. } => match object_type {
            ObjectType::View => Form::Statement(StmtKind::DropView),
            ObjectType::Table => Form::Statement(StmtKind::DropTable),
            _ => return Err(Fault::Drop),
        },
        SqlStatement::CreateTable(create) if create.query.is_some() => {
            Form::Statement(StmtKind::Ctas)
        }
        SqlStatement::CreateTable(_) => Form::Statement(StmtKind::CreateTable),
        SqlStatement::Insert(_) => Form::Statement(StmtKind::Insert),
        SqlStatement::Update(_) => Form::Statement(StmtKind::Update),
        SqlStatement::Delete(_) => Form::Statement(StmtKind::Delete),
        SqlStatement::CreateDatabase { .. } | SqlStatement::CreateSchema { .. } => {
            return Err(Fault::CreateDatabase)
        }
        SqlStatement::Set(_) => Form::Statement(StmtKind::Set),
        SqlStatement::Prepare { .. } => Form::Statement(StmtKind::Prepare),
        SqlStatement::Deallocate { .. } => Form::Statement(StmtKind::Deallocate),
        SqlStatement::CreateFunction(_) => Form::Statement(StmtKind::CreateFunction),
        SqlStatement::DropFunction(_) => Form::Statement(StmtKind::DropFunction),
        _ => return Err(Fault::Unsupported),
    })
}

/// The fault a **well-formed** form still carries — the two clauses the grammar can see and the
/// arms cannot be trusted to. Held on [`Classified`] rather than returned as an `Err`, so a
/// caller refused the form entire hears about the form.
fn form_fault(stmt: &DFStatement) -> Option<Fault> {
    let DFStatement::Statement(s) = stmt else {
        return None;
    };
    match s.as_ref() {
        SqlStatement::Insert(insert) if insert.overwrite => Some(Fault::InsertOverwrite),
        SqlStatement::Prepare { statement, .. } => match statement.as_ref() {
            SqlStatement::Query(_) => None,
            _ => Some(Fault::PrepareNonQuery),
        },
        _ => None,
    }
}

/// Whether `stmt` names a snapshot-reserved table — one it reads, or one it writes.
///
/// The read half keeps a typed `COPY (SELECT * FROM __snap_3) TO …` from writing `__strata_ord`
/// into a user's file; the write half keeps `CREATE TABLE __snap_2` and friends off the namespace
/// a Run mints into. sqlparser's own `visit_relations` covers the reads and the two sqlparser
/// targets upstream annotates (`CREATE TABLE`'s name and `INSERT`'s), but `CREATE VIEW`'s name,
/// `DROP`'s name list and `DELETE`'s multi-table list carry no annotation — and DataFusion's own
/// extension statements are outside the visitor entirely — so those targets are named here rather
/// than assumed. An `UPDATE`'s target and a `DELETE`'s `FROM` are table factors, so the visitor
/// has them.
fn names_reserved(stmt: &DFStatement) -> bool {
    match stmt {
        DFStatement::CreateExternalTable(create) => is_reserved(&create.name),
        DFStatement::CopyTo(copy) => match &copy.source {
            CopyToSource::Relation(name) => is_reserved(name),
            CopyToSource::Query(query) => reads_reserved(query.as_ref()),
        },
        DFStatement::Statement(s) => {
            let targets: &[ObjectName] = match s.as_ref() {
                SqlStatement::CreateView(view) => slice::from_ref(&view.name),
                SqlStatement::Drop { names, .. } => names,
                SqlStatement::Delete(delete) => &delete.tables,
                _ => &[],
            };
            targets.iter().any(is_reserved) || reads_reserved(s.as_ref())
        }
        DFStatement::Explain(explain) => names_reserved(&explain.statement),
        DFStatement::Reset(_) => false,
    }
}

/// Whether any relation `node` reads carries the snapshot prefix.
fn reads_reserved<V: Visit>(node: &V) -> bool {
    visit_relations(node, |name| {
        if is_reserved(name) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_break()
}

/// Whether `name` addresses the snapshot namespace. The predicate itself is
/// [`is_snapshot_ref`], next to the function that mints those names, because the provider's
/// hiding rule asks the same question and the two must not drift.
///
/// **Where the name points, not merely how it is spelled.** The namespace is the workspace
/// catalog's, and a database connection's `__snap_3` is whatever the server called a table. So the
/// qualifier is read through DataFusion's **own** normalization rather than a second reading of the
/// identifier rules, and the reference judged is the one the planner would resolve. A name it
/// refuses resolves nowhere and is reserved by nothing.
///
/// **The prefix is tested first, and only then the qualifier**, because this runs per relation per
/// statement on every re-validation and the answer is almost always no.
fn is_reserved(name: &ObjectName) -> bool {
    let named = name
        .0
        .last()
        .and_then(|part| part.as_ident())
        .is_some_and(|ident| is_snapshot_name(&ident.value));
    named && object_name_to_table_reference(name.clone(), true).is_ok_and(|n| is_snapshot_ref(&n))
}

/// How a read has to be **planned** — the second half of the classifier's answer, for the one
/// query form whose plan is not a plain query.
///
/// `EXECUTE` returns rows and rides the snapshot pipeline whole, but its plan is a
/// `LogicalPlan::Statement`, which the read path's all-false triple refuses. Widening rides the
/// **dispatch** rather than the path, because the widening is only sound for a statement that came
/// through this classifier: `PREPARE` verified the prepared plan under the read triple, and
/// `verify_plan` cannot see through an `Execute` node to check it again.
///
/// **Through `EXPLAIN`, because `verify_plan` visits the whole tree** — which is also why this is
/// a question about the statement rather than about the [`Form`]. An `EXPLAIN EXECUTE p` is an
/// `EXPLAIN`, and its form says so; its *plan* is `Explain { Statement(Execute) }`, and the
/// visitor reaches that child, so it needs the same widening the run of one does. Unwrapped
/// through the resolver's own [`unwrap_statement`], because DataFusion spells `EXPLAIN` twice and
/// answering differently by parser arm is the drift one shared unwrap prevents.
pub(crate) fn read_policy(stmt: &DFStatement) -> ReadPolicy {
    match unwrap_statement(stmt) {
        Some(SqlStatement::Execute { .. }) => ReadPolicy::Statements,
        _ => ReadPolicy::ReadOnly,
    }
}

#[cfg(test)]
mod tests {
    use datafusion::sql::parser::DFParserBuilder;
    use datafusion::sql::sqlparser::dialect::GenericDialect;

    use super::*;

    /// The one parsed statement in `sql`.
    fn parse_one(sql: &str) -> DFStatement {
        let mut stmts = DFParserBuilder::new(sql)
            .with_dialect(&GenericDialect {})
            .build()
            .expect("builds")
            .parse_statements()
            .expect("parses");
        assert_eq!(stmts.len(), 1, "{sql}");
        stmts.pop_back().unwrap()
    }

    fn classify(sql: &str) -> Result<Classified, Fault> {
        classify_stmt(&parse_one(sql))
    }

    /// The form table, statement by statement — what every later phase is derived from.
    #[test]
    fn every_form_is_named_by_its_statement() {
        for (sql, form) in [
            ("SELECT * FROM t", Form::Read),
            ("EXPLAIN SELECT * FROM t", Form::Read),
            ("SHOW TABLES", Form::Read),
            ("DESCRIBE t", Form::Read),
            ("EXECUTE p", Form::Execute),
            (
                "CREATE EXTERNAL TABLE x STORED AS PARQUET LOCATION 'f.parquet'",
                Form::Statement(StmtKind::CreateExternalTable),
            ),
            (
                "CREATE TABLE copy_t AS SELECT * FROM t",
                Form::Statement(StmtKind::Ctas),
            ),
            (
                "CREATE TABLE cols (id BIGINT)",
                Form::Statement(StmtKind::CreateTable),
            ),
            (
                "INSERT INTO t VALUES (3, 'c')",
                Form::Statement(StmtKind::Insert),
            ),
            ("DROP TABLE t", Form::Statement(StmtKind::DropTable)),
            (
                "CREATE VIEW v AS SELECT id FROM t",
                Form::Statement(StmtKind::CreateView),
            ),
            ("DROP VIEW IF EXISTS v", Form::Statement(StmtKind::DropView)),
            ("COPY t TO 'out.parquet'", Form::Statement(StmtKind::Copy)),
            (
                "SET datafusion.execution.batch_size = 1024",
                Form::Statement(StmtKind::Set),
            ),
            (
                "RESET datafusion.execution.batch_size",
                Form::Statement(StmtKind::Reset),
            ),
            (
                "PREPARE p AS SELECT id FROM t",
                Form::Statement(StmtKind::Prepare),
            ),
            ("DEALLOCATE p", Form::Statement(StmtKind::Deallocate)),
            (
                "CREATE FUNCTION f(BIGINT) RETURNS BIGINT RETURN $1 + 1",
                Form::Statement(StmtKind::CreateFunction),
            ),
            ("DROP FUNCTION f", Form::Statement(StmtKind::DropFunction)),
            ("UPDATE t SET name = 'x'", Form::Statement(StmtKind::Update)),
            ("DELETE FROM t", Form::Statement(StmtKind::Delete)),
        ] {
            assert_eq!(classify(sql).expect("classifies").form, form, "{sql}");
        }
    }

    /// The refusals no capability makes well-formed, and their wording.
    #[test]
    fn a_grammar_refusal_is_the_same_on_every_surface() {
        for (sql, fault) in [
            ("CREATE DATABASE other", Fault::CreateDatabase),
            ("CREATE SCHEMA other", Fault::CreateDatabase),
            ("DROP SCHEMA s", Fault::Drop),
            ("TRUNCATE TABLE t", Fault::Unsupported),
            (
                "MERGE INTO t USING u ON t.id = u.id WHEN MATCHED THEN DELETE",
                Fault::Unsupported,
            ),
        ] {
            assert_eq!(classify(sql).expect_err("refused"), fault, "{sql}");
        }
    }

    /// A fault rides on a form rather than replacing it, so the policy phase can refuse the form
    /// first and a permitted caller still hits the fault.
    #[test]
    fn a_faulted_statement_still_names_its_form() {
        let overwrite = classify("INSERT OVERWRITE INTO t VALUES (3, 'c')").expect("has a form");
        assert_eq!(overwrite.form, Form::Statement(StmtKind::Insert));
        assert_eq!(overwrite.fault, Some(Fault::InsertOverwrite));

        let prepare = classify("PREPARE p AS INSERT INTO t VALUES (3, 'c')").expect("has a form");
        assert_eq!(prepare.form, Form::Statement(StmtKind::Prepare));
        assert_eq!(prepare.fault, Some(Fault::PrepareNonQuery));
    }

    /// A clause fault outranks a reserved name: both are the statement's, and the one that names
    /// what the user typed wrong is the more useful sentence.
    #[test]
    fn a_clause_fault_outranks_a_reserved_name() {
        let both = classify("INSERT OVERWRITE INTO __snap_2 VALUES (3, 'c')").expect("has a form");
        assert_eq!(both.fault, Some(Fault::InsertOverwrite));
    }

    /// Reserved names, both halves: a `__snap_` identifier in a statement the engine would run
    /// itself is refused before it can collide with a live snapshot registration — which fails as
    /// "already exists", on a name the same prefix keeps invisible.
    #[test]
    fn a_snapshot_name_is_reserved_in_any_statement() {
        for sql in [
            "CREATE EXTERNAL TABLE __snap_2 STORED AS PARQUET LOCATION 'f.parquet'",
            "CREATE TABLE __snap_2 AS SELECT * FROM t",
            "CREATE TABLE __SNAP_2 (id BIGINT)",
            "CREATE VIEW __snap_2 AS SELECT id FROM t",
            "INSERT INTO __snap_2 VALUES (3, 'c')",
            "DROP TABLE __snap_2",
            "DROP VIEW __snap_2",
            "CREATE TABLE mine AS SELECT * FROM __snap_3",
            "COPY (SELECT * FROM __snap_3) TO 'out.parquet'",
            "COPY __snap_3 TO 'out.parquet'",
            "SELECT 1 FROM __snap_3",
            "SELECT * FROM __snap_3",
            "EXPLAIN SELECT * FROM __snap_3",
        ] {
            assert_eq!(
                classify(sql).expect("has a form").fault,
                Some(Fault::ReservedName),
                "{sql}"
            );
        }
    }

    /// **The prefix is a namespace, and the namespace is the workspace catalog's.** A `__snap_`
    /// name qualified into a database connection's catalog is a relation somebody else named, so
    /// reading it is ordinary and writing to it is refused for being remote rather than reserved.
    ///
    /// Deliberately **syntactic**: nothing asks whether `pg` is registered, because the classifier
    /// is a pure function of the parsed statement. A qualifier naming no catalog resolves nowhere,
    /// and the two arms that could care already say so.
    ///
    /// The second half holds the other direction, and **the quoted spellings are the ones that
    /// bite**: the catalog list resolves by `fold_ident`, so a raw compare reads `"STRATA"` as
    /// somewhere else and let `SELECT * FROM "STRATA".public.__snap_3` hand back another tab's
    /// snapshot. The unquoted spellings could not have caught it, since the parser folds those
    /// first.
    #[test]
    fn the_reserved_namespace_is_the_workspace_catalog() {
        for sql in [
            "SELECT * FROM pg.public.__snap_3",
            "SELECT * FROM pg.analytics.__snap_3",
            "EXPLAIN SELECT * FROM pg.public.__snap_3",
        ] {
            assert_eq!(classify(sql).expect("has a form").fault, None, "{sql}");
        }
        for sql in [
            "SELECT * FROM public.__snap_3",
            "SELECT * FROM strata.public.__snap_3",
            "DROP TABLE strata.public.__snap_3",
            "SELECT * FROM STRATA.PUBLIC.__SNAP_3",
            "SELECT * FROM \"STRATA\".public.__snap_3",
            "SELECT * FROM \"strata\".\"public\".\"__snap_3\"",
        ] {
            assert_eq!(
                classify(sql).expect("has a form").fault,
                Some(Fault::ReservedName),
                "{sql}"
            );
        }
    }

    /// `EXECUTE` is the one read the snapshot pipeline has to widen for — through `EXPLAIN` too,
    /// because `verify_plan` visits the whole tree, and that is why the widening is a question
    /// about the statement and not about its [`Form`].
    #[test]
    fn only_execute_widens_the_read_policy() {
        for sql in ["EXECUTE p", "EXPLAIN EXECUTE p"] {
            assert_eq!(
                read_policy(&parse_one(sql)),
                ReadPolicy::Statements,
                "{sql}"
            );
        }
        for sql in ["SELECT 1", "EXPLAIN SELECT 1", "SHOW TABLES"] {
            assert_eq!(read_policy(&parse_one(sql)), ReadPolicy::ReadOnly, "{sql}");
        }
        assert_eq!(
            classify("EXPLAIN EXECUTE p").expect("classifies").form,
            Form::Read
        );
    }

    /// A grammar refusal says the same thing however it is reached — the shared literals, pinned.
    #[test]
    fn the_shared_refusal_literals_are_one_string() {
        assert_eq!(Fault::Drop.message(), DROP_UNSUPPORTED);
        assert_eq!(Fault::Unsupported.message(), UNSUPPORTED);
    }
}
