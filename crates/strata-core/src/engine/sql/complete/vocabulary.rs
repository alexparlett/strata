//! The **declared grammar tables** — the knowledge no parser table encodes,
//! each one a named, documented policy: the clause ladder and its derived
//! continuations, the statement leads offered first, the curated common
//! vocabulary, the presentation phrases, and the blocked DDL/DML set (kept
//! honest against the statement router, `validate::classify`, by test).

use crate::engine::sql::context::Clause;

/// The query/inspection leads (SELECT/EXPLAIN/SHOW/DESCRIBE + WITH) — offered first
/// at a blank statement **and** at every [`Clause::Restart`] position, because a
/// restart is a fresh *query* (`EXPLAIN DROP TABLE` is nothing Run accepts).
pub(super) const QUERY_LEADS: &[&str] = &[
    "SELECT",
    "WITH",
    "EXPLAIN",
    "EXPLAIN ANALYZE",
    "SHOW",
    "SHOW TABLES",
    "DESCRIBE",
];

/// The statement leads — every statement the router intercepts for the editor
/// (ED-04…ED-10), offered at `Start` only and **after** the query leads: a blank tab
/// is usually a query. Kept honest against `validate::classify` by
/// `policy_and_completion_agree_on_statement_leads`, whose lead → canonical-tail
/// table panics on a lead with no entry — so a lead added here without extending the
/// test fails the suite. (`CREATE TABLE AS` is not a lead: the name sits between
/// `TABLE` and `AS`, so CTAS is reached `CREATE TABLE` → name → `AS`.)
pub(super) const STATEMENT_LEADS: &[&str] = &[
    "SET",
    "CREATE TABLE",
    "CREATE VIEW",
    "CREATE EXTERNAL TABLE",
    "CREATE FUNCTION",
    "CREATE OR REPLACE VIEW",
    "CREATE OR REPLACE FUNCTION",
    "INSERT INTO",
    "COPY",
    "DROP TABLE",
    "DROP VIEW",
    "DROP FUNCTION",
    "PREPARE",
    "EXECUTE",
    "DEALLOCATE",
    "RESET",
];

/// The clause ladder — the canonical clause order of a SELECT statement. A
/// **continuation** position offers the ladder **strictly after** its clause (SQL
/// never revisits an earlier clause; skipping forward is always legal), in ladder
/// order — which is also the likelihood order. This one table replaces any
/// per-position follow-keyword curation.
pub(super) const LADDER: &[(Clause, &[&str])] = &[
    (Clause::Select, &["SELECT"]),
    (Clause::From, &["FROM"]),
    (Clause::Where, &["WHERE"]),
    (Clause::GroupBy, &["GROUP BY"]),
    (Clause::Having, &["HAVING"]),
    (Clause::Qualify, &["QUALIFY"]),
    (Clause::OrderBy, &["ORDER BY"]),
    (Clause::Limit, &["LIMIT"]),
    (Clause::Offset, &["OFFSET"]),
];

/// Set operations — legal after any complete clause; appended to every ladder tail.
pub(super) const SET_OPS: &[&str] = &["UNION ALL", "UNION", "EXCEPT", "INTERSECT"];

/// Expression continuations — the operators that extend a complete operand, legal
/// in every expression clause (`a AND b` is as valid in a SELECT list as in WHERE).
pub(super) const EXPR_OPS: &[&str] = &[
    "AND",
    "OR",
    "IS NULL",
    "IS NOT NULL",
    "IN",
    "NOT IN",
    "BETWEEN",
    "LIKE",
    "ILIKE",
];

/// FROM-zone continuations after a complete relation target: join phrases + the
/// join glue.
pub(super) const JOIN_CONT: &[&str] = &[
    "JOIN",
    "LEFT JOIN",
    "INNER JOIN",
    "RIGHT JOIN",
    "FULL JOIN",
    "CROSS JOIN",
    "NATURAL JOIN",
    "ON",
    "USING",
    "AS",
];

/// ORDER BY item continuations.
pub(super) const ORDER_CONT: &[&str] = &["ASC", "DESC", "NULLS FIRST", "NULLS LAST"];

/// The ladder keywords strictly after `clause` (+ the always-legal set ops). A
/// clause with no rung (`Start`/`Unknown` map their own way in
/// [`continuation_keywords`]) yields only the set ops — never the whole ladder,
/// which would violate the never-revisits invariant.
pub(super) fn ladder_after(clause: Clause) -> impl Iterator<Item = &'static str> {
    let idx = LADDER
        .iter()
        .position(|(c, _)| *c == clause)
        .map(|i| i + 1)
        .unwrap_or(LADDER.len());
    LADDER[idx..]
        .iter()
        .flat_map(|(_, ks)| ks.iter().copied())
        .chain(SET_OPS.iter().copied())
}

