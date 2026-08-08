//! The SQL **validator** (S25 / P2-18) — everything the editor squiggles.
//!
//! One entry point, [`validate`], accumulating four tiers of diagnostics:
//!
//! 1. **Lexical** — the tokenizer's own faults (unterminated string / quoted ident),
//!    unbalanced parentheses, and the keyword-typo lint (`FORM` → `FROM`).
//! 2. **Policy** — each statement is parsed with DataFusion's own `DFParser` (via
//!    [`SessionState::sql_to_statement`]) and put through the statement router,
//!    [`classify`], as [`Capability::Editor`]. Queries and introspection run; the
//!    statements the editor implements itself ([`Verdict::Intercept`] — typed table
//!    and view DDL, internal tables, `COPY`, `SET`) draw no squiggle and go on to the
//!    tiers below; the short list still refused ([`Verdict::Refuse`] —
//!    `CREATE DATABASE`/`SCHEMA`, `UPDATE`/`DELETE`, unknown kinds) gets a policy
//!    diagnostic pointing at the right surface instead of a confusing engine error.
//! 3. **Names** — the native [`resolve`](crate::engine::sql::resolve)r walks the
//!    parsed AST and reports **every** unknown table/column with a span (the planner
//!    below is fail-fast: one name per statement), staying quiet where a mid-edit
//!    scope is unknowable. When it finds name faults, the dry-plan is skipped.
//! 4. **Semantic** — the allowed statements are **dry-planned** against the live
//!    `SessionContext` ([`SessionState::statement_to_plan`], then
//!    [`SessionState::optimize`] for the analyzer's type coercion): unknown
//!    functions, bad casts, arity/coercion faults and name semantics the resolver
//!    skips (ambiguity, exact case) surface as the *same* errors a Run would hit —
//!    zero drift, nothing executes and no snapshot materializes. DF 54 attaches
//!    spanned [`Diagnostic`]s to resolution errors (the engine enables
//!    `collect_spans`), which map straight onto squiggles.
//!
//! Statements are split on top-level `;` and validated independently, so one broken
//! statement never hides the others' diagnostics.

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::ops::{ControlFlow, Range};
use std::slice;

use datafusion::common::diagnostic::DiagnosticKind;
use datafusion::common::{DataFusionError, SchemaError, TableReference};
use datafusion::prelude::SessionContext;
use datafusion::sql::parser::{CopyToSource, DFParserBuilder, Statement as DFStatement};
use datafusion::sql::sqlparser::ast::{
    visit_relations, ObjectName, ObjectType, Statement as SqlStatement, Visit,
};
use datafusion::sql::sqlparser::dialect::dialect_from_str;
use datafusion::sql::sqlparser::parser::ParserError;

use crate::engine::query::is_snapshot_name;
use crate::engine::sql::lex::{
    is_reserved_in_name_position, lex, rel_offset, split_statements, Tok, TokKind,
};
use crate::engine::sql::resolve::resolve;
use crate::engine::sql::FunctionCatalog;
use strata_model::{Diagnostic, Severity};

/// Clause keywords we typo-check bare identifiers against (edit distance ≤ 1).
const CLAUSE_KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "GROUP",
    "ORDER",
    "HAVING",
    "QUALIFY",
    "LIMIT",
    "OFFSET",
    "JOIN",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "CROSS",
    "NATURAL",
    "ON",
    "USING",
    "AS",
    "BY",
    "DISTINCT",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "WITH",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "ASC",
    "DESC",
];

/// Validate `sql` against the live session and return **all** diagnostics, byte-spanned
/// where the fault is localizable. Read-only over the context: statements are parsed and
/// planned, never executed (DDL only takes effect when its plan is driven).
pub async fn validate(
    ctx: &SessionContext,
    functions: &FunctionCatalog,
    sql: &str,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if sql.trim().is_empty() {
        return out;
    }

    // The dialect first, because the *tokenizer* takes it too: reading it after lexing is
    // how the two came apart in the first place (WJ-04). Off `state_ref`, not `ctx.state()` —
    // that clones the whole `SessionState`, and the tokenizer-error arm below returns without
    // ever needing one (an unterminated string is a constant mid-edit state, and this runs per
    // keystroke). The dialect itself is a `Copy` enum, so nothing outlives the guard.
    let dialect = ctx.state_ref().read().config_options().sql_parser.dialect;

    let (toks, lex_err) = lex(sql, dialect.as_ref());
    if let Some(e) = lex_err {
        out.push(diag(Severity::Error, e.message, e.span, sql));
        // A tokenizer failure means splitting/planning would misread the text.
        return out;
    }

    check_parens(&toks, sql, &mut out);
    let hints = keyword_typo_hints(&toks, ctx, functions);

    let state = ctx.state();
    let ranges = statement_ranges(sql, &toks);
    let last = ranges.len().saturating_sub(1);
    for (idx, stmt_range) in ranges.into_iter().enumerate() {
        let slice = &sql[stmt_range.clone()];
        let stmt = match state.sql_to_statement(slice, &dialect) {
            Ok(stmt) => stmt,
            Err(err) => {
                // A trailing statement that fails at end-of-input is a valid *prefix*
                // — the user is mid-thought, not mistaken. Stay quiet (Run still
                // rejects it); an incomplete statement *followed by* another one is a
                // real fault and keeps its error. Name checks below run either way.
                if idx == last && is_incomplete(&err, slice, &stmt_range, &toks) {
                    check_from_targets(ctx, &toks, &stmt_range, sql, &mut out);
                    continue;
                }
                let mut d = df_error_diag(&err, sql, slice, &stmt_range, &toks);
                // When the parser choked on a token that reads as a keyword typo, the
                // hint is the better wording of the same fault — one diagnostic, not
                // an error and a warning stacked on the same span.
                if let Some((_, hint)) = hints
                    .iter()
                    .find(|(span, _)| d.span.as_ref().is_some_and(|s| overlaps(s, span)))
                {
                    d.message = hint.clone();
                }
                out.push(d);
                // The statement didn't parse, so the planner never resolved names —
                // best-effort check the FROM/JOIN targets against the catalog so a
                // broken keyword doesn't hide an unknown table. (When the parse
                // succeeds, the planner is the authority and this never runs.)
                check_from_targets(ctx, &toks, &stmt_range, sql, &mut out);
                continue;
            }
        };
        match classify(&stmt, Capability::Editor) {
            Verdict::Refuse(blocked) => {
                out.push(diag(
                    Severity::Error,
                    blocked.editor_message(),
                    leading_keywords_span(&toks, &stmt_range),
                    sql,
                ));
                continue;
            }
            // An intercepted statement is one the editor *runs*, through an engine
            // method — so no squiggle, and it falls through to the same name and
            // semantic tiers a query gets. Planning a DDL statement builds its node
            // without executing it (execution lives in `execute_logical_plan`), so
            // typed DDL earns its name-resolution diagnostics for free.
            Verdict::Intercept(_) | Verdict::Query => {}
        }
        // The native name resolver first: every unknown table/column in the
        // statement, not just the one the planner would fail-fast on. When it
        // finds name faults the dry-plan is skipped — the planner would stop at
        // the same first name, and types are meaningless against unknown columns
        // (they surface on the next pass, once the names are fixed).
        let resolution = resolve(ctx, &stmt, slice, stmt_range.start, sql).await;
        if !resolution.diags.is_empty() {
            out.extend(resolution.diags);
            continue;
        }
        let planned = match state.statement_to_plan(stmt).await {
            // The analyzer pass (type coercion, subquery checks) only runs in
            // `optimize` — it's what catches statically-bad casts and expressions.
            Ok(plan) => state.optimize(&plan).map(|_| ()),
            Err(err) => Err(err),
        };
        if let Err(err) = planned {
            // The resolver found nothing wrong. If it also had *full* knowledge of
            // every scope, a planner field error is engine truth the walk skipped
            // (ambiguity, exact-case semantics) and surfaces. But where the walk
            // went quiet — a FROM-less draft, a table function, an underivable
            // projection — "column not found" is premature, not wrong (`SELECT
            // name, tags` mid-composition resolves against an empty schema): the
            // same valid-prefix stance as the incomplete trailing statement above.
            // Everything else (unknown functions, bad casts) still surfaces, and a
            // Run reports the real engine error in the results.
            let premature = is_unresolved_column(&err) && !resolution.complete;
            if !premature {
                out.push(df_error_diag(&err, sql, slice, &stmt_range, &toks));
            }
        }
    }

    // A hint standing on its own becomes a warning; one overlapping any error is
    // redundant (the parse arm already took its wording, or the engine's message —
    // e.g. an unknown-column error on the same token — says it better).
    for (span, hint) in hints {
        let covered = out
            .iter()
            .any(|d| d.span.as_ref().is_some_and(|s| overlaps(s, &span)));
        if !covered {
            out.push(diag(Severity::Warning, hint, span, sql));
        }
    }
    out
}

/// Whether two byte ranges intersect.
fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// The planner failed to resolve a column reference (`Schema error: No field
/// named …`) — matched by variant, not message text.
fn is_unresolved_column(err: &DataFusionError) -> bool {
    matches!(
        err.find_root(),
        DataFusionError::SchemaError(e, _)
            if matches!(e.as_ref(), SchemaError::FieldNotFound { .. })
    )
}

