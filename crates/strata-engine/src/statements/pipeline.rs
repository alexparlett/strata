//! The one place a statement becomes something the engine will act on.
//!
//! Three stages, whose order the types enforce: [`parse`] mints a [`Parsed`], [`qualify`] mints a
//! [`Qualified`] from one, and [`classify`] takes only a `Qualified`. Both have private fields and
//! no constructor, so qualify-before-classify is a property of the types rather than a call
//! discipline nobody can check. It has to be: the resolution can change a classification, since a
//! bare `__snap_3` the workspace does not hold stops being a reserved name once it resolves into a
//! connection, where the prefix reserves nothing.
//!
//! [`accept`] is the one composition of them. A Run, the agent's pre-dispatch gate and the
//! editor's diagnostics pass all enter here, so a statement the editor did not underline is a
//! statement Run is prepared to perform. (The diagnostics pass drives the stages one at a time
//! rather than calling `accept`, because it reports every statement in a buffer with a span each.)

use std::collections::VecDeque;
use std::ops::Deref;

use datafusion::config::Dialect;
use datafusion::error::DataFusionError;
use datafusion::execution::SessionState;
use datafusion::prelude::SessionContext;
use datafusion::sql::parser::{DFParserBuilder, Statement as DFStatement};
use datafusion::sql::sqlparser::dialect::dialect_from_str;
use datafusion::sql::sqlparser::tokenizer::Span;

use super::classify::{classify_stmt, denied, read_policy, Classified, Fault, Form, StmtKind};
use crate::policy::{Admit, DenyCode, PolicyProvider, Principal};
use crate::query::ReadPolicy;
use crate::sql::qualify::{qualify as resolve_names, Refusal as NameRefusal};

/// The session a statement is judged against, and the policy that judges it.
///
/// The two grammar stages take the session alone, which is what lets a caller already inside an
/// admitted arm resolve a statement of its own composing ([`resolved_one`]) with no policy to
/// ask.
pub struct Pipeline<'e> {
    ctx: &'e SessionContext,
    policy: &'e dyn PolicyProvider,
}

impl<'e> Pipeline<'e> {
    pub fn new(ctx: &'e SessionContext, policy: &'e dyn PolicyProvider) -> Self {
        Pipeline { ctx, policy }
    }

    /// Returns the session the stages read from, for the tiers that run after one: name
    /// resolution and the dry-plan both work against the session the classification judged.
    pub fn context(&self) -> &'e SessionContext {
        self.ctx
    }
}

/// One statement, parsed. Private field: only this module's parse entries mint one.
pub struct Parsed {
    stmt: DFStatement,
}

/// One statement whose bare reads have been resolved against the connected databases. Private
/// field: only [`qualify`] mints one, which is what makes qualify-before-classify a property of
/// the types.
pub struct Qualified {
    stmt: DFStatement,
}

impl Qualified {
    /// Returns the statement.
    pub fn statement(&self) -> &DFStatement {
        &self.stmt
    }

    /// Consumes this and returns the statement, for the read path to plan or an arm to perform.
    pub fn into_statement(self) -> DFStatement {
        self.stmt
    }
}

impl Deref for Qualified {
    type Target = DFStatement;

    fn deref(&self) -> &DFStatement {
        &self.stmt
    }
}

/// A statement the pipeline will act on, and how.
pub enum Admitted {
    /// The snapshot pipeline, carrying the [`ReadPolicy`] the statement is planned under.
    Query { stmt: Qualified, policy: ReadPolicy },
    /// The engine implements it as `kind`; the store folds the outcome.
    Statement { kind: StmtKind, stmt: Qualified },
}

impl Admitted {
    /// Consumes this and returns the statement, whichever arm it is.
    pub fn into_statement(self) -> DFStatement {
        match self {
            Admitted::Query { stmt, .. } | Admitted::Statement { stmt, .. } => {
                stmt.into_statement()
            }
        }
    }
}