/// The continuation-position keyword offer for a clause, best-first: the clause's
/// own internal continuations interleaved with the onward ladder, per what the
/// grammar makes likeliest there. (`On` is nested in the FROM zone, so it resumes
/// the join chain and then FROM's ladder.)
pub(super) fn continuation_keywords(clause: Clause) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    match clause {
        Clause::Select => {
            v.push("FROM");
            v.push("AS");
            v.extend(ladder_after(Clause::From));
            v.extend(EXPR_OPS);
        }
        Clause::From => {
            v.push("WHERE");
            v.extend(JOIN_CONT);
            v.extend(ladder_after(Clause::Where));
        }
        Clause::On => {
            v.extend(EXPR_OPS);
            v.extend(JOIN_CONT);
            v.extend(ladder_after(Clause::From));
        }
        Clause::Where | Clause::Having | Clause::Qualify => {
            v.extend(EXPR_OPS);
            v.extend(ladder_after(clause));
        }
        Clause::GroupBy => {
            v.extend(ladder_after(Clause::GroupBy));
            v.extend(EXPR_OPS);
        }
        Clause::OrderBy => {
            v.extend(ORDER_CONT);
            v.extend(ladder_after(Clause::OrderBy));
            v.extend(EXPR_OPS);
        }
        Clause::Limit | Clause::Offset => {
            v.extend(ladder_after(clause));
        }
        Clause::Describe | Clause::Execute => {}
        Clause::Create => {
            v.extend(["TABLE", "EXTERNAL TABLE", "VIEW", "FUNCTION", "OR REPLACE"]);
        }
        Clause::Drop => {
            v.extend(["TABLE", "VIEW", "FUNCTION"]);
        }
        Clause::CreateTable | Clause::CreateView | Clause::Prepare => {
            v.push("AS");
        }
        Clause::CreateExternal => {
            v.extend(["STORED AS", "LOCATION", "PARTITIONED BY", "OPTIONS"]);
        }
        Clause::CreateFunction => {
            v.extend(["RETURNS", "RETURN"]);
        }
        Clause::DropTable | Clause::DropView | Clause::DropFunction => {}
        Clause::Insert => {
            v.extend(["SELECT", "VALUES"]);
        }
        Clause::Copy => {
            v.extend(["TO", "STORED AS", "PARTITIONED BY", "OPTIONS"]);
        }
        Clause::SetOption => {}
        Clause::Start | Clause::Restart | Clause::Unknown => {
            v.extend(ladder_after(Clause::Select));
            v.extend(EXPR_OPS);
        }
    }
    v
}

/// The common query vocabulary — ranks at the context's keyword tier. Everything else
/// in `ALL_KEYWORDS` is the demoted tail.
pub(super) const CORE_KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "HAVING",
    "QUALIFY",
    "LIMIT",
    "OFFSET",
    "JOIN",
    "ON",
    "USING",
    "AS",
    "AND",
    "OR",
    "NOT",
    "IN",
    "IS",
    "NULL",
    "LIKE",
    "ILIKE",
    "BETWEEN",
    "EXISTS",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "CAST",
    "DISTINCT",
    "ALL",
    "UNION",
    "EXCEPT",
    "INTERSECT",
    "WITH",
    "ASC",
    "DESC",
    "NULLS",
    "FIRST",
    "LAST",
    "OVER",
    "ROWS",
    "RANGE",
    "TRUE",
    "FALSE",
    "INTERVAL",
    "EXPLAIN",
    "ANALYZE",
    "SHOW",
    "DESCRIBE",
];

/// Curated multi-word phrases — `sqlparser` keywords are single tokens, so these read
/// nicer as one completion (`GROUP BY` not `GROUP` then `BY`). Offered alongside the
/// full single-word `ALL_KEYWORDS` set. Query-only; every word here must be a keyword
/// we *don't* block below, so the phrase and its parts stay consistent.
pub(super) const MULTI_WORD: &[&str] = &[
    "GROUP BY",
    "ORDER BY",
    "PARTITION BY",
    "UNION ALL",
    "INNER JOIN",
    "LEFT JOIN",
    "RIGHT JOIN",
    "FULL JOIN",
    "CROSS JOIN",
    "NATURAL JOIN",
    "LEFT OUTER JOIN",
    "RIGHT OUTER JOIN",
    "FULL OUTER JOIN",
    "LEFT SEMI JOIN",
    "RIGHT SEMI JOIN",
    "LEFT ANTI JOIN",
    "RIGHT ANTI JOIN",
    "IS NULL",
    "IS NOT NULL",
    "NOT IN",
    "IS DISTINCT FROM",
    "IS NOT DISTINCT FROM",
];

/// DDL/DML keywords excluded from completion — the words that appear **only** in
/// statement forms `validate.rs`'s router still refuses for
/// [`Capability::Editor`](crate::engine::sql::Capability). Offering what validation
/// squiggles would mislead. Filtered (case-insensitively) out of `ALL_KEYWORDS`;
/// `policy_and_completion_agree` keeps the two encodings from drifting. (Scalar
/// fns like `replace` still come from the engine registry, so blocking the
/// *keyword* doesn't hide the function.)
///
/// A word that leads both an intercepted form and a refused one is **not** here:
/// `CREATE` leads `CREATE TABLE` and `CREATE EXTERNAL TABLE` as well as
/// `CREATE DATABASE`, so the refusal is carried by `DATABASE`/`SCHEMA` alone.
pub(super) const BLOCKED_KEYWORDS: &[&str] = &[
    "DATABASE",
    "SCHEMA",
    "ALTER",
    "TRUNCATE",
    "RENAME",
    "CASCADE",
    "RESTRICT",
    "TEMPORARY",
    "TEMP",
    "UNLOGGED",
    "UPDATE",
    "DELETE",
    "MERGE",
    "UPSERT",
    "OVERWRITE",
    "VACUUM",
    "GRANT",
    "REVOKE",
    "COMMIT",
    "ROLLBACK",
    "SAVEPOINT",
    "BEGIN",
    "START",
    "TRANSACTION",
    "LOCK",
    "UNLOCK",
    "CONSTRAINT",
    "REFERENCES",
    "INDEX",
    "SEQUENCE",
    "TRIGGER",
    "PROCEDURE",
];