/// A parse failure at end-of-input — the statement is a valid *prefix* of something,
/// i.e. incomplete rather than wrong. Structural test first: the parser's reported
/// position (the `Line: N, Column: M` suffix sqlparser appends to every parse error —
/// the same contract [`df_error_diag`] spans rely on) sits at or past the statement's
/// last token, meaning the parser consumed everything written and wanted more. A
/// message with no position falls back to sqlparser's `found: EOF` wording.
fn is_incomplete(err: &DataFusionError, slice: &str, stmt: &Range<usize>, toks: &[Tok]) -> bool {
    let msg = match err.find_root() {
        DataFusionError::SQL(pe, _) => match pe.as_ref() {
            ParserError::ParserError(s) | ParserError::TokenizerError(s) => s,
            ParserError::RecursionLimitExceeded => return false,
        },
        _ => return false,
    };
    if let Some((line, col)) = extract_line_col(msg) {
        let at = rel_offset(slice, line as u64, col as u64);
        let last_tok_end = toks
            .iter()
            .filter(|t| t.span.start >= stmt.start && t.span.end <= stmt.end)
            .map(|t| t.span.end - stmt.start)
            .max();
        return last_tok_end.is_none_or(|end| at >= end);
    }
    msg.contains("found: EOF")
}

// ---- statement split -------------------------------------------------------

/// Byte ranges of the token-bearing statements in `sql`, split on top-level `;`.
/// Token-level, so `;` inside strings/comments never splits, and whitespace- or
/// comment-only segments (no tokens) are dropped rather than "validated".
fn statement_ranges(sql: &str, toks: &[Tok]) -> Vec<Range<usize>> {
    split_statements(toks, sql.len())
        .into_iter()
        .filter(|r| {
            toks.iter()
                .any(|t| t.span.start >= r.start && t.span.end <= r.end)
        })
        .filter_map(|r| trim_range(sql, r))
        .collect()
}

/// Shrink `range` to its non-whitespace core; `None` if nothing is left.
fn trim_range(sql: &str, range: Range<usize>) -> Option<Range<usize>> {
    let slice = &sql[range.clone()];
    let trimmed = slice.trim_start();
    let start = range.start + (slice.len() - trimmed.len());
    let end = start + trimmed.trim_end().len();
    (start < end).then_some(start..end)
}

// ---- the statement router --------------------------------------------------

/// Which surface is asking — the router's second axis (ED-01).
///
/// The **editor** is a full-statement surface: what it cannot run natively it
/// *intercepts*, implementing the statement as an engine method whose outcome the
/// store folds. The **agent** surface is read-only, and refuses every non-query with
/// the classification AA-01 shipped. One classification, two answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Editor,
    Agent,
}

/// What an intercepted statement *is* — [`Verdict::Intercept`]'s payload, and the arm
/// the dispatcher (`engine::ddl::execute`) switches on. Each kind is
/// an engine method rather than a `ctx.sql` passthrough because each has an outcome the
/// catalog store has to fold.
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
        }
    }
}

/// The router's answer for one parsed statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Runs the snapshot pipeline unchanged.
    Query,
    /// The engine implements it; the store folds the outcome.
    Intercept(StmtKind),
    /// Refused, with the classification each surface renders its own way.
    Refuse(Blocked),
}

/// Why a statement is refused — the **classification alone**, so
/// each consumer names its own owning surface. The zero-copies rule only needs the
/// predicate shared: the editor's rendering is [`editor_message`](Blocked::editor_message),
/// and a headless consumer (the agent tool layer, AA-02) renders the same variant in
/// its own words — over stdio there is no Table Config pane to be pointed at.
/// The variants above the split were the whole managed-DDL policy. They stay defined
/// as **the agent path's error messages** — [`Capability::Agent`] still answers with
/// each of them verbatim — and are unreachable from the editor, which intercepts every
/// one of those statements and runs it. They are kept, not pruned: `strata-agent` names
/// them directly, so a deletion is a compile break rather than a silent rewording.
/// What the editor still refuses is the short list below them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    CreateExternalTable,
    CopyTo,
    Reset,
    /// Views are Save's artifact on a read-only surface: ⌘S / Save-as-view wraps the
    /// *plain query* in `CREATE OR REPLACE VIEW` itself. (The editor intercepts typed
    /// view DDL onto that same funnel; the agent surface keeps the refusal.)
    CreateView,
    DropView,
    /// `DROP` of anything that is not a view.
    Drop,
    CreateTable,
    Insert,
    /// `CREATE DATABASE` / `CREATE SCHEMA` — hard-blocked, no owning surface.
    CreateDatabase,
    Set,
    /// Every other DDL/DML form.
    Unsupported,

    // ---- what the editor still refuses (ED-01) ----
    // `CreateDatabase` and `Unsupported` above are the rest of that list.
    // Some are a pure function of the parsed statement and are produced by
    // `classify`; the rest need context the bare statement lacks (an INSERT
    // target's origin, a SET key's class) and are produced at dispatch. Either
    // way the wording lives here, so a refusal reads the same wherever it is
    // decided.
    /// `INSERT` into an external table or a view — only internal tables take writes.
    InsertExternal,
    /// `INSERT OVERWRITE` — no internal-table implementation, and the Arrow sink has none.
    InsertOverwrite,
    /// `SET` of a key Strata owns (`is_owned_key`).
    SetOwned,
    /// `SET datafusion.runtime.*` — a restart-scoped key, so Settings owns it.
    SetRuntime,
    /// `SET datafusion.format.*` — display keys, which the grid and the chart read
    /// from the Settings store.
    SetFormat,
    /// `PREPARE` of a non-query body: `verify_plan` cannot see through the later
    /// `EXECUTE`, so the fence is here.
    PrepareNonQuery,
    /// A `__snap_`-prefixed identifier in an intercepted statement, read or written.
    ReservedName,
}

impl Blocked {
    /// The editor's wording: IDE register, naming the surface that owns the
    /// capability. The validator's policy diagnostics are this, verbatim.
    pub fn editor_message(self) -> String {
        match self {
            Blocked::CreateExternalTable => {
                "CREATE EXTERNAL TABLE is not supported in the editor. Register tables in \
                 Table Config"
            }
            Blocked::CopyTo => "COPY TO is not supported in the editor. Use Export",
            Blocked::Reset => {
                "RESET is not supported in the editor. Engine options are set in Settings"
            }
            Blocked::CreateView => {
                "CREATE VIEW is not supported in the editor. Write the query and use Save as view"
            }
            Blocked::DropView => {
                "DROP VIEW is not supported in the editor. Drop views from the catalog"
            }
            Blocked::Drop => {
                "DROP is not supported in the editor. Deregister tables from the catalog"
            }
            Blocked::CreateTable => {
                "CREATE TABLE is not supported in the editor. Register tables in Table Config"
            }
            Blocked::Insert => {
                "INSERT is not supported in the editor. Load data through Table Config"
            }
            Blocked::CreateDatabase => "CREATE DATABASE and CREATE SCHEMA are not supported",
            Blocked::Set => {
                "SET is not supported in the editor. Engine options are set in Settings"
            }
            Blocked::Unsupported => {
                "This statement is not supported in the editor. Only SELECT, EXPLAIN, SHOW and \
                 DESCRIBE can run here"
            }
            Blocked::InsertExternal => {
                "INSERT targets internal tables. Load external table data through Table Config"
            }
            Blocked::InsertOverwrite => {
                "INSERT OVERWRITE is not supported. Drop the table and recreate it with \
                 CREATE TABLE AS"
            }
            Blocked::SetOwned => "This option is managed by Strata and cannot be set",
            Blocked::SetRuntime => "Engine runtime options are set in Settings",
            Blocked::SetFormat => "Display options are set in Settings",
            Blocked::PrepareNonQuery => "PREPARE supports queries only",
            Blocked::ReservedName => "Names starting with '__snap_' are reserved for query results",
        }
        .into()
    }
}

/// The router: how `cap` should treat one parsed statement.
///
/// Matching the *parsed* statement keeps this a general classification, not a
/// leading-keyword sniff, and it is a pure function of that statement — a refusal
/// needing context the statement does not carry (an INSERT target's origin, a SET
/// key's class) is the dispatcher's, decided with the same [`Blocked`] vocabulary.
pub fn classify(stmt: &DFStatement, cap: Capability) -> Verdict {
    let (editor, agent) = classify_form(stmt);
    match cap {
        // Reserved names, read and write: a `__snap_` identifier anywhere in a
        // statement the editor would run itself is refused before it can collide with
        // a live snapshot registration — which the provider answers "already exists"
        // to, so the collision costs a *Run*, on a name the same prefix hides from
        // every catalog reader. The agent column is untouched — it already refuses
        // every intercepted form, with its own words.
        Capability::Editor => match editor {
            Verdict::Intercept(_) if names_reserved(stmt) => Verdict::Refuse(Blocked::ReservedName),
            verdict => verdict,
        },
        Capability::Agent => match agent {
            Some(blocked) => Verdict::Refuse(blocked),
            None => Verdict::Query,
        },
    }
}