/// Why a statement was refused.
///
/// Carried rather than rendered, so a consumer can log the classification and print the sentence
/// from the one table that mints it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The statement itself is at fault, and no capability makes it well-formed.
    Grammar(Fault),
    /// This caller may not perform this form.
    Policy { form: Form, code: DenyCode },
    /// The policy provider could not decide — an unreachable decision point, expired credentials.
    /// Not a pass: the statement is refused and the provider's own words are surfaced.
    Undecided(String),
}

impl Refused {
    /// Returns the sentence the user reads.
    ///
    /// One table per refusal family, and both are the engine's: a policy provider answers in codes
    /// precisely so it cannot reword this.
    pub fn message(&self) -> String {
        match self {
            Refused::Grammar(fault) => fault.message(),
            Refused::Policy { form, code } => denied(*form, *code),
            Refused::Undecided(why) => why.clone(),
        }
    }
}

/// A refusal as a surface reads it: the sentence, and where in the buffer to point.
///
/// `span` is set where the stage that minted the refusal knows a position — the resolution pass,
/// which can name the identifier it refused. A grammar or policy refusal is about the statement as
/// a whole, so it carries none and the editor underlines the leading keywords.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub message: String,
    pub span: Option<Span>,
}

impl From<Refused> for Refusal {
    fn from(refused: Refused) -> Self {
        Refusal {
            message: refused.message(),
            span: None,
        }
    }
}

impl From<NameRefusal> for Refusal {
    fn from(refusal: NameRefusal) -> Self {
        Refusal {
            message: refusal.message,
            span: Some(refusal.span),
        }
    }
}

/// One statement [`policy_verdicts`] refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyRefusal {
    /// Zero-based position of the refused statement in the input.
    pub index: usize,
    /// The refused statement as parsed — its canonical rendering, for naming it back to the
    /// caller. Deliberately not a byte slice of the input: the gate never does offset arithmetic
    /// over text it is judging (the editor's spans are approximate over non-ASCII, which a
    /// squiggle tolerates and a gate must not).
    pub statement: String,
    /// Why it is refused.
    pub reason: Refused,
}

impl PolicyRefusal {
    /// Returns the sentence the caller reads.
    pub fn message(&self) -> String {
        self.reason.message()
    }
}

/// Parses `sql` with this session's own dialect and recursion limit, the same resolution
/// `SessionState::sql_to_statement` performs.
///
/// The one parse in front of the classification, because the gates that call it must not read the
/// same buffer differently.
///
/// # Errors
///
/// The input does not parse, or the session's configured dialect is unknown.
pub fn parse(ctx: &SessionContext, sql: &str) -> Result<VecDeque<Parsed>, Refusal> {
    let state = ctx.state();
    let options = state.config_options();
    let dialect = dialect_from_str(options.sql_parser.dialect).ok_or_else(|| Refusal {
        message: format!("Unsupported SQL dialect: {}", options.sql_parser.dialect),
        span: None,
    })?;
    DFParserBuilder::new(sql)
        .with_dialect(dialect.as_ref())
        .with_recursion_limit(options.sql_parser.recursion_limit)
        .build()
        .and_then(|mut parser| parser.parse_statements())
        .map(|stmts| stmts.into_iter().map(|stmt| Parsed { stmt }).collect())
        .map_err(|e| Refusal {
            message: e.to_string(),
            span: None,
        })
}

/// Parses `sql` as exactly one statement.
///
/// A buffer holding several is still judged per statement by the diagnostics pass; it is a *Run*
/// that takes one, and it refuses the batch here rather than letting DataFusion answer for a limit
/// that is ours (`SessionContext::sql` refuses a batch too, in its own words about its own parser,
/// which tells the user nothing about what to do next).
///
/// # Errors
///
/// As [`parse`], plus an empty buffer and a buffer holding more than one statement.
pub fn parse_one(ctx: &SessionContext, sql: &str) -> Result<Parsed, Refusal> {
    let mut statements = parse(ctx, sql)?;
    if statements.len() > 1 {
        return Err(Refusal {
            message: "Run executes one statement at a time".into(),
            span: None,
        });
    }
    statements.pop_front().ok_or_else(|| Refusal {
        message: "Nothing to run".into(),
        span: None,
    })
}

