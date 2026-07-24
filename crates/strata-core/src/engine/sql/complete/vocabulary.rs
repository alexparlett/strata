//! The **declared grammar tables** — the knowledge no parser table encodes,
//! each one a named, documented policy: the clause ladder and its derived
//! continuations, the statement leads the managed-DDL policy allows, the
//! curated common vocabulary, the presentation phrases, and the blocked
//! DDL/DML set (kept honest against `validate::policy_block` by test).

use crate::engine::sql::context::Clause;

/// Statement-lead keywords — the editor's managed-DDL policy allows exactly the
/// query/inspection statements (SELECT/EXPLAIN/SHOW/DESCRIBE + WITH).
pub(super) const STATEMENT_KEYWORDS: &[&str] = &[
    "SELECT",
    "WITH",
    "EXPLAIN",
    "EXPLAIN ANALYZE",
    "SHOW",
    "SHOW TABLES",
    "DESCRIBE",
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
        // `DESCRIBE t` is complete — nothing follows.
        Clause::Describe => {}
        Clause::Start | Clause::Unknown => {
            v.extend(ladder_after(Clause::Select));
            v.extend(EXPR_OPS);
        }
    }
    v
}

/// The common query vocabulary — ranks at the context's keyword tier. Everything else
/// in `ALL_KEYWORDS` is the demoted tail.
pub(super) const CORE_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "HAVING", "QUALIFY", "LIMIT", "OFFSET", "JOIN", "ON", "USING",
    "AS", "AND", "OR", "NOT", "IN", "IS", "NULL", "LIKE", "ILIKE", "BETWEEN", "EXISTS", "CASE",
    "WHEN", "THEN", "ELSE", "END", "CAST", "DISTINCT", "ALL", "UNION", "EXCEPT", "INTERSECT",
    "WITH", "ASC", "DESC", "NULLS", "FIRST", "LAST", "OVER", "ROWS", "RANGE", "TRUE", "FALSE",
    "INTERVAL", "EXPLAIN", "ANALYZE", "SHOW", "DESCRIBE",
];

/// Curated multi-word phrases — `sqlparser` keywords are single tokens, so these read
/// nicer as one completion (`GROUP BY` not `GROUP` then `BY`). Offered alongside the
/// full single-word `ALL_KEYWORDS` set. Query-only; every word here must be a keyword
/// we *don't* block below, so the phrase and its parts stay consistent.
pub(super) const MULTI_WORD: &[&str] = &[
    // clauses
    "GROUP BY",
    "ORDER BY",
    "PARTITION BY",
    "UNION ALL",
    // joins (incl. DataFusion's semi/anti/natural — see the SELECT reference)
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
    // predicates
    "IS NULL",
    "IS NOT NULL",
    "NOT IN",
    "IS DISTINCT FROM",
    "IS NOT DISTINCT FROM",
];

/// DDL/DML keywords excluded from completion — the statements `validate.rs`'s
/// `policy_block` rejects (managed-DDL policy: the editor runs queries +
/// inspection only; views via Save, tables via Table Config, settings via
/// Settings — which is why SET/RESET are here too). Offering what validation
/// squiggles would mislead. Filtered (case-insensitively) out of `ALL_KEYWORDS`;
/// `policy_and_completion_agree` keeps the two encodings from drifting. (Scalar
/// fns like `replace` still come from the engine registry, so blocking the
/// *keyword* doesn't hide the function.)
pub(super) const BLOCKED_KEYWORDS: &[&str] = &[
    // settings surface
    "SET",
    "RESET",
    // create / drop / alter surface
    "CREATE",
    "TABLE",
    "VIEW",
    "EXTERNAL",
    "DATABASE",
    "SCHEMA",
    "DROP",
    "ALTER",
    "TRUNCATE",
    "RENAME",
    "CASCADE",
    "RESTRICT",
    "TEMPORARY",
    "TEMP",
    "UNLOGGED",
    // data mutation
    "INSERT",
    "INTO",
    "UPDATE",
    "DELETE",
    "COPY",
    "MERGE",
    "UPSERT",
    "REPLACE",
    "OVERWRITE",
    "VACUUM",
    // transactions / permissions
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
    // schema objects
    "CONSTRAINT",
    "REFERENCES",
    "INDEX",
    "SEQUENCE",
    "TRIGGER",
    "PROCEDURE",
    "STORED",
];