/// One statement form's two answers: `(what the editor does, what the agent surface
/// refuses it as)`, where `None` is the agent's read-only pass.
///
/// The capability axis is a **column of the same match arm**, never a second
/// traversal: an arm cannot answer one surface and forget the other, and the agent
/// column is AA-01's shipped answer written beside the editor's new one — which is
/// what makes the parity matrix a test of a table rather than of two functions
/// staying in step. The agent never intercepts, and the type says so.
fn classify_form(stmt: &DFStatement) -> (Verdict, Option<Blocked>) {
    let s = match stmt {
        // Typed registration is the second gesture into Table Config's own funnel.
        DFStatement::CreateExternalTable(_) => {
            return intercept(StmtKind::CreateExternalTable, Blocked::CreateExternalTable)
        }
        DFStatement::CopyTo(_) => return intercept(StmtKind::Copy, Blocked::CopyTo),
        DFStatement::Reset(_) => return intercept(StmtKind::Reset, Blocked::Reset),
        DFStatement::Explain(_) => return runnable(),
        DFStatement::Statement(s) => s.as_ref(),
    };
    match s {
        // Runnable: queries + introspection.
        SqlStatement::Query(_)
        | SqlStatement::Explain { .. }
        | SqlStatement::ExplainTable { .. }
        | SqlStatement::ShowTables { .. }
        | SqlStatement::ShowColumns { .. }
        | SqlStatement::ShowFunctions { .. }
        | SqlStatement::ShowVariable { .. }
        | SqlStatement::ShowVariables { .. }
        | SqlStatement::ShowDatabases { .. }
        | SqlStatement::ShowSchemas { .. } => runnable(),
        // `EXECUTE` rides the snapshot pipeline whole — safe because `PREPARE` fenced
        // the inner plan. The agent surface cannot `PREPARE`, so `EXECUTE` is nothing
        // it can name, and it keeps the wildcard answer it shipped with.
        //
        // The one `Verdict::Query` the query path cannot run **yet**: `run_and_snapshot`
        // sets `with_allow_statements(false)` (`query.rs`), so `verify_plan` rejects
        // `LogicalPlan::Statement(Execute)` with DataFusion's wording. Widening that to
        // statements-only for this arm is ED-08's, and it must stay per-dispatch — the
        // read path's triple is all-false on purpose. (`EXECUTE IMMEDIATE` is not a hole:
        // DataFusion answers `not_impl` before any string is planned.)
        SqlStatement::Execute { .. } => (Verdict::Query, Some(Blocked::Unsupported)),
        SqlStatement::CreateView(_) => intercept(StmtKind::CreateView, Blocked::CreateView),
        SqlStatement::Drop { object_type, .. } => match object_type {
            ObjectType::View => intercept(StmtKind::DropView, Blocked::DropView),
            ObjectType::Table => intercept(StmtKind::DropTable, Blocked::Drop),
            _ => refuse(Blocked::Drop),
        },
        // CTAS and a bare column list are different engine methods — one spools a
        // query, the other writes an empty schema-carrying file — so they are named
        // apart here rather than re-derived at dispatch.
        SqlStatement::CreateTable(create) if create.query.is_some() => {
            intercept(StmtKind::Ctas, Blocked::CreateTable)
        }
        SqlStatement::CreateTable(_) => intercept(StmtKind::CreateTable, Blocked::CreateTable),
        SqlStatement::Insert(insert) if insert.overwrite => (
            Verdict::Refuse(Blocked::InsertOverwrite),
            Some(Blocked::Insert),
        ),
        SqlStatement::Insert(_) => intercept(StmtKind::Insert, Blocked::Insert),
        SqlStatement::CreateDatabase { .. } | SqlStatement::CreateSchema { .. } => {
            refuse(Blocked::CreateDatabase)
        }
        SqlStatement::Set(_) => intercept(StmtKind::Set, Blocked::Set),
        SqlStatement::Prepare { statement, .. } => match statement.as_ref() {
            SqlStatement::Query(_) => intercept(StmtKind::Prepare, Blocked::Unsupported),
            _ => (
                Verdict::Refuse(Blocked::PrepareNonQuery),
                Some(Blocked::Unsupported),
            ),
        },
        SqlStatement::Deallocate { .. } => intercept(StmtKind::Deallocate, Blocked::Unsupported),
        SqlStatement::CreateFunction(_) => {
            intercept(StmtKind::CreateFunction, Blocked::Unsupported)
        }
        SqlStatement::DropFunction(_) => intercept(StmtKind::DropFunction, Blocked::Unsupported),
        _ => refuse(Blocked::Unsupported),
    }
}

/// A form both surfaces run.
fn runnable() -> (Verdict, Option<Blocked>) {
    (Verdict::Query, None)
}

/// A form the editor implements as `kind` and the agent surface refuses as `agent`.
fn intercept(kind: StmtKind, agent: Blocked) -> (Verdict, Option<Blocked>) {
    (Verdict::Intercept(kind), Some(agent))
}

/// A form both surfaces refuse, identically.
fn refuse(blocked: Blocked) -> (Verdict, Option<Blocked>) {
    (Verdict::Refuse(blocked), Some(blocked))
}

/// Whether `stmt` names a snapshot-reserved table — one it reads, or one it writes.
///
/// The read half keeps a typed `COPY (SELECT * FROM __snap_3) TO …` from writing
/// `__strata_ord` into a user's file; the write half keeps `CREATE TABLE __snap_2` and
/// friends off the namespace a Run mints into. sqlparser's own `visit_relations` covers
/// the reads and the two sqlparser targets upstream annotates (`CREATE TABLE`'s name
/// and `INSERT`'s), but `CREATE VIEW`'s name and `DROP`'s name list carry no
/// annotation — and DataFusion's own extension statements are outside the visitor
/// entirely — so those targets are named here rather than assumed.
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
                _ => &[],
            };
            targets.iter().any(is_reserved) || reads_reserved(s.as_ref())
        }
        // Never intercepted (`EXPLAIN`), or naming nothing (`RESET`).
        DFStatement::Explain(_) | DFStatement::Reset(_) => false,
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

/// Whether any part of `name` is in the snapshot namespace. The predicate itself is
/// [`is_snapshot_name`], next to the function that mints those names, because the
/// provider's hiding rule asks the same question and the two must not drift.
fn is_reserved(name: &ObjectName) -> bool {
    name.0.iter().any(|part| {
        part.as_ident()
            .is_some_and(|ident| is_snapshot_name(&ident.value))
    })
}

/// One statement the managed-DDL policy refuses — [`policy_verdicts`]' per-statement
/// answer. Carries the classification, never a rendered message: the consumer names
/// its own owning surface ([`Blocked::editor_message`] is the editor's rendering).
#[derive(Clone, Debug, PartialEq)]
pub struct PolicyRefusal {
    /// Zero-based position of the refused statement in the input.
    pub index: usize,
    /// The refused statement as parsed — its canonical rendering, for naming it back
    /// to the caller. Deliberately not a byte slice of the input: the gate never does
    /// offset arithmetic over text it is judging (the editor's spans are approximate
    /// over non-ASCII, which a squiggle tolerates and a gate must not).
    pub statement: String,
    /// Why it is refused.
    pub blocked: Blocked,
}

/// The managed-DDL policy over `sql`, standing alone: parse the input with this
/// session's own dialect and recursion limit (the same resolution
/// `SessionState::sql_to_statement` performs) and return a [`PolicyRefusal`] for every
/// statement [`Capability::Agent`] refuses. The agent gate's whole reading of the
/// policy, and a thin one: this and [`validate`] consume the same [`classify`], one
/// capability apart, so the two surfaces can never disagree about a form — one
/// predicate, two consumers, zero copies.
///
/// **`Err` means the input could not be judged, and the gate fails closed.** The input
/// does not parse (or the configured dialect is unknown), so the caller refuses dispatch
/// and surfaces the returned error — the engine's own parse wording, the same terminal a
/// Run would reach. Unparseable input is never a policy pass, and one broken statement
/// never silently approves its neighbours: `Ok(vec![])` is only ever said about input
/// that parsed whole.
pub fn policy_verdicts(ctx: &SessionContext, sql: &str) -> Result<Vec<PolicyRefusal>, String> {
    Ok(parse(ctx, sql)?
        .into_iter()
        .enumerate()
        .filter_map(|(index, stmt)| match classify(&stmt, Capability::Agent) {
            Verdict::Refuse(blocked) => Some(PolicyRefusal {
                index,
                statement: stmt.to_string(),
                blocked,
            }),
            Verdict::Query | Verdict::Intercept(_) => None,
        })
        .collect())
}