/// Parses one statement off an already-taken session state, keeping DataFusion's own error.
///
/// The diagnostics pass's mint: it parses one statement range at a time and reads the fault's
/// `Line: N, Column: M` back into a byte span, which needs the `DataFusionError` rather than a
/// message. It hoists the state because taking one deep-clones every function registry, and a
/// buffer has as many statements as the user typed.
pub(crate) fn parse_range(
    state: &SessionState,
    dialect: &Dialect,
    sql: &str,
) -> Result<Parsed, DataFusionError> {
    state
        .sql_to_statement(sql, dialect)
        .map(|stmt| Parsed { stmt })
}

/// Resolves every bare read in `parsed` against the connected databases, minting the
/// [`Qualified`] every later stage takes.
///
/// # Errors
///
/// A bare name more than one connected database holds, one entry per name. All of them, because
/// the diagnostics pass squiggles each; a caller judging one statement takes the first.
pub fn qualify(ctx: &SessionContext, parsed: Parsed) -> Result<Qualified, Vec<Refusal>> {
    let Parsed { mut stmt } = parsed;
    let refusals = resolve_names(ctx, &mut stmt);
    match refusals.is_empty() {
        true => Ok(Qualified { stmt }),
        false => Err(refusals.into_iter().map(Refusal::from).collect()),
    }
}

/// Returns what `who` may do with `stmt`.
///
/// The grammar first, then the policy, then the statement's own fault. That order is the design: a
/// caller refused the form outright is owed that sentence, so a read-only agent asking for
/// `INSERT OVERWRITE` hears "INSERT is not supported" rather than a note about the `OVERWRITE` on
/// a statement it may not write at all.
///
/// # Errors
///
/// A form the engine has no arm for, a form `who` may not perform, a fault the statement carries,
/// or a policy provider that could not decide.
pub async fn classify(
    p: &Pipeline<'_>,
    who: &Principal,
    stmt: Qualified,
) -> Result<Admitted, Refused> {
    let Classified { form, fault } = classify_stmt(&stmt).map_err(Refused::Grammar)?;
    match p.policy.admit(who, form.family()).await {
        Err(why) => return Err(Refused::Undecided(why)),
        Ok(Admit::Deny(code)) => return Err(Refused::Policy { form, code }),
        Ok(Admit::Allow) => {}
    }
    if let Some(fault) = fault {
        return Err(Refused::Grammar(fault));
    }
    Ok(match form {
        Form::Read | Form::Execute => Admitted::Query {
            policy: read_policy(&stmt),
            stmt,
        },
        Form::Statement(kind) => Admitted::Statement { kind, stmt },
    })
}

/// Returns `sql` as one statement `who` may perform.
///
/// The one composition of the three stages.
///
/// # Errors
///
/// Anything [`parse_one`], [`qualify`] or [`classify`] refuses, as the sentence a surface shows.
///
/// ```compile_fail,E0308
/// // Classifying an unqualified statement does not compile: `classify` takes a `Qualified`, and
/// // only `qualify` mints one.
/// fn hand_over(parsed: strata_engine::statements::Parsed) -> strata_engine::statements::Qualified {
///     parsed
/// }
/// ```
pub async fn accept(p: &Pipeline<'_>, sql: &str, who: &Principal) -> Result<Admitted, Refusal> {
    let parsed = parse_one(p.ctx, sql)?;
    let qualified = qualify(p.ctx, parsed).map_err(first)?;
    classify(p, who, qualified).await.map_err(Refusal::from)
}