/// The router's answer for a **Run** (ED-02): `sql` parsed as exactly one statement, and what
/// [`Capability::Editor`] does with it.
///
/// **One statement per Run**, which is today's behaviour kept rather than a new rule: a buffer
/// holding several statements is still judged per statement by [`validate`], and Run refuses the
/// batch here with a policy sentence instead of letting DataFusion answer for a limit that is
/// ours. (`SessionContext::sql` refuses a batch too, in its own words about its own parser —
/// which tells the user nothing about what to do next.)
///
/// `Err` is the same fail-closed contract [`policy_verdicts`] has: input that could not be
/// judged is never dispatched.
pub fn classify_one(ctx: &SessionContext, sql: &str) -> Result<(DFStatement, Verdict), String> {
    let mut statements = parse(ctx, sql)?;
    if statements.len() > 1 {
        return Err("Run executes one statement at a time".into());
    }
    // Not unreachable: a buffer of only comments tokenizes fine and parses to nothing, and the
    // blank-buffer gate upstream (`press_query`) does not catch it.
    let stmt = statements.pop_front().ok_or("Nothing to run")?;
    let verdict = classify(&stmt, Capability::Editor);
    Ok((stmt, verdict))
}

/// Parse `sql` with **this session's own** dialect and recursion limit — the same resolution
/// `SessionState::sql_to_statement` performs, and the one parse in front of the router.
///
/// One funnel, because the two gates that call it ([`policy_verdicts`] for the agent,
/// [`classify_one`] for a Run) must not be able to read the same buffer differently: a dialect
/// the agent gate resolved and the Run gate did not would be a statement judged as one form and
/// executed as another.
fn parse(ctx: &SessionContext, sql: &str) -> Result<VecDeque<DFStatement>, String> {
    let state = ctx.state();
    let options = state.config_options();
    let dialect = dialect_from_str(&options.sql_parser.dialect)
        .ok_or_else(|| format!("Unsupported SQL dialect: {}", options.sql_parser.dialect))?;
    DFParserBuilder::new(sql)
        .with_dialect(dialect.as_ref())
        .with_recursion_limit(options.sql_parser.recursion_limit)
        .build()
        .and_then(|mut parser| parser.parse_statements())
        .map_err(|e| e.to_string())
}

/// The span of a statement's leading keyword run (`CREATE EXTERNAL TABLE`,
/// `INSERT INTO`, …) — what a policy diagnostic underlines instead of the whole
/// statement.
fn leading_keywords_span(toks: &[Tok], stmt: &Range<usize>) -> Range<usize> {
    let mut in_stmt = toks
        .iter()
        .filter(|t| t.span.start >= stmt.start && t.span.end <= stmt.end);
    let Some(first) = in_stmt.next() else {
        return stmt.clone();
    };
    let mut end = first.span.end;
    for t in in_stmt.take(2) {
        if t.kind == TokKind::Keyword {
            end = t.span.end;
        } else {
            break;
        }
    }
    first.span.start..end
}

/// Token-level unknown-table check for a statement the **parser rejected**: the name
/// chains in table position (right after `FROM`/`JOIN`) are resolved against the live
/// catalog. Conservative by design — names the statement introduces itself (any ident
/// directly followed by `AS`: CTEs, aliases) and table functions (chain followed by
/// `(`) are skipped, and mixed/quoted multi-part names are left to the planner.
fn check_from_targets(
    ctx: &SessionContext,
    toks: &[Tok],
    stmt: &Range<usize>,
    sql: &str,
    out: &mut Vec<Diagnostic>,
) {
    // A token usable as a table name. sqlparser classes every word in its keyword
    // dictionary as a keyword — including non-reserved ones that are perfectly
    // legal table names (`event`, `user`, `day`, …) — so keyword tokens count as
    // names here too, except the words the parser itself reserves in name position
    // (the same authority the context analyzer's name captures use).
    fn is_name(t: &Tok) -> bool {
        match t.kind {
            TokKind::Ident | TokKind::QuotedIdent => true,
            TokKind::Keyword => !is_reserved_in_name_position(&t.text),
            _ => false,
        }
    }
    let stmt_toks: Vec<&Tok> = toks
        .iter()
        .filter(|t| t.span.start >= stmt.start && t.span.end <= stmt.end)
        .collect();
    let local: Vec<&str> = stmt_toks
        .windows(2)
        .filter(|w| is_name(w[0]) && w[1].kind == TokKind::Keyword && w[1].eq_ci("AS"))
        .map(|w| w[0].text.as_str())
        .collect();

    let mut i = 0;
    while i < stmt_toks.len() {
        let t = stmt_toks[i];
        i += 1;
        if t.kind != TokKind::Keyword || !(t.eq_ci("FROM") || t.eq_ci("JOIN")) {
            continue;
        }
        // The dotted name chain right after the clause keyword.
        let mut parts: Vec<&Tok> = Vec::new();
        let mut j = i;
        while j < stmt_toks.len() && is_name(stmt_toks[j]) {
            parts.push(stmt_toks[j]);
            j += 1;
            if j < stmt_toks.len()
                && stmt_toks[j].kind == TokKind::Punct
                && stmt_toks[j].text == "."
            {
                j += 1;
            } else {
                break;
            }
        }
        if parts.is_empty() {
            continue;
        }
        // A table function call, not a table name.
        if stmt_toks
            .get(j)
            .is_some_and(|t| t.kind == TokKind::Punct && t.text == "(")
        {
            continue;
        }
        if local.iter().any(|l| l.eq_ignore_ascii_case(&parts[0].text)) {
            continue;
        }
        let exists = match parts.as_slice() {
            // A quoted name resolves exactly; `bare` skips the parse-and-normalize.
            [one] if one.kind == TokKind::QuotedIdent => {
                ctx.table_exist(TableReference::bare(one.text.clone()))
            }
            [one] => ctx.table_exist(one.text.as_str()),
            many if many.iter().all(|p| p.kind != TokKind::QuotedIdent) => {
                let name = many
                    .iter()
                    .map(|p| p.text.as_str())
                    .collect::<Vec<_>>()
                    .join(".");
                ctx.table_exist(name.as_str())
            }
            _ => continue,
        };
        if !exists.unwrap_or(true) {
            let span = parts.first().unwrap().span.start..parts.last().unwrap().span.end;
            out.push(diag(
                Severity::Error,
                format!("Table or view '{}' not found", &sql[span.clone()]),
                span,
                sql,
            ));
        }
    }
}

// ---- engine error → diagnostic ---------------------------------------------

/// Fold a parse/plan error for the statement at `stmt` (whose text is `slice`) into a
/// byte-spanned [`Diagnostic`]. Best span first: the planner's own spanned
/// `Diagnostic` (DF 54, `collect_spans` on) → the `Line: N, Column: M` embedded in a
/// parser message → the statement's leading keywords.
fn df_error_diag(
    err: &DataFusionError,
    sql: &str,
    slice: &str,
    stmt: &Range<usize>,
    toks: &[Tok],
) -> Diagnostic {
    if let Some(d) = err.diagnostic() {
        let severity = match d.kind {
            DiagnosticKind::Error => Severity::Error,
            DiagnosticKind::Warning => Severity::Warning,
        };
        let span = d
            .span
            .map(|s| {
                let start = stmt.start + rel_offset(slice, s.start.line, s.start.column);
                let end = stmt.start + rel_offset(slice, s.end.line, s.end.column);
                widen_to_token(start..end, toks)
            })
            .unwrap_or_else(|| leading_keywords_span(toks, stmt));
        return diag(severity, d.message.clone(), span, sql);
    }

    let (mut message, parse_loc) = match err.find_root() {
        DataFusionError::SQL(pe, _) => match pe.as_ref() {
            ParserError::ParserError(s) | ParserError::TokenizerError(s) => {
                (s.clone(), extract_line_col(s))
            }
            ParserError::RecursionLimitExceeded => {
                ("Statement is too deeply nested to parse".to_string(), None)
            }
        },
        root => (root.message().into_owned(), None),
    };
    let span = match parse_loc {
        Some((line, col)) => {
            // The location is part of the span now — drop the noisy suffix.
            if let Some(at) = message.rfind(" at Line: ") {
                message.truncate(at);
            }
            widen_to_token(
                {
                    let at = stmt.start + rel_offset(slice, line as u64, col as u64);
                    at..at
                },
                toks,
            )
        }
        None => leading_keywords_span(toks, stmt),
    };
    diag(Severity::Error, message, span, sql)
}

/// Grow a (possibly empty) span to the full token under its start, so squiggles cover
/// the offending word rather than a single character. Leaves real ranges alone.
fn widen_to_token(span: Range<usize>, toks: &[Tok]) -> Range<usize> {
    if span.end > span.start {
        return span;
    }
    toks.iter()
        .find(|t| t.span.start <= span.start && span.start < t.span.end)
        .map(|t| t.span.clone())
        .unwrap_or(span.start..span.start + 1)
}

/// `Line: N, Column: M` from a sqlparser message, if present.
fn extract_line_col(message: &str) -> Option<(usize, usize)> {
    let at = message.rfind("Line: ")?;
    let rest = &message[at + "Line: ".len()..];
    let line: usize = rest
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    let at = rest.find("Column: ")?;
    let rest = &rest[at + "Column: ".len()..];
    let column: usize = rest
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    Some((line, column))
}

// ---- lexical / structural tier ----------------------------------------------