/// Returns every statement in `sql` that `who` may not perform.
///
/// An empty answer is a clean pass. A pre-dispatch gate, so the caller refuses dispatch on either
/// non-clean answer.
///
/// # Errors
///
/// The input could not be judged: it does not parse, the dialect is unknown, or a bare name is
/// ambiguous. The gate fails closed on all three — unjudgeable input is never a policy pass, and
/// one broken statement never silently approves its neighbours.
pub async fn policy_verdicts(
    p: &Pipeline<'_>,
    who: &Principal,
    sql: &str,
) -> Result<Vec<PolicyRefusal>, String> {
    let mut refusals = Vec::new();
    for (index, parsed) in parse(p.ctx, sql)
        .map_err(|r| r.message)?
        .into_iter()
        .enumerate()
    {
        let qualified = qualify(p.ctx, parsed).map_err(|r| first(r).message)?;
        let statement = qualified.statement().to_string();
        if let Err(reason) = classify(p, who, qualified).await {
            refusals.push(PolicyRefusal {
                index,
                statement,
                reason,
            });
        }
    }
    Ok(refusals)
}

/// [`parse_one`] then [`qualify`], for the callers that compose a statement of their own and have
/// to hand it to the planner.
///
/// No classification, deliberately: these are either already inside an arm the pipeline admitted
/// (a view's body, a statement bound for a server) or a read the facade limits to reading. What
/// they need is the resolution, because a resolved statement cannot be rendered back to text
/// without losing the buffer the user wrote.
pub(crate) fn resolved_one(ctx: &SessionContext, sql: &str) -> Result<DFStatement, String> {
    let parsed = parse_one(ctx, sql).map_err(|r| r.message)?;
    qualify(ctx, parsed)
        .map(Qualified::into_statement)
        .map_err(|refusals| first(refusals).message)
}

/// The first of a stage's refusals — what a caller judging one statement reports.
fn first(refusals: Vec<Refusal>) -> Refusal {
    refusals.into_iter().next().expect("a refusal to report")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use datafusion::arrow::array::{Int64Array, StringArray};
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;
    use futures::executor::block_on;
    use std::sync::Arc;

    use super::*;
    use crate::policy::{Capability, CapabilityPolicyProvider};

    /// A context with one table `t(id, name)` — enough for every classification below.
    fn ctx() -> SessionContext {
        let ctx = SessionContext::new();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("name", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![1_i64, 2])),
                Arc::new(StringArray::from(vec!["a", "b"])),
            ],
        )
        .unwrap();
        ctx.register_batch("t", batch).unwrap();
        ctx
    }

    /// The engine's default policy — an engine nobody restricted.
    fn provider() -> CapabilityPolicyProvider {
        CapabilityPolicyProvider::new(Capability::full())
    }

    /// What `sql` comes back as for a caller holding `capability`.
    fn admitted(
        ctx: &SessionContext,
        capability: Capability,
        sql: &str,
    ) -> Result<Admitted, Refusal> {
        let policy = provider();
        let pipeline = Pipeline::new(ctx, &policy);
        block_on(accept(&pipeline, sql, &Principal::new(capability)))
    }

    /// The refusal `sql` came back with for `capability`, or a panic naming what it did instead.
    fn refusal(ctx: &SessionContext, capability: Capability, sql: &str) -> String {
        match admitted(ctx, capability, sql) {
            Err(refusal) => refusal.message,
            Ok(_) => panic!("'{sql}' was admitted"),
        }
    }

    /// **The parity matrix.** For every statement form, what the editor's capability performs and
    /// what the read-only one is refused with. This table *is* the claim that the grants model
    /// reproduces the editor and the agent as they shipped, byte for byte: the right-hand column
    /// is the agent's message, and `grants`'s own test pins those literals.
    #[test]
    fn the_two_presets_reproduce_the_editor_and_the_agent() {
        let ctx = ctx();
        for (sql, kind) in [
            (
                "CREATE EXTERNAL TABLE x STORED AS PARQUET LOCATION 'f.parquet'",
                StmtKind::CreateExternalTable,
            ),
            ("CREATE TABLE copy_t AS SELECT * FROM t", StmtKind::Ctas),
            ("CREATE TABLE cols (id BIGINT)", StmtKind::CreateTable),
            ("INSERT INTO t VALUES (3, 'c')", StmtKind::Insert),
            ("DROP TABLE t", StmtKind::DropTable),
            ("CREATE VIEW v AS SELECT id FROM t", StmtKind::CreateView),
            ("DROP VIEW IF EXISTS v", StmtKind::DropView),
            ("COPY t TO 'out.parquet'", StmtKind::Copy),
            ("SET datafusion.execution.batch_size = 1024", StmtKind::Set),
            ("RESET datafusion.execution.batch_size", StmtKind::Reset),
            ("PREPARE p AS SELECT id FROM t", StmtKind::Prepare),
            ("DEALLOCATE p", StmtKind::Deallocate),
            (
                "CREATE FUNCTION f(BIGINT) RETURNS BIGINT RETURN $1 + 1",
                StmtKind::CreateFunction,
            ),
            ("DROP FUNCTION f", StmtKind::DropFunction),
            ("UPDATE t SET name = 'x'", StmtKind::Update),
            ("DELETE FROM t", StmtKind::Delete),
        ] {
            let Ok(Admitted::Statement { kind: got, .. }) = admitted(&ctx, Capability::full(), sql)
            else {
                panic!("'{sql}' must be a statement for the editor");
            };
            assert_eq!(got, kind, "{sql}");
            assert_eq!(
                refusal(&ctx, Capability::read_only(), sql),
                denied(Form::Statement(kind), DenyCode::NotGranted),
                "{sql}"
            );
        }
    }

    /// Reading is never refused, whoever is asking.
    #[test]
    fn every_capability_reads() {
        let ctx = ctx();
        for sql in [
            "SELECT * FROM t",
            "EXPLAIN SELECT * FROM t",
            "SHOW TABLES",
            "DESCRIBE t",
        ] {
            for capability in [Capability::full(), Capability::read_only()] {
                assert!(
                    matches!(admitted(&ctx, capability, sql), Ok(Admitted::Query { .. })),
                    "{sql}"
                );
            }
        }
    }

    /// **A caller refused the form hears about the form**, which is why the grammar holds a fault
    /// rather than raising it: telling a read-only caller what is wrong with the `OVERWRITE` on a
    /// statement it may not write at all is an answer to a question it did not ask.
    #[test]
    fn a_caller_refused_the_form_is_told_about_the_form() {
        let ctx = ctx();
        for (sql, fault, kind) in [
            (
                "INSERT OVERWRITE INTO t VALUES (3, 'c')",
                Fault::InsertOverwrite,
                StmtKind::Insert,
            ),
            (
                "PREPARE p AS INSERT INTO t VALUES (3, 'c')",
                Fault::PrepareNonQuery,
                StmtKind::Prepare,
            ),
        ] {
            assert_eq!(
                refusal(&ctx, Capability::full(), sql),
                fault.message(),
                "{sql}"
            );
            assert_eq!(
                refusal(&ctx, Capability::read_only(), sql),
                denied(Form::Statement(kind), DenyCode::NotGranted),
                "{sql}"
            );
        }
    }

    /// `EXECUTE` is a read the editor runs under the widened policy, and a session statement a
    /// caller that may not `PREPARE` may not reach.
    #[test]
    fn execute_reads_for_one_capability_and_is_refused_the_other() {
        let ctx = ctx();
        assert!(matches!(
            admitted(&ctx, Capability::full(), "EXECUTE p"),
            Ok(Admitted::Query {
                policy: ReadPolicy::Statements,
                ..
            })
        ));
        assert_eq!(
            refusal(&ctx, Capability::read_only(), "EXECUTE p"),
            denied(Form::Execute, DenyCode::NotGranted)
        );
    }

    /// A reserved name refuses whatever the caller holds — behind the form refusal, which comes
    /// first for a caller that may not perform the form at all.
    #[test]
    fn a_reserved_name_refuses_behind_the_form() {
        let ctx = ctx();
        let write = "CREATE TABLE __snap_2 AS SELECT * FROM t";
        assert_eq!(
            refusal(&ctx, Capability::full(), write),
            Fault::ReservedName.message()
        );
        assert_eq!(
            refusal(&ctx, Capability::read_only(), write),
            denied(Form::Statement(StmtKind::Ctas), DenyCode::NotGranted)
        );
        for capability in [Capability::full(), Capability::read_only()] {
            assert_eq!(
                refusal(&ctx, capability, "SELECT * FROM __snap_3"),
                Fault::ReservedName.message()
            );
        }
    }

    /// A grammar refusal is the same sentence for every capability: no policy makes a malformed
    /// statement well-formed.
    #[test]
    fn a_grammar_refusal_is_capability_blind() {
        let ctx = ctx();
        for (sql, fault) in [
            ("CREATE DATABASE other", Fault::CreateDatabase),
            ("CREATE SCHEMA other", Fault::CreateDatabase),
            ("DROP SCHEMA s", Fault::Drop),
            ("TRUNCATE TABLE t", Fault::Unsupported),
        ] {
            for capability in [Capability::full(), Capability::read_only()] {
                assert_eq!(refusal(&ctx, capability, sql), fault.message(), "{sql}");
            }
        }
    }

    /// The read-only claim, structurally: whatever the editor implements itself, a read-only
    /// caller is refused. No form may become runnable there by growing an interception.
    #[test]
    fn a_read_only_caller_is_refused_everything_the_editor_implements() {
        let ctx = ctx();
        for sql in [
            "CREATE EXTERNAL TABLE x STORED AS PARQUET LOCATION 'f.parquet'",
            "CREATE TABLE copy_t AS SELECT * FROM t",
            "CREATE TABLE cols (id BIGINT)",
            "INSERT INTO t VALUES (3, 'c')",
            "DROP TABLE t",
            "CREATE VIEW v AS SELECT id FROM t",
            "DROP VIEW IF EXISTS v",
            "COPY t TO 'out.parquet'",
            "SET datafusion.execution.batch_size = 1024",
            "RESET datafusion.execution.batch_size",
            "PREPARE p AS SELECT id FROM t",
            "DEALLOCATE p",
            "CREATE FUNCTION f(BIGINT) RETURNS BIGINT RETURN $1 + 1",
            "DROP FUNCTION f",
        ] {
            assert!(
                matches!(
                    admitted(&ctx, Capability::full(), sql),
                    Ok(Admitted::Statement { .. })
                ),
                "{sql} must be a statement"
            );
            let _ = refusal(&ctx, Capability::read_only(), sql);
        }
    }

    #[test]
    fn a_multi_statement_input_is_judged_per_statement() {
        let ctx = ctx();
        let policy = provider();
        let pipeline = Pipeline::new(&ctx, &policy);
        let who = Principal::new(Capability::read_only());
        let out = block_on(policy_verdicts(
            &pipeline,
            &who,
            "SELECT 1; INSERT INTO t VALUES (1, 'a'); DROP VIEW v",
        ))
        .expect("parses");
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0].index, 1);
        assert_eq!(
            out[0].reason,
            Refused::Policy {
                form: Form::Statement(StmtKind::Insert),
                code: DenyCode::NotGranted
            }
        );
        assert!(
            out[0].statement.starts_with("INSERT"),
            "{}",
            out[0].statement
        );
        assert_eq!(out[1].index, 2);
        assert_eq!(
            out[1].reason,
            Refused::Policy {
                form: Form::Statement(StmtKind::DropView),
                code: DenyCode::NotGranted
            }
        );
    }

    #[test]
    fn a_full_capability_gets_no_verdict() {
        let ctx = ctx();
        let policy = provider();
        let pipeline = Pipeline::new(&ctx, &policy);
        let who = Principal::new(Capability::full());
        for sql in [
            "SELECT * FROM t",
            "EXPLAIN SELECT * FROM t",
            "SHOW TABLES",
            "SHOW COLUMNS FROM t",
            "DESCRIBE t",
            "INSERT INTO t VALUES (1, 'a')",
        ] {
            assert!(
                block_on(policy_verdicts(&pipeline, &who, sql))
                    .expect("parses")
                    .is_empty(),
                "{sql}"
            );
        }
    }

    /// The gate fails **closed**: input it cannot judge is `Err`, never an empty `Ok` that reads
    /// as a clean pass — and one broken statement never silently approves its neighbours.
    #[test]
    fn the_gate_fails_closed_on_input_it_cannot_judge() {
        let ctx = ctx();
        let policy = provider();
        let pipeline = Pipeline::new(&ctx, &policy);
        let who = Principal::new(Capability::read_only());
        for sql in [
            "SELEC * FRM t",
            "SELEC 1; INSERT INTO t VALUES (1, 'a')",
            "INSERT INTO t VALUES (1, 'a'); SELECT 'oops",
        ] {
            assert!(
                block_on(policy_verdicts(&pipeline, &who, sql)).is_err(),
                "{sql}"
            );
        }
    }

    /// Non-ASCII text ahead of a refusal must not disturb it: the gate parses the input whole
    /// rather than re-slicing it by computed offsets (which mis-split exactly here — character
    /// columns added to byte positions).
    #[test]
    fn a_refusal_behind_multibyte_text_still_lands() {
        let ctx = ctx();
        let policy = provider();
        let pipeline = Pipeline::new(&ctx, &policy);
        let who = Principal::new(Capability::read_only());
        let out = block_on(policy_verdicts(
            &pipeline,
            &who,
            "SELECT 'caféé'; INSERT INTO t VALUES (1, 'a')",
        ))
        .expect("parses");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].index, 1);
    }

    /// A Run takes one statement, and says so in its own words rather than DataFusion's about a
    /// limit that is ours.
    #[test]
    fn a_run_takes_exactly_one_statement() {
        let ctx = ctx();
        assert_eq!(
            refusal(&ctx, Capability::full(), "SELECT 1; SELECT 2"),
            "Run executes one statement at a time"
        );
        assert_eq!(refusal(&ctx, Capability::full(), "   "), "Nothing to run");
    }

    /// A policy provider that cannot decide refuses the statement in its own words — never a
    /// pass.
    #[test]
    fn an_undecided_policy_refuses_the_statement() {
        assert_eq!(
            Refused::Undecided("the policy service is unreachable".into()).message(),
            "the policy service is unreachable"
        );
    }

    /// **One answerer at the classification tier**, asserted by reading the source — because the
    /// property is about where a function is *not* defined.
    ///
    /// The motivating bug was the editor red-squiggling a statement Run executed fine:
    /// two answerers, kept in step by hand until they were not. The diagnostics pass now enters
    /// these stages like everything else, and this is what keeps a second classifier from growing
    /// back beside it.
    #[test]
    fn nothing_outside_this_module_classifies_a_statement() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let definition = classifier_definition();
        let defined: Vec<PathBuf> = rust_files(&src)
            .into_iter()
            .filter(|file| {
                fs::read_to_string(file)
                    .expect("readable")
                    .contains(&definition)
            })
            .collect();
        assert_eq!(
            defined.len(),
            1,
            "the classifier is defined once: {defined:?}"
        );

        let validate = fs::read_to_string(src.join("sql/validate.rs")).expect("readable");
        assert!(
            validate.contains("statements::"),
            "the diagnostics pass reaches the pipeline"
        );
        for grown_back in [
            "fn classify",
            "enum StmtKind",
            "enum Blocked",
            "enum Verdict",
        ] {
            assert!(
                !validate.contains(grown_back),
                "`{grown_back}` is back in the diagnostics pass — it classifies through the \
                 pipeline and nowhere else"
            );
        }
    }

    /// The classifier's definition as a source file spells it, assembled rather than written out
    /// so the test looking for it does not match its own file.
    fn classifier_definition() -> String {
        format!("fn {}", "classify_stmt")
    }

    /// Every `.rs` file under `dir`, recursively.
    fn rust_files(dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                found.extend(rust_files(&path));
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
        found.sort();
        found
    }
}