/// Unbalanced parentheses → point at the offending `(` or `)`.
fn check_parens(toks: &[Tok], sql: &str, out: &mut Vec<Diagnostic>) {
    let mut stack: Vec<Range<usize>> = Vec::new();
    for t in toks {
        if t.kind == TokKind::Punct && t.text == "(" {
            stack.push(t.span.clone());
        } else if t.kind == TokKind::Punct && t.text == ")" {
            if stack.pop().is_none() {
                out.push(diag(
                    Severity::Error,
                    "Unmatched closing parenthesis".into(),
                    t.span.clone(),
                    sql,
                ));
            }
        }
    }
    for open in stack {
        out.push(diag(
            Severity::Error,
            "Unclosed parenthesis".into(),
            open,
            sql,
        ));
    }
}

/// Spot bare identifiers one edit away from a clause keyword — e.g. `FORM` → `FROM` —
/// and return them as `(span, message)` hints. High-confidence only: an identifier
/// that resolves as a table or registered function is never second-guessed. The
/// caller decides how each hint surfaces: merged into an overlapping parse error's
/// message, dropped under a better engine error, or a standalone warning.
fn keyword_typo_hints(
    toks: &[Tok],
    ctx: &SessionContext,
    functions: &FunctionCatalog,
) -> Vec<(Range<usize>, String)> {
    /// A token usable as a name — what an alias or a typo'd keyword's *operand*
    /// looks like.
    fn name_like(t: Option<&Tok>) -> bool {
        t.is_some_and(|t| match t.kind {
            TokKind::Ident | TokKind::QuotedIdent => true,
            TokKind::Keyword => !is_reserved_in_name_position(&t.text),
            _ => false,
        })
    }
    let mut hints = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if t.kind != TokKind::Ident || t.text.len() < 2 {
            continue;
        }
        // Don't second-guess something that actually resolves.
        if ctx.table_exist(t.text.as_str()).unwrap_or(false) || functions.contains(&t.text) {
            continue;
        }
        // An identifier right after a name with nothing name-like following is an
        // alias slot (`FROM orders od WHERE …`), not a typo'd clause keyword — a
        // typo'd keyword would still be followed by its operand (`FORM t`).
        if name_like(i.checked_sub(1).and_then(|p| toks.get(p))) && !name_like(toks.get(i + 1)) {
            continue;
        }
        // A dotted position (`od.amount`, `t.od`) is a qualified reference —
        // clause keywords never touch a dot. Unknown qualifiers are the
        // resolver's finding, with the better message.
        let dot = |t: Option<&Tok>| t.is_some_and(|t| t.kind == TokKind::Punct && t.text == ".");
        if dot(toks.get(i + 1)) || dot(i.checked_sub(1).and_then(|p| toks.get(p))) {
            continue;
        }
        let up = t.text.to_ascii_uppercase();
        if CLAUSE_KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(&up)) {
            continue; // it *is* a keyword (lexer may have classed a contextual word as ident)
        }
        if let Some(kw) = CLAUSE_KEYWORDS.iter().find(|k| near_keyword(&up, k)) {
            hints.push((
                t.span.clone(),
                format!("Unknown keyword '{}'. Did you mean '{}'?", t.text, kw),
            ));
        }
    }
    hints
}

pub(crate) fn diag(
    severity: Severity,
    message: String,
    span: Range<usize>,
    sql: &str,
) -> Diagnostic {
    Diagnostic {
        severity,
        message,
        loc: Some(line_col(sql, span.start)),
        span: Some(span),
    }
}

/// 1-based `line L:C` label for a byte offset (Problems row display).
fn line_col(sql: &str, offset: usize) -> String {
    let off = offset.min(sql.len());
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, ch) in sql.char_indices() {
        if i >= off {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    format!("line {line}:{col}")
}

/// A likely typo of a keyword: differs by ≤1 insert/delete/substitute **or** a single
/// adjacent transposition (Damerau) — so `FORM`→`FROM` (a swap = 2 substitutions) is
/// caught. Case-insensitive; inputs already upper-cased.
fn near_keyword(a: &str, b: &str) -> bool {
    let (av, bv): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if av == bv {
        return false;
    }
    edit_distance_at_most_1(&av, &bv) || adjacent_transposition(&av, &bv)
}

/// ≤1 insertion/deletion/substitution via a two-pointer walk.
fn edit_distance_at_most_1(a: &[char], b: &[char]) -> bool {
    let (la, lb) = (a.len(), b.len());
    if la.abs_diff(lb) > 1 {
        return false;
    }
    let (mut i, mut j, mut edits) = (0usize, 0usize, 0u8);
    while i < la && j < lb {
        if a[i].eq_ignore_ascii_case(&b[j]) {
            i += 1;
            j += 1;
        } else {
            if edits == 1 {
                return false;
            }
            edits += 1;
            match la.cmp(&lb) {
                Ordering::Greater => i += 1,
                Ordering::Less => j += 1,
                Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
            }
        }
    }
    true
}

/// Exactly one adjacent swap (same length, two neighbouring positions swapped).
fn adjacent_transposition(a: &[char], b: &[char]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff: Vec<usize> = (0..a.len())
        .filter(|&i| !a[i].eq_ignore_ascii_case(&b[i]))
        .collect();
    diff.len() == 2
        && diff[1] == diff[0] + 1
        && a[diff[0]].eq_ignore_ascii_case(&b[diff[1]])
        && a[diff[1]].eq_ignore_ascii_case(&b[diff[0]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::arrow::array::{Int64Array, StringArray};
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;
    use datafusion::prelude::SessionConfig;
    use datafusion::sql::sqlparser::dialect::GenericDialect;
    use futures::executor::block_on;
    use std::sync::Arc;

    /// A context shaped like the engine's: `collect_spans` on, one table `t(id, name)`.
    fn ctx() -> SessionContext {
        let mut config = SessionConfig::new();
        config.options_mut().sql_parser.collect_spans = true;
        let ctx = SessionContext::new_with_config(config);
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

    fn run(sql: &str) -> Vec<Diagnostic> {
        block_on(validate(&ctx(), &FunctionCatalog::default(), sql))
    }

    fn spanned<'a>(sql: &'a str, d: &Diagnostic) -> &'a str {
        &sql[d.span.clone().expect("diagnostic span")]
    }

    #[test]
    fn valid_statements_produce_no_diagnostics() {
        assert!(run("SELECT id, name FROM t WHERE id > 1 ORDER BY name").is_empty());
        assert!(run("SELECT * FROM t; EXPLAIN SELECT id FROM t;").is_empty());
        assert!(run("").is_empty());
        assert!(run("-- just a comment").is_empty());
    }

    #[test]
    fn unknown_table_is_spanned() {
        let sql = "SELECT * FROM nope";
        let out = run(sql);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_error());
        assert!(out[0].message.contains("nope"), "{}", out[0].message);
        assert_eq!(spanned(sql, &out[0]), "nope");
    }

    #[test]
    fn unknown_column_is_spanned() {
        let sql = "SELECT missing FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 1);
        assert_eq!(spanned(sql, &out[0]), "missing");
    }

    #[test]
    fn cte_drafts_keep_the_no_from_grace() {
        // The FROM inside the CTE body resolves *that* scope — the main query is
        // still a FROM-less draft and keeps its mid-edit grace.
        assert!(run("WITH x AS (SELECT id FROM t) SELECT draft_col").is_empty());
    }

    #[test]
    fn columns_before_from_stay_quiet() {
        // Mid-composition: no FROM yet, so column references have nothing to
        // resolve against — flagging them is premature, not helpful.
        assert!(run("SELECT name, tags").is_empty());
        assert!(run("SELECT missing").is_empty());
        // Non-column faults still surface without a FROM…
        assert!(!run("SELECT nosuchfn(1)").is_empty());
        // …and once a FROM exists, unknown columns are real again (see
        // `unknown_column_is_spanned`); a FROM-less literal projection stays
        // valid as ever.
        assert!(run("SELECT 1 + 2").is_empty());
    }

    /// The base context plus a registered view `v` over `t` — the Save flow's result.
    fn ctx_with_view() -> SessionContext {
        let ctx = ctx();
        let df = block_on(ctx.sql("CREATE VIEW v AS SELECT id, name FROM t")).expect("create view");
        block_on(df.collect()).expect("apply view");
        ctx
    }

    #[test]
    fn views_resolve_like_tables() {
        let ctx = ctx_with_view();
        let f = FunctionCatalog::default();
        // A view is a first-class query target…
        assert!(block_on(validate(&ctx, &f, "SELECT id FROM v")).is_empty());
        // …its columns are checked through it…
        let sql = "SELECT missing FROM v";
        let out = block_on(validate(&ctx, &f, sql));
        assert_eq!(out.len(), 1);
        assert_eq!(spanned(sql, &out[0]), "missing");
        // …and the broken-parse fallback resolves views too (no false "not found").
        let out = block_on(validate(&ctx, &f, "selct id from v"));
        assert!(
            !out.iter().any(|d| d.message.contains("not found")),
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn unknown_function_errors() {
        let out = run("SELECT not_a_function(id) FROM t");
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("not_a_function"),
            "{}",
            out[0].message
        );
    }

    #[test]
    fn function_arity_is_checked() {
        // Too few and too many arguments both fail the signature at plan time.
        let out = run("SELECT upper() FROM t");
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());

        let out = run("SELECT upper(name, id) FROM t");
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());
    }

    #[test]
    fn function_argument_types_are_checked() {
        // An argument the signature can't accept (a scalar into an array function).
        // Note the bound is the engine's own coercion rules: e.g. Int64 into
        // `character_length` coerces and is deliberately NOT flagged.
        let out = run("SELECT array_length(id) FROM t");
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());

        // A correctly-typed call stays clean.
        assert!(run("SELECT character_length(name) FROM t").is_empty());
    }

    #[test]
    fn expression_type_faults_are_checked() {
        // Un-coercible arithmetic (the analyzer's type-coercion pass).
        let out = run("SELECT name + INTERVAL '1 day' FROM t");
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());
    }

    #[test]
    fn bad_cast_errors() {
        let out = run("SELECT CAST(id AS notatype) FROM t");
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());
    }

    #[test]
    fn statements_accumulate_independently() {
        let sql = "SELECT * FROM nope; SELECT missing FROM t; SELECT id FROM t";
        let out = run(sql);
        assert_eq!(
            out.len(),
            2,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert_eq!(spanned(sql, &out[0]), "nope");
        assert_eq!(spanned(sql, &out[1]), "missing");
    }

    #[test]
    fn syntax_error_is_located() {
        // A mid-statement fault (an expression can't start with AND) — not an
        // incompleteness case, so it reports with a span.
        let sql = "SELECT id FROM t WHERE AND id = 1";
        let out = run(sql);
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].span.is_some());
        assert!(out[0].is_error());
    }

    #[test]
    fn trailing_incomplete_statement_stays_quiet() {
        // A valid prefix at the end of the buffer is typing-in-progress, not a fault.
        assert!(run("select").is_empty());
        assert!(run("SELECT id FROM t WHERE").is_empty());
        assert!(run("SELECT id FROM t ORDER BY").is_empty());

        // …but an incomplete statement followed by another one is a real fault.
        let sql = "SELECT id FROM t WHERE; SELECT id FROM t";
        let out = run(sql);
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());
    }

    #[test]
    fn unterminated_string_reports_lex_error() {
        let out = run("SELECT 'abc FROM t");
        assert_eq!(out.len(), 1);
        assert!(out[0].is_error());
    }

    #[test]
    fn broken_statement_still_flags_unknown_from_target() {
        // A typo'd keyword kills the parse, but the FROM target must still be checked
        // (the token-level fallback). Exactly two diagnostics: the merged parse error
        // on `selct` (hint wording, no separate warning) and the `nope` lookup.
        let sql = "selct * from nope";
        let out = run(sql);
        assert_eq!(
            out.len(),
            2,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(
            out.iter().any(|d| d.is_error()
                && d.message.contains("Did you mean 'SELECT'")
                && spanned(sql, d) == "selct"),
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(
            out.iter().any(|d| d.is_error()
                && d.message.contains("not found")
                && spanned(sql, d) == "nope"),
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn from_target_fallback_stays_conservative() {
        // A real table never gets flagged…
        let sql = "selct * from t";
        assert!(!run(sql).iter().any(|d| d.message.contains("not found")));
        // …nor a qualified name that resolves…
        let sql = "selct * from public.t";
        assert!(!run(sql).iter().any(|d| d.message.contains("not found")));
        // …nor a name the statement introduces itself (CTE).
        let sql = "WITH x AS (SELCT 1) SELECT * FROM x";
        assert!(
            !run(sql).iter().any(|d| d.message.contains("not found")),
            "CTE name must not be flagged"
        );
        // …nor a table-function call in FROM position.
        let sql = "selct * from read_parquet('f.parquet')";
        assert!(!run(sql).iter().any(|d| d.message.contains("not found")));
    }

    #[test]
    fn keyword_like_table_names_are_still_checked() {
        // `event` sits in sqlparser's keyword dictionary (non-reserved), so it lexes
        // as a keyword — the fallback must still treat it as a table name.
        let sql = "selc * from event";
        let out = run(sql);
        assert!(
            out.iter()
                .any(|d| d.message.contains("not found") && spanned(sql, d) == "event"),
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );

        // And a real table that happens to carry a keyword name is not flagged.
        let ctx = ctx();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)])),
            vec![Arc::new(Int64Array::from(vec![1_i64]))],
        )
        .unwrap();
        ctx.register_batch("event", batch).unwrap();
        let out = block_on(validate(&ctx, &FunctionCatalog::default(), sql));
        assert!(
            !out.iter().any(|d| d.message.contains("not found")),
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn keyword_typo_merges_into_the_parse_error() {
        // One diagnostic, not an error and a warning stacked on the same token: the
        // parse error takes the hint's wording.
        let sql = "SELECT * FORM t";
        let out = run(sql);
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());
        assert!(
            out[0].message.contains("Did you mean 'FROM'"),
            "{}",
            out[0].message
        );
        assert_eq!(spanned(sql, &out[0]), "FORM");
    }

    #[test]
    fn typo_hint_defers_to_a_better_engine_error() {
        // `fom` parses fine as a column, so the planner's unknown-column error is the
        // authority — the speculative keyword hint must not add a second row.
        let sql = "SELECT fom FROM t";
        let out = run(sql);
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());
        assert_eq!(spanned(sql, &out[0]), "fom");
    }

    #[test]
    fn the_editor_squiggles_what_the_router_still_refuses() {
        let out = run("CREATE DATABASE other");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].message, Blocked::CreateDatabase.editor_message());

        // A refusal underlines the statement's leading keyword run, not the statement.
        let sql = "DELETE FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].message, Blocked::Unsupported.editor_message());
        assert_eq!(spanned(sql, &out[0]), "DELETE FROM");

        // The refusals inside otherwise-intercepted forms.
        let out = run("INSERT OVERWRITE INTO t VALUES (3, 'c')");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].message, Blocked::InsertOverwrite.editor_message());

        let out = run("PREPARE p AS INSERT INTO t VALUES (3, 'c')");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].message, Blocked::PrepareNonQuery.editor_message());
    }

    /// The statements the editor now runs itself draw no policy squiggle — the whole
    /// point of the `Intercept` verdict, and the thing a `Refuse` used to hide.
    #[test]
    fn an_intercepted_statement_gets_no_squiggle() {
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
        ] {
            let out = run(sql);
            assert!(out.is_empty(), "{sql}: {out:?}");
        }
    }

    /// Interception is not a bypass: an intercepted statement falls through to the
    /// name and semantic tiers exactly as a query does, so typed DDL gets the same
    /// unknown-table diagnostic a `SELECT` would.
    #[test]
    fn an_intercepted_statement_still_gets_name_diagnostics() {
        let out = run("CREATE TABLE copy_t AS SELECT * FROM nope");
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].is_error());
        assert!(
            out[0].message.to_lowercase().contains("nope"),
            "{}",
            out[0].message
        );
    }

    /// Reserved names, both halves: a `__snap_` identifier in a statement the editor
    /// would run itself is refused before it can collide with a live snapshot
    /// registration — which fails as "already exists", on a name the same prefix keeps
    /// invisible.
    #[test]
    fn a_snapshot_name_is_refused_in_an_intercepted_statement() {
        for sql in [
            // Written.
            "CREATE EXTERNAL TABLE __snap_2 STORED AS PARQUET LOCATION 'f.parquet'",
            "CREATE TABLE __snap_2 AS SELECT * FROM t",
            "CREATE TABLE __SNAP_2 (id BIGINT)",
            "CREATE VIEW __snap_2 AS SELECT id FROM t",
            "INSERT INTO __snap_2 VALUES (3, 'c')",
            "DROP TABLE __snap_2",
            "DROP VIEW __snap_2",
            // Read.
            "CREATE TABLE mine AS SELECT * FROM __snap_3",
            "COPY (SELECT * FROM __snap_3) TO 'out.parquet'",
            "COPY __snap_3 TO 'out.parquet'",
        ] {
            let out = run(sql);
            assert_eq!(out.len(), 1, "{sql}: {out:?}");
            assert_eq!(
                out[0].message,
                Blocked::ReservedName.editor_message(),
                "{sql}"
            );
        }
        // A query may still read one — snapshots are how results are addressed at all,
        // and only the intercepted forms can write or export through the prefix.
        assert!(run("SELECT 1 FROM __snap_3")
            .iter()
            .all(|d| d.message != Blocked::ReservedName.editor_message()));
    }

    // ---- the capability axis (ED-01) ---------------------------------------

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

    /// The parity matrix: for every statement form, `Capability::Agent` is the answer
    /// AA-01 shipped — same variant, so same rendered message — beside the editor's
    /// new one. This table *is* the claim that adding the axis changed the agent
    /// surface by not one byte.
    #[test]
    fn the_capability_axis_keeps_the_agent_surfaces_answers() {
        for (sql, editor, agent) in [
            ("SELECT * FROM t", Verdict::Query, Verdict::Query),
            ("EXPLAIN SELECT * FROM t", Verdict::Query, Verdict::Query),
            ("SHOW TABLES", Verdict::Query, Verdict::Query),
            ("DESCRIBE t", Verdict::Query, Verdict::Query),
            (
                "CREATE EXTERNAL TABLE x STORED AS PARQUET LOCATION 'f.parquet'",
                Verdict::Intercept(StmtKind::CreateExternalTable),
                Verdict::Refuse(Blocked::CreateExternalTable),
            ),
            (
                "CREATE TABLE copy_t AS SELECT * FROM t",
                Verdict::Intercept(StmtKind::Ctas),
                Verdict::Refuse(Blocked::CreateTable),
            ),
            (
                "CREATE TABLE cols (id BIGINT)",
                Verdict::Intercept(StmtKind::CreateTable),
                Verdict::Refuse(Blocked::CreateTable),
            ),
            (
                "INSERT INTO t VALUES (3, 'c')",
                Verdict::Intercept(StmtKind::Insert),
                Verdict::Refuse(Blocked::Insert),
            ),
            (
                "INSERT OVERWRITE INTO t VALUES (3, 'c')",
                Verdict::Refuse(Blocked::InsertOverwrite),
                Verdict::Refuse(Blocked::Insert),
            ),
            (
                "DROP TABLE t",
                Verdict::Intercept(StmtKind::DropTable),
                Verdict::Refuse(Blocked::Drop),
            ),
            (
                "DROP SCHEMA s",
                Verdict::Refuse(Blocked::Drop),
                Verdict::Refuse(Blocked::Drop),
            ),
            (
                "CREATE VIEW v AS SELECT id FROM t",
                Verdict::Intercept(StmtKind::CreateView),
                Verdict::Refuse(Blocked::CreateView),
            ),
            (
                "DROP VIEW IF EXISTS v",
                Verdict::Intercept(StmtKind::DropView),
                Verdict::Refuse(Blocked::DropView),
            ),
            (
                "COPY t TO 'out.parquet'",
                Verdict::Intercept(StmtKind::Copy),
                Verdict::Refuse(Blocked::CopyTo),
            ),
            (
                "SET datafusion.execution.batch_size = 1024",
                Verdict::Intercept(StmtKind::Set),
                Verdict::Refuse(Blocked::Set),
            ),
            (
                "RESET datafusion.execution.batch_size",
                Verdict::Intercept(StmtKind::Reset),
                Verdict::Refuse(Blocked::Reset),
            ),
            (
                "PREPARE p AS SELECT id FROM t",
                Verdict::Intercept(StmtKind::Prepare),
                Verdict::Refuse(Blocked::Unsupported),
            ),
            (
                "PREPARE p AS INSERT INTO t VALUES (3, 'c')",
                Verdict::Refuse(Blocked::PrepareNonQuery),
                Verdict::Refuse(Blocked::Unsupported),
            ),
            // EXECUTE rides the snapshot pipeline for the editor; the agent surface
            // cannot PREPARE, so it keeps the wildcard refusal it shipped with.
            (
                "EXECUTE p",
                Verdict::Query,
                Verdict::Refuse(Blocked::Unsupported),
            ),
            (
                "DEALLOCATE p",
                Verdict::Intercept(StmtKind::Deallocate),
                Verdict::Refuse(Blocked::Unsupported),
            ),
            (
                "CREATE FUNCTION f(BIGINT) RETURNS BIGINT RETURN $1 + 1",
                Verdict::Intercept(StmtKind::CreateFunction),
                Verdict::Refuse(Blocked::Unsupported),
            ),
            (
                "DROP FUNCTION f",
                Verdict::Intercept(StmtKind::DropFunction),
                Verdict::Refuse(Blocked::Unsupported),
            ),
            (
                "CREATE DATABASE other",
                Verdict::Refuse(Blocked::CreateDatabase),
                Verdict::Refuse(Blocked::CreateDatabase),
            ),
            (
                "CREATE SCHEMA other",
                Verdict::Refuse(Blocked::CreateDatabase),
                Verdict::Refuse(Blocked::CreateDatabase),
            ),
            (
                "UPDATE t SET name = 'x'",
                Verdict::Refuse(Blocked::Unsupported),
                Verdict::Refuse(Blocked::Unsupported),
            ),
            (
                "DELETE FROM t",
                Verdict::Refuse(Blocked::Unsupported),
                Verdict::Refuse(Blocked::Unsupported),
            ),
            // A reserved name is the editor's refusal alone: the agent already refuses
            // the form, and with the words it has always used.
            (
                "CREATE TABLE __snap_2 AS SELECT * FROM t",
                Verdict::Refuse(Blocked::ReservedName),
                Verdict::Refuse(Blocked::CreateTable),
            ),
        ] {
            let stmt = parse_one(sql);
            assert_eq!(classify(&stmt, Capability::Editor), editor, "{sql}");
            assert_eq!(classify(&stmt, Capability::Agent), agent, "{sql}");
        }
    }

    /// "Not one byte" said in bytes. The matrix above pins the agent's *variants*, and
    /// a variant only implies a message while the wording behind it holds still — but
    /// these variants are now unreachable from the editor, so a future ED task
    /// rewording one (say `Insert`, toward the internal-table story) would silently
    /// change the agent surface with every other test green. `strata-agent`'s own
    /// parity tests cannot catch it either: they compare `AgentError`'s rendering
    /// against `editor_message()`, so both sides move together. These are the literals.
    #[test]
    fn the_agent_paths_messages_are_pinned_verbatim() {
        for (blocked, message) in [
            (
                Blocked::CreateExternalTable,
                "CREATE EXTERNAL TABLE is not supported in the editor. Register tables in \
                 Table Config",
            ),
            (
                Blocked::CopyTo,
                "COPY TO is not supported in the editor. Use Export",
            ),
            (
                Blocked::Reset,
                "RESET is not supported in the editor. Engine options are set in Settings",
            ),
            (
                Blocked::CreateView,
                "CREATE VIEW is not supported in the editor. Write the query and use Save as view",
            ),
            (
                Blocked::DropView,
                "DROP VIEW is not supported in the editor. Drop views from the catalog",
            ),
            (
                Blocked::Drop,
                "DROP is not supported in the editor. Deregister tables from the catalog",
            ),
            (
                Blocked::CreateTable,
                "CREATE TABLE is not supported in the editor. Register tables in Table Config",
            ),
            (
                Blocked::Insert,
                "INSERT is not supported in the editor. Load data through Table Config",
            ),
            (
                Blocked::CreateDatabase,
                "CREATE DATABASE and CREATE SCHEMA are not supported",
            ),
            (
                Blocked::Set,
                "SET is not supported in the editor. Engine options are set in Settings",
            ),
            (
                Blocked::Unsupported,
                "This statement is not supported in the editor. Only SELECT, EXPLAIN, SHOW and \
                 DESCRIBE can run here",
            ),
        ] {
            assert_eq!(blocked.editor_message(), message, "{blocked:?}");
        }
    }

    /// The read-only claim, structurally: whatever the editor implements itself, the
    /// agent surface refuses. No form may become runnable there by growing an
    /// interception.
    #[test]
    fn the_agent_surface_refuses_everything_the_editor_intercepts() {
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
            let stmt = parse_one(sql);
            assert!(
                matches!(classify(&stmt, Capability::Editor), Verdict::Intercept(_)),
                "{sql} must be intercepted"
            );
            assert!(
                matches!(classify(&stmt, Capability::Agent), Verdict::Refuse(_)),
                "{sql} must be refused for the agent"
            );
        }
    }

    // ---- the standalone policy gate (AA-01) --------------------------------

    /// The zero-copies claim, made executable: for every form **both** surfaces
    /// refuse, the gate's classification renders byte-for-byte the message the
    /// editor's diagnostic shows for the same SQL — one `classify`, one
    /// `editor_message`, two consumers. (Where the two now diverge, the divergence
    /// itself is pinned by `the_capability_axis_keeps_the_agent_surfaces_answers`.)
    #[test]
    fn the_gate_and_the_editor_refuse_with_the_same_words() {
        let ctx = ctx();
        for sql in [
            "CREATE DATABASE other",
            "CREATE SCHEMA other",
            "DROP SCHEMA s",
            "UPDATE t SET name = 'x'",
            "DELETE FROM t",
        ] {
            let verdicts = policy_verdicts(&ctx, sql).expect("parses");
            assert_eq!(verdicts.len(), 1, "{sql}");
            let diags = block_on(validate(&ctx, &FunctionCatalog::default(), sql));
            assert_eq!(
                verdicts[0].blocked.editor_message(),
                diags[0].message,
                "{sql}"
            );
        }
    }

    #[test]
    fn runnable_statements_get_no_verdict() {
        let ctx = ctx();
        for sql in [
            "SELECT * FROM t",
            "EXPLAIN SELECT * FROM t",
            "SHOW TABLES",
            "SHOW COLUMNS FROM t",
            "DESCRIBE t",
        ] {
            assert!(
                policy_verdicts(&ctx, sql).expect("parses").is_empty(),
                "{sql}"
            );
        }
    }

    #[test]
    fn a_multi_statement_input_is_judged_per_statement() {
        let sql = "SELECT 1; INSERT INTO t VALUES (1, 'a'); DROP VIEW v";
        let out = policy_verdicts(&ctx(), sql).expect("parses");
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0].index, 1);
        assert_eq!(out[0].blocked, Blocked::Insert);
        assert!(
            out[0].statement.starts_with("INSERT"),
            "{}",
            out[0].statement
        );
        assert_eq!(out[1].index, 2);
        assert_eq!(out[1].blocked, Blocked::DropView);
    }

    /// The gate fails **closed**: input it cannot judge is `Err`, never an empty `Ok`
    /// that reads as a clean pass — and one broken statement never silently approves
    /// its neighbours (the pre-`Result` shape returned `[]` for exactly these).
    #[test]
    fn the_gate_fails_closed_on_input_it_cannot_judge() {
        let ctx = ctx();
        assert!(policy_verdicts(&ctx, "SELEC * FRM t").is_err());
        // A refusal beside a statement that does not parse: still Err, not a pass.
        assert!(policy_verdicts(&ctx, "SELEC 1; INSERT INTO t VALUES (1, 'a')").is_err());
        // A tokenizer-level fault (unterminated string) beside a refusal — the case
        // where the old lex-gated shape returned a clean-looking `[]`.
        assert!(policy_verdicts(&ctx, "INSERT INTO t VALUES (1, 'a'); SELECT 'oops").is_err());
    }

    /// Non-ASCII text ahead of a refusal must not disturb it: the gate parses the
    /// input whole rather than re-slicing it by computed offsets (which mis-split
    /// exactly here — character columns added to byte positions).
    #[test]
    fn a_refusal_behind_multibyte_text_still_lands() {
        let out = policy_verdicts(&ctx(), "SELECT 'caféé'; INSERT INTO t VALUES (1, 'a')")
            .expect("parses");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].index, 1);
        assert_eq!(out[0].blocked, Blocked::Insert);
    }

    /// The dry-plan reaches typed DDL now that the editor intercepts it rather than
    /// refusing it — and planning a DDL statement must still build its node without
    /// executing it (execution lives only in `execute_logical_plan`). This is the
    /// pin on that: the tier-4 pass runs, reports nothing, and creates nothing.
    #[test]
    fn validation_never_mutates_the_session() {
        let ctx = ctx();
        for sql in [
            "CREATE VIEW v AS SELECT id FROM t",
            "CREATE TABLE made AS SELECT id FROM t",
            "DROP TABLE t",
        ] {
            let out = block_on(validate(&ctx, &FunctionCatalog::default(), sql));
            assert!(out.is_empty(), "{sql}: {out:?}");
        }
        assert!(!ctx.table_exist("v").unwrap());
        assert!(!ctx.table_exist("made").unwrap());
        assert!(ctx.table_exist("t").unwrap());
    }

    #[test]
    fn unbalanced_parens_are_flagged() {
        let sql = "SELECT sum(id FROM t";
        let out = run(sql);
        assert!(out.iter().any(|d| d.message.contains("Unclosed")));
    }

    // ---- the native resolver in front of the dry-plan (P2-23) --------------

    fn messages(out: &[Diagnostic]) -> Vec<&str> {
        out.iter().map(|d| d.message.as_str()).collect()
    }

    #[test]
    fn multiple_unknown_columns_all_squiggle() {
        // The headline: the planner stops at the first bad name; the resolver
        // reports them all.
        let sql = "SELECT nme, product_idd FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 2, "{:?}", messages(&out));
        assert!(out.iter().all(|d| d.is_error()));
        assert_eq!(spanned(sql, &out[0]), "nme");
        assert_eq!(spanned(sql, &out[1]), "product_idd");
    }

    #[test]
    fn unknown_table_mutes_its_columns() {
        // The table is the fault; its columns have nothing to resolve against.
        let sql = "SELECT missing FROM nope";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "nope");
    }

    #[test]
    fn qualified_unknown_columns_are_spanned() {
        let sql = "SELECT t.missing FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "t.missing");

        let sql = "SELECT e.missing FROM t e";
        let out = run(sql);
        assert_eq!(out.len(), 1);
        assert_eq!(spanned(sql, &out[0]), "e.missing");

        assert!(run("SELECT e.id FROM t e").is_empty());
    }

    #[test]
    fn unknown_qualifier_is_flagged() {
        let sql = "SELECT x.id FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "x.id");
    }

    #[test]
    fn cte_columns_resolve() {
        let sql = "WITH c AS (SELECT id FROM t) SELECT missing FROM c";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "missing");

        assert!(run("WITH c AS (SELECT id FROM t) SELECT id FROM c").is_empty());
    }

    #[test]
    fn cte_body_columns_are_checked() {
        // The main query is a FROM-less draft (quiet), but the CTE body has a
        // FROM of its own and resolves.
        let sql = "WITH c AS (SELECT missing FROM t) SELECT draft";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "missing");
    }

    #[test]
    fn derived_table_columns_resolve() {
        let sql = "SELECT d.missing FROM (SELECT id FROM t) d";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "d.missing");

        // A wildcard body still expands to checkable columns…
        let sql = "SELECT d.missing FROM (SELECT * FROM t) d";
        let out = run(sql);
        assert_eq!(out.len(), 1);
        assert_eq!(spanned(sql, &out[0]), "d.missing");

        // …while a computed projection is unknowable — quiet, and the planner's
        // own field error is suppressed as mid-edit noise.
        assert!(run("SELECT d.x FROM (SELECT id + 1 FROM t) d").is_empty());
    }

    #[test]
    fn set_op_branches_each_checked() {
        let sql = "SELECT nme FROM t UNION ALL SELECT idd FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 2, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "nme");
        assert_eq!(spanned(sql, &out[1]), "idd");
    }

    #[test]
    fn correlated_subqueries_see_the_outer_scope() {
        assert!(
            run("SELECT id FROM t WHERE EXISTS (SELECT 1 FROM t u WHERE u.id = t.id)").is_empty()
        );

        let sql = "SELECT id FROM t WHERE EXISTS (SELECT 1 FROM t u WHERE u.missing = t.id)";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "u.missing");
    }

    #[test]
    fn select_aliases_are_legal_in_order_and_group() {
        assert!(run("SELECT id AS a FROM t ORDER BY a").is_empty());
        assert!(run("SELECT id FROM t GROUP BY 1").is_empty());

        let sql = "SELECT id AS a FROM t ORDER BY missing";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "missing");
    }

    #[test]
    fn name_faults_defer_type_faults() {
        // With a bad name in the statement the dry-plan is skipped — the type
        // fault surfaces on the next pass, once the name is fixed (the planner
        // never co-reported them either: it fail-fasts on the name).
        let sql = "SELECT nme, name + INTERVAL '1 day' FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "nme");
        // And with the name fixed, the type fault is the diagnostic.
        assert!(!run("SELECT name + INTERVAL '1 day' FROM t").is_empty());
    }

    #[test]
    fn dangling_join_stays_quiet() {
        // Half-written JOINs are valid prefixes — the trailing-incomplete grace.
        assert!(run("SELECT id FROM t JOIN").is_empty());
        assert!(run("SELECT id FROM t LEFT JOIN").is_empty());
    }

    #[test]
    fn half_written_cte_stays_quiet() {
        assert!(run("WITH x AS (SELECT id FROM t)").is_empty());
        assert!(run("WITH x AS (SELECT id FROM t) SELECT").is_empty());
    }

    #[test]
    fn ambiguity_is_still_engine_authoritative() {
        // The resolver proves names exist; *ambiguity* between relations is the
        // planner's judgement and still surfaces.
        let ctx = ctx();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("name", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![1_i64])),
                Arc::new(StringArray::from(vec!["x"])),
            ],
        )
        .unwrap();
        ctx.register_batch("t2", batch).unwrap();
        let out = block_on(validate(
            &ctx,
            &FunctionCatalog::default(),
            "SELECT name FROM t JOIN t2 ON t.id = t2.id",
        ));
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert!(
            out[0].message.to_lowercase().contains("ambiguous"),
            "{}",
            out[0].message
        );
    }

    #[test]
    fn views_get_multi_error_parity() {
        let ctx = ctx_with_view();
        let sql = "SELECT nme, idd FROM v";
        let out = block_on(validate(&ctx, &FunctionCatalog::default(), sql));
        assert_eq!(out.len(), 2, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "nme");
        assert_eq!(spanned(sql, &out[1]), "idd");
    }

    #[test]
    fn aliases_near_keywords_are_not_second_guessed() {
        // `od` is one edit from `ON`, but it sits in an alias slot — flagging a
        // legitimate alias as a keyword typo is lint noise, not help.
        assert!(run("SELECT od.id FROM t od WHERE od.id > 0").is_empty());
        assert!(run("SELECT id AS od FROM t").is_empty());
    }

    #[test]
    fn incompleteness_is_positional_not_textual() {
        // The parser choking *past* the written tokens is incomplete (quiet)…
        assert!(run("SELECT id FROM t WHERE").is_empty());
        // …choking *on* a written token is a real fault, even at the very end.
        assert!(!run("SELECT id FROM t WHERE ORDER").is_empty());
    }
}
