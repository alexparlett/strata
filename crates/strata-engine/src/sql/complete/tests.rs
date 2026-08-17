//! The completion suite: scalpels (one rule per test), the cohesion-review
//! fixes, join intelligence, and the torture corpus with its every-caret sweep.

use super::*;
use std::collections::HashSet;
use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, Field, TimeUnit};

use crate::sql::FunctionCatalog;
use strata_arrow::column_info;
use strata_model::ColumnInfo;

/// `events(user_id, amount, status, ts)` + `users(user_id, name, guid)` + a saved
/// view `spenders(user_id, total)` + a few functions.
fn catalog() -> Catalog {
    fn col(name: &str, dtype: DataType) -> ColumnInfo {
        column_info(&Field::new(name, dtype, true))
    }
    let events = [
        col("user_id", DataType::Int64),
        col("amount", DataType::Float64),
        col("status", DataType::Utf8),
        col("ts", DataType::Timestamp(TimeUnit::Millisecond, None)),
    ];
    let users = [
        col("user_id", DataType::Int64),
        col("name", DataType::Utf8),
        col("guid", DataType::Utf8),
    ];
    let spenders = [
        col("user_id", DataType::Int64),
        col("total", DataType::Float64),
    ];
    Catalog::build(
        [("events", &events[..], false), ("users", &users[..], false)],
        [("spenders", &spenders[..])],
        Arc::new(FunctionCatalog {
            scalar: vec!["round".into(), "lower".into(), "set_bit".into()],
            aggregate: vec!["sum".into(), "count".into()],
            window: vec!["row_number".into()],
        }),
        Vec::new(),
        "generic".into(),
    )
}

/// Run `complete` with the caret at the `|` marker in `sql`.
fn at(sql_with_caret: &str) -> Vec<Completion> {
    let caret = sql_with_caret.find('|').expect("caret marker");
    let sql = sql_with_caret.replace('|', "");
    complete(&sql, caret, &catalog(), false)
}

fn labels(items: &[Completion]) -> Vec<&str> {
    items.iter().map(|c| c.label.as_str()).collect()
}

fn pos(items: &[Completion], label: &str) -> usize {
    items
        .iter()
        .position(|c| c.label.eq_ignore_ascii_case(label))
        .unwrap_or_else(|| panic!("`{label}` not offered: {:?}", labels(items)))
}

fn absent(items: &[Completion], label: &str) {
    assert!(
        !items.iter().any(|c| c.label.eq_ignore_ascii_case(label)),
        "`{label}` unexpectedly offered"
    );
}

#[test]
fn own_column_beats_short_keywords() {
    let items = at("SELECT s| FROM events");
    assert!(pos(&items, "status") < pos(&items, "sum"));
    for kw in ["SET", "SOME", "SORT"] {
        absent(&items, kw);
    }
}

#[test]
fn blank_statement_offers_statement_keywords_first() {
    let items = at("|");
    assert_eq!(pos(&items, "SELECT"), 0);
    let with = pos(&items, "WITH");
    let explain = pos(&items, "EXPLAIN");
    assert!(with < 7 && explain < 7, "{:?}", labels(&items));
    absent(&items, "events");
    absent(&items, "round");
}

#[test]
fn from_target_ranked_by_written_projection() {
    let items = at("SELECT name, guid FROM |");
    assert_eq!(items[0].label, "users", "{:?}", labels(&items));
    pos(&items, "events");
    pos(&items, "spenders");
    let items = at("SELECT e.amount AS spend FROM |");
    assert_eq!(items[0].label, "events", "{:?}", labels(&items));
    let items = at("SELECT total FROM |");
    assert_eq!(items[0].label, "spenders", "{:?}", labels(&items));
}

#[test]
fn fallback_columns_cluster_by_covering_table() {
    let items = at("SELECT user_id, ts, |");
    for winner in ["amount", "status"] {
        for laggard in ["name", "guid", "total"] {
            assert!(
                pos(&items, winner) < pos(&items, laggard),
                "`{winner}` should beat `{laggard}`: {:?}",
                labels(&items)
            );
        }
    }
    pos(&items, "total");
    let items = at("SELECT guid, n|");
    assert_eq!(
        items[pos(&items, "name")].detail.as_deref(),
        Some("users · Utf8")
    );
}

#[test]
fn unknown_projection_columns_never_filter_relations() {
    let items = at("SELECT zzz FROM |");
    pos(&items, "events");
    pos(&items, "users");
    pos(&items, "spenders");
}

#[test]
fn from_target_offers_relations_only() {
    let items = at("SELECT * FROM |");
    assert!(!items.is_empty());
    assert!(items
        .iter()
        .all(|c| matches!(c.kind, CompletionKind::Table | CompletionKind::View)));
    pos(&items, "events");
    pos(&items, "spenders");
}

#[test]
fn from_clause_offers_follow_keywords_first() {
    let items = at("SELECT * FROM events |");
    assert_eq!(pos(&items, "WHERE"), 0, "{:?}", labels(&items));
    assert!(pos(&items, "LEFT JOIN") < 8, "{:?}", labels(&items));
    absent(&items, "user_id");
    absent(&items, "events");
}

#[test]
fn select_star_offers_from_above_functions() {
    let items = at("SELECT * f|");
    assert_eq!(pos(&items, "FROM"), 0, "{:?}", labels(&items));
    absent(&items, "floor");
    absent(&items, "round");
}

#[test]
fn select_item_continuation_offers_from_then_as() {
    let items = at("SELECT sum(amount) | FROM events");
    assert_eq!(pos(&items, "FROM"), 0, "{:?}", labels(&items));
    assert_eq!(pos(&items, "AS"), 1, "{:?}", labels(&items));
    absent(&items, "amount");
    absent(&items, "sum");
}

#[test]
fn where_continuation_offers_boolean_ops_first() {
    let items = at("SELECT * FROM events WHERE amount > 5 a|");
    assert_eq!(pos(&items, "AND"), 0, "{:?}", labels(&items));
    absent(&items, "avg");
}

#[test]
fn where_continuation_ladders_forward_only() {
    let items = at("SELECT * FROM events WHERE amount > 5 |");
    pos(&items, "GROUP BY");
    pos(&items, "ORDER BY");
    pos(&items, "LIMIT");
    absent(&items, "FROM");
    absent(&items, "SELECT");
}

#[test]
fn group_by_continuation_offers_having_first() {
    let items = at("SELECT * FROM events GROUP BY status h|");
    assert_eq!(pos(&items, "HAVING"), 0, "{:?}", labels(&items));
}

#[test]
fn order_by_continuation_offers_direction_first() {
    let items = at("SELECT * FROM events ORDER BY ts |");
    assert_eq!(pos(&items, "ASC"), 0, "{:?}", labels(&items));
    assert_eq!(pos(&items, "DESC"), 1, "{:?}", labels(&items));
    pos(&items, "LIMIT");
}

#[test]
fn on_continuation_resumes_the_join_chain() {
    let items = at("SELECT * FROM events e JOIN users u ON e.user_id = u.user_id |");
    assert_eq!(pos(&items, "AND"), 0, "{:?}", labels(&items));
    pos(&items, "LEFT JOIN");
    pos(&items, "WHERE");
}

#[test]
fn limit_positions() {
    assert!(at("SELECT * FROM events LIMIT |").is_empty());
    let items = at("SELECT * FROM events LIMIT 5 |");
    assert_eq!(pos(&items, "OFFSET"), 0, "{:?}", labels(&items));
}

#[test]
fn multiplication_star_still_offers_operands() {
    let items = at("SELECT amount * | FROM events");
    pos(&items, "status");
    assert!(pos(&items, "status") < pos(&items, "FROM").min(items.len()));
}

#[test]
fn keyword_named_columns_end_items_too() {
    let items = at("SELECT status f|");
    assert_eq!(pos(&items, "FROM"), 0, "{:?}", labels(&items));
    absent(&items, "floor");
}

#[test]
fn connectives_still_start_operands() {
    let items = at("SELECT * FROM events WHERE amount > 5 AND s|");
    assert!(
        pos(&items, "status") < pos(&items, "SELECT").min(items.len()),
        "{:?}",
        labels(&items)
    );
}

#[test]
fn dangling_decimal_stays_quiet() {
    assert!(at("SELECT * FROM events WHERE amount > 1.|").is_empty());
}

#[test]
fn describe_offers_relations() {
    let items = at("DESCRIBE |");
    assert!(!items.is_empty());
    assert!(items
        .iter()
        .all(|c| matches!(c.kind, CompletionKind::Table | CompletionKind::View)));
    pos(&items, "events");
    assert!(at("DESCRIBE events |").is_empty());
}

/// **A statement lead only governs when it leads the statement.** sqlparser classes every word in
/// its dictionary as a `Keyword`, `execute` and `deallocate` included, so without the
/// position guard a table with an `execute` column would have that column govern the rest of its
/// SELECT list — where the offer is prepared statements only, i.e. empty. The user's column names
/// are not ours to reserve.
#[test]
fn a_column_named_execute_does_not_govern_its_clause() {
    fn col(name: &str) -> ColumnInfo {
        column_info(&Field::new(name, DataType::Utf8, true))
    }
    let cols = [col("execute"), col("deallocate"), col("amount")];
    let cat = Catalog::build(
        [("jobs", &cols[..], false)],
        [],
        Arc::default(),
        Vec::new(),
        "generic".into(),
    );
    for sql in ["SELECT execute, ", "SELECT deallocate, "] {
        let items = complete(&format!("{sql} FROM jobs"), sql.len(), &cat, false);
        assert!(
            items.iter().any(|c| c.label == "amount"),
            "{sql}| lost its columns: {:?}",
            items.iter().map(|c| c.label.as_str()).collect::<Vec<_>>()
        );
    }
    assert!(complete("EXECUTE ", 8, &cat, false).is_empty());
}

#[test]
fn select_before_from_falls_back_to_all_columns() {
    let items = at("SELECT na|");
    let p = pos(&items, "name");
    assert_eq!(items[p].kind, CompletionKind::Column);
    assert_eq!(items[p].detail.as_deref(), Some("users · Utf8"));
}

#[test]
fn cte_completes_as_relation_and_dot_resolves() {
    let items = at("WITH recent AS (SELECT amount AS amt FROM events) SELECT * FROM rec|");
    let p = pos(&items, "recent");
    assert_eq!(items[p].detail.as_deref(), Some("cte"));

    let items = at("WITH recent AS (SELECT amount AS amt FROM events) SELECT recent.| FROM recent");
    assert_eq!(labels(&items), vec!["amt"]);
    assert_eq!(items[0].detail.as_deref(), Some("cte"));
}

#[test]
fn cte_bare_projection_columns_are_captured() {
    let items = at("WITH r AS (SELECT amount, status FROM events) SELECT r.| FROM r");
    assert_eq!(labels(&items), vec!["amount", "status"]);
}

#[test]
fn cte_explicit_column_list_wins() {
    let items = at("WITH r (a, b) AS (SELECT amount, status FROM events) SELECT r.| FROM r");
    assert_eq!(labels(&items), vec!["a", "b"]);
}

#[test]
fn alias_dot_resolves_to_that_table() {
    let items = at("SELECT o.| FROM events o");
    assert_eq!(
        labels(&items),
        vec!["ts", "amount", "status", "user_id"],
        "events' columns only (sorted by length then alpha)"
    );
    assert_eq!(items[0].kind, CompletionKind::Column);
}

#[test]
fn unknown_dot_qualifier_is_empty() {
    assert!(at("SELECT x.| FROM events o").is_empty());
}

#[test]
fn hump_match_beats_substring_match() {
    let items = at("SELECT ui| FROM users");
    assert!(pos(&items, "user_id") < pos(&items, "guid"));
}

#[test]
fn gap_subsequence_still_matches() {
    let items = at("SELECT usrid| FROM users");
    pos(&items, "user_id");
}

#[test]
fn prefix_beats_everything_looser() {
    let items = at("SELECT fr| FROM events");
    assert_eq!(items[pos(&items, "FROM")].kind, CompletionKind::Keyword);
    assert_eq!(pos(&items, "FROM"), 0, "{:?}", labels(&items));
}

#[test]
fn rare_keywords_need_a_two_char_prefix() {
    let items = at("SELECT s| FROM events");
    absent(&items, "SERDE");
    let items = at("SELECT serd| FROM events");
    pos(&items, "SERDE");
}

#[test]
fn blocked_ddl_keywords_are_never_offered() {
    for sql in ["|", "SELECT mer| FROM events", "SELECT * FROM events alt|"] {
        let items = at(sql);
        absent(&items, "ALTER");
        absent(&items, "MERGE");
        absent(&items, "TRUNCATE");
    }
}

/// The two statements only a database connection can take are offered like any other lead — the
/// arm refuses a workspace target in its own words, which is a different thing from the editor
/// pretending the verb does not exist.
#[test]
fn the_remote_dml_leads_are_offered_at_a_blank_statement() {
    let items = at("|");
    let _ = pos(&items, "UPDATE");
    let _ = pos(&items, "DELETE FROM");
}

#[test]
fn multi_word_phrases_are_offered() {
    let items = at("SELECT * FROM events gro|");
    assert_eq!(items[pos(&items, "GROUP BY")].kind, CompletionKind::Keyword);
}

#[test]
fn function_inserts_open_paren() {
    let items = at("SELECT rou| FROM events");
    let p = pos(&items, "round");
    assert_eq!(items[p].insert, "round(");
    assert_eq!(items[p].kind, CompletionKind::Function);
}

#[test]
fn replace_span_covers_the_partial_token() {
    let sql = "SELECT sta FROM events";
    let caret = "SELECT sta".len();
    let items = complete(sql, caret, &catalog(), false);
    let p = pos(&items, "status");
    assert_eq!(items[p].replace, 7..10);
}

#[test]
fn empty_position_replace_span_is_caret_caret() {
    let items = at("SELECT * FROM |");
    assert!(!items.is_empty());
    let caret = "SELECT * FROM ".len();
    assert_eq!(items[0].replace, caret..caret);
}

#[test]
fn mid_word_caret_yields_no_partial_and_stays_quiet_on_symbols() {
    let sql = "SELECT status FROM events";
    let caret = "SELECT sta".len();
    let items = complete(sql, caret, &catalog(), false);
    assert!(items.iter().all(|c| c.replace == (caret..caret)));
}

#[test]
fn no_duplicate_kind_label_pairs() {
    let items = at("SELECT * FROM events uni|");
    let mut seen = HashSet::new();
    for c in &items {
        assert!(
            seen.insert((c.kind, c.label.to_ascii_lowercase())),
            "duplicate: {}",
            c.label
        );
    }
}

#[test]
fn select_aliases_referenceable_in_order_by() {
    let items = at("SELECT sum(amount) AS spend FROM events ORDER BY sp|");
    let p = pos(&items, "spend");
    assert_eq!(items[p].detail.as_deref(), Some("alias"));
}

#[test]
fn keyword_accept_normalizes_to_upper_with_trailing_space() {
    let items = at("SELECT * FROM events wher|");
    let p = pos(&items, "WHERE");
    assert_eq!(items[p].insert, "WHERE ");
}

#[test]
fn keyword_space_skipped_when_the_buffer_already_has_one() {
    let items = at("SELECT * FROM events orde| LIMIT 5");
    let p = pos(&items, "ORDER BY");
    assert_eq!(items[p].insert, "ORDER BY");
}

#[test]
fn identifier_accepts_never_add_a_space() {
    let items = at("SELECT sta| FROM events");
    assert_eq!(items[pos(&items, "status")].insert, "status");
    let items = at("SELECT * FROM eve|");
    assert_eq!(items[pos(&items, "events")].insert, "events");
}

#[test]
fn weird_identifiers_insert_quoted() {
    fn col(name: &str) -> ColumnInfo {
        column_info(&Field::new(name, DataType::Utf8, true))
    }
    let cols = [col("Amount USD"), col("order"), col("plain")];
    let cat = Catalog::build(
        [("t", &cols[..], false)],
        [],
        Arc::default(),
        Vec::new(),
        "generic".into(),
    );
    let items = complete("SELECT  FROM t", 7, &cat, false);
    let find = |l: &str| items.iter().find(|c| c.label == l).unwrap().insert.clone();
    assert_eq!(find("Amount USD"), "\"Amount USD\"");
    assert_eq!(find("order"), "\"order\"");
    assert_eq!(find("plain"), "plain");
}

#[test]
fn all_keywords_is_sorted_for_binary_search() {
    assert!(ALL_KEYWORDS.windows(2).all(|w| w[0] <= w[1]));
}

#[test]
fn already_written_columns_sink_in_their_clause() {
    let items = at("SELECT user_id, | FROM events");
    for fresh in ["amount", "status", "ts"] {
        assert!(
            pos(&items, fresh) < pos(&items, "user_id"),
            "{:?}",
            labels(&items)
        );
    }
    pos(&items, "user_id");
    let items = at("SELECT * FROM events GROUP BY status, |");
    assert!(
        pos(&items, "amount") < pos(&items, "status"),
        "{:?}",
        labels(&items)
    );
    let items = at("SELECT * FROM events WHERE amount > 5 AND |");
    assert!(
        pos(&items, "ts") < pos(&items, "amount"),
        "{:?}",
        labels(&items)
    );
    pos(&items, "amount");
}

#[test]
fn select_list_refs_do_not_demote_in_where() {
    let with_projection = at("SELECT ts FROM events WHERE |");
    let plain = at("SELECT amount FROM events WHERE |");
    assert_eq!(labels(&with_projection), labels(&plain));
}

#[test]
fn written_relations_sink_in_join_targets() {
    let items = at("SELECT * FROM events e JOIN |");
    assert!(
        pos(&items, "users") < pos(&items, "events"),
        "{:?}",
        labels(&items)
    );
    assert!(
        pos(&items, "spenders") < pos(&items, "events"),
        "{:?}",
        labels(&items)
    );
    pos(&items, "events");
}

#[test]
fn union_branches_do_not_share_written_refs() {
    assert_eq!(
        labels(&at("SELECT amount FROM events UNION ALL SELECT |")),
        labels(&at("SELECT |"))
    );
}

#[test]
fn on_positions_prefer_cross_side_join_keys() {
    let items = at("SELECT * FROM events e JOIN users u ON e.|");
    assert_eq!(items[0].label, "user_id", "{:?}", labels(&items));
    let items = at("SELECT * FROM events e JOIN users u ON e.user_id = u.|");
    assert_eq!(items[0].label, "user_id", "{:?}", labels(&items));
}

#[test]
fn comparison_rhs_prefers_matching_type_family() {
    let items = at("SELECT * FROM events WHERE amount > |");
    assert!(
        pos(&items, "user_id") < pos(&items, "status"),
        "{:?}",
        labels(&items)
    );
    assert!(
        pos(&items, "user_id") < pos(&items, "ts"),
        "{:?}",
        labels(&items)
    );
    pos(&items, "status");
}

#[test]
fn derived_table_aliases_resolve_like_inline_ctes() {
    let items = at("SELECT t.| FROM (SELECT user_id, amount FROM events) t");
    assert_eq!(labels(&items), vec!["amount", "user_id"]);
    let items = at("SELECT | FROM (SELECT user_id, amount FROM events) t");
    pos(&items, "user_id");
    pos(&items, "amount");
}

#[test]
fn subquery_tails_are_governed_by_the_outer_clause() {
    let items = at("SELECT name FROM users WHERE user_id > (SELECT avg(amount) FROM events) AND |");
    assert_eq!(
        items[0].kind,
        CompletionKind::Column,
        "{:?}",
        labels(&items)
    );
    assert!(
        pos(&items, "name") < pos(&items, "WHERE"),
        "{:?}",
        labels(&items)
    );
}

#[test]
fn grammar_vocabulary_columns_insert_quoted() {
    fn col(name: &str) -> ColumnInfo {
        column_info(&Field::new(name, DataType::Utf8, true))
    }
    let cols = [col("null"), col("case"), col("asc"), col("plain")];
    let cat = Catalog::build(
        [("t", &cols[..], false)],
        [],
        Arc::default(),
        Vec::new(),
        "generic".into(),
    );
    let items = complete("SELECT  FROM t", 7, &cat, false);
    let find = |l: &str| items.iter().find(|c| c.label == l).unwrap().insert.clone();
    assert_eq!(find("null"), "\"null\"");
    assert_eq!(find("case"), "\"case\"");
    assert_eq!(find("asc"), "\"asc\"");
    assert_eq!(find("plain"), "plain");
}

#[test]
fn alias_binding_positions_offer_nothing() {
    assert!(at("SELECT amount AS s| FROM events").is_empty());
    assert!(at("SELECT * FROM events AS |").is_empty());
}

#[test]
fn explain_restarts_the_statement() {
    let items = at("EXPLAIN |");
    assert_eq!(pos(&items, "SELECT"), 0, "{:?}", labels(&items));
    let items = at("EXPLAIN ANALYZE se|");
    assert_eq!(pos(&items, "SELECT"), 0, "{:?}", labels(&items));
}

#[test]
fn show_nouns_stay_quiet() {
    assert!(at("SHOW |").is_empty());
    assert!(at("SHOW TABLES |").is_empty());
}

#[test]
fn select_aliases_only_offered_where_sql_allows_them() {
    let items = at("SELECT sum(amount) AS spend, sp| FROM events");
    absent(&items, "spend");
    let items = at("SELECT sum(amount) AS spend FROM events WHERE sp|");
    absent(&items, "spend");
}

#[test]
fn cte_internal_aliases_do_not_leak_into_the_main_scope() {
    let items =
        at("WITH r AS (SELECT amount AS inner_x FROM events) SELECT * FROM events ORDER BY in|");
    absent(&items, "inner_x");
}

#[test]
fn cte_literal_projections_yield_no_phantom_columns() {
    let items = at("WITH r AS (SELECT NULL FROM events) SELECT r.| FROM r");
    absent(&items, "null");
}

#[test]
fn untokenizable_buffers_stay_quiet_everywhere() {
    assert!(at("SELECT na| FROM events WHERE x = \"oops").is_empty());
}

#[test]
fn manual_trigger_lifts_the_tail_gate() {
    let auto = complete("SELECT s FROM events", 8, &catalog(), false);
    absent(&auto, "SERDE");
    let manual = complete("SELECT s FROM events", 8, &catalog(), true);
    pos(&manual, "SERDE");
}

#[test]
fn policy_and_completion_agree_on_statement_leads() {
    use crate::sql::{classify, Capability, Verdict};
    use datafusion::sql::parser::DFParserBuilder;
    use datafusion::sql::sqlparser::dialect::GenericDialect;
    for blocked in [
        "MERGE",
        "ALTER",
        "TRUNCATE",
        "GRANT",
        "DATABASE",
        "OVERWRITE",
    ] {
        assert!(
            BLOCKED_KEYWORDS
                .iter()
                .any(|b| b.eq_ignore_ascii_case(blocked)),
            "{blocked} must be blocked"
        );
    }
    for allowed in [
        "SELECT", "WITH", "EXPLAIN", "SHOW", "DESCRIBE", "CREATE", "DROP", "TABLE", "VIEW",
        "EXTERNAL", "INSERT", "INTO", "COPY", "STORED", "SET", "RESET", "UPDATE", "DELETE",
    ] {
        assert!(
            !BLOCKED_KEYWORDS
                .iter()
                .any(|b| b.eq_ignore_ascii_case(allowed)),
            "{allowed} must stay offered"
        );
    }

    let tail = |lead: &str| match lead {
        "SELECT" => "SELECT 1",
        "WITH" => "WITH x AS (SELECT 1) SELECT * FROM x",
        "EXPLAIN" => "EXPLAIN SELECT 1",
        "EXPLAIN ANALYZE" => "EXPLAIN ANALYZE SELECT 1",
        "SHOW" => "SHOW datafusion.execution.batch_size",
        "SHOW TABLES" => "SHOW TABLES",
        "DESCRIBE" => "DESCRIBE t",
        "SET" => "SET datafusion.execution.batch_size = 1024",
        "CREATE TABLE" => "CREATE TABLE t AS SELECT 1",
        "CREATE VIEW" => "CREATE VIEW v AS SELECT 1",
        "CREATE EXTERNAL TABLE" => "CREATE EXTERNAL TABLE t STORED AS PARQUET LOCATION 'f.parquet'",
        "CREATE FUNCTION" => "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT RETURN x + 1",
        "CREATE OR REPLACE VIEW" => "CREATE OR REPLACE VIEW v AS SELECT 1",
        "CREATE OR REPLACE FUNCTION" => {
            "CREATE OR REPLACE FUNCTION f(x BIGINT) RETURNS BIGINT RETURN x + 1"
        }
        "INSERT INTO" => "INSERT INTO t VALUES (1)",
        "UPDATE" => "UPDATE t SET n = 1 WHERE n = 0",
        "DELETE FROM" => "DELETE FROM t WHERE n = 0",
        "COPY" => "COPY t TO 'x.parquet'",
        "DROP TABLE" => "DROP TABLE t",
        "DROP VIEW" => "DROP VIEW v",
        "DROP FUNCTION" => "DROP FUNCTION f",
        "PREPARE" => "PREPARE p AS SELECT 1",
        "EXECUTE" => "EXECUTE p",
        "DEALLOCATE" => "DEALLOCATE p",
        "RESET" => "RESET datafusion.execution.batch_size",
        other => panic!("lead '{other}' has no canonical tail — extend this table"),
    };
    for lead in QUERY_LEADS.iter().chain(STATEMENT_LEADS) {
        let sql = tail(lead);
        let mut stmts = DFParserBuilder::new(sql)
            .with_dialect(&GenericDialect {})
            .build()
            .expect("builds")
            .parse_statements()
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
        assert_eq!(stmts.len(), 1, "{sql}");
        let verdict = classify(&stmts.pop_back().unwrap(), Capability::Editor);
        assert!(
            matches!(verdict, Verdict::Query | Verdict::Intercept(_)),
            "{lead} → {sql}: {verdict:?}"
        );
    }
}

#[test]
fn statement_leads_offered_at_start_and_not_at_restarts() {
    let items = at("|");
    for lead in STATEMENT_LEADS {
        pos(&items, lead);
    }
    assert!(
        pos(&items, "SELECT") < pos(&items, "SET"),
        "{:?}",
        labels(&items)
    );
    for sql in [
        "EXPLAIN |",
        "SELECT 1 UNION ALL |",
        "SELECT * FROM (|",
        "COPY (|",
        "CREATE TABLE t AS |",
        "CREATE OR REPLACE VIEW v AS |",
        "PREPARE p AS |",
    ] {
        let items = at(sql);
        assert_eq!(pos(&items, "SELECT"), 0, "{sql}: {:?}", labels(&items));
        for lead in ["SET", "DROP TABLE", "INSERT INTO", "COPY", "PREPARE"] {
            absent(&items, lead);
        }
    }
}

#[test]
fn set_key_completes_and_replaces_the_whole_dotted_chain() {
    let sql = "SET datafusion.exec";
    let items = complete(sql, sql.len(), &catalog(), false);
    let p = pos(&items, "datafusion.execution.batch_size");
    assert_eq!(items[p].insert, "datafusion.execution.batch_size");
    assert_eq!(items[p].replace, 4..19, "{:?}", items[p].replace);
    assert_eq!(items[p].detail.as_deref(), Some("8192"));
    let sql = "SET datafusion.";
    let items = complete(sql, sql.len(), &catalog(), false);
    let p = pos(&items, "datafusion.execution.batch_size");
    assert_eq!(items[p].replace, 4..15);
}

#[test]
fn set_key_pool_agrees_bidirectionally_with_the_dispatch_fence() {
    use crate::ddl::refuse_reserved_key;
    use strata_arrow::config::{DIALECT_KEY, ENGINE_KEYS};
    let items = at("SET |");
    for k in ENGINE_KEYS {
        assert_eq!(
            items.iter().any(|c| c.label == k.key),
            refuse_reserved_key(k.key).is_ok(),
            "{}",
            k.key
        );
    }
    absent(&items, DIALECT_KEY);
    absent(&items, "datafusion.runtime.memory_limit");
    absent(&items, "datafusion.format.null");
    for c in &items {
        assert!(
            c.detail.as_deref().is_some_and(|d| !d.is_empty()),
            "{} has no default to show",
            c.label
        );
    }
    pos(&at("RESET |"), "datafusion.execution.batch_size");
}

#[test]
fn set_value_positions_offer_the_keys_own_vocabulary() {
    let items = at("SET datafusion.execution.coalesce_batches = |");
    assert_eq!(labels(&items), vec!["true", "false"]);
    assert_eq!(items[0].insert, "true");
    let items = at("SET datafusion.explain.format = |");
    assert_eq!(labels(&items), vec!["indent", "tree"]);
    assert!(at("SET datafusion.execution.batch_size = |").is_empty());
    assert!(at("SET datafusion.execution.batch_size = 1024 |").is_empty());
}

#[test]
fn drop_and_insert_operands_filter_by_statement() {
    fn col(name: &str) -> ColumnInfo {
        column_info(&Field::new(name, DataType::Int64, true))
    }
    let cols = [col("id")];
    let scratch = [col("id"), col("ts")];
    let cat = Catalog::build(
        [
            ("events", &cols[..], false),
            ("scratch", &scratch[..], true),
        ],
        [("spenders", &cols[..])],
        Arc::default(),
        Vec::new(),
        "generic".into(),
    );
    let items = complete("DROP TABLE ", 11, &cat, false);
    pos(&items, "events");
    pos(&items, "scratch");
    absent(&items, "spenders");
    let items = complete("DROP TABLE IF EXISTS ", 21, &cat, false);
    pos(&items, "events");
    let items = complete("DROP VIEW ", 10, &cat, false);
    assert_eq!(labels(&items), vec!["spenders"]);
    let items = complete("INSERT INTO ", 12, &cat, false);
    assert_eq!(labels(&items), vec!["scratch"]);
    assert!(complete("CREATE TABLE ", 13, &cat, false).is_empty());
    assert!(complete("PREPARE ", 8, &cat, false).is_empty());
    let items = complete("INSERT INTO scratch (", 21, &cat, false);
    assert_eq!(labels(&items), vec!["id", "ts"]);
    assert!(complete("INSERT INTO events (", 20, &cat, false).is_empty());
    assert!(complete("INSERT INTO scratch VALUES (1, ", 31, &cat, false).is_empty());
    let sql = "INSERT INTO scratch (id, ";
    let items = complete(sql, sql.len(), &cat, false);
    assert_eq!(labels(&items), vec!["ts", "id"]);
    let vals = [col("values"), col("n")];
    let vcat = Catalog::build(
        [("v", &vals[..], true)],
        [],
        Arc::default(),
        Vec::new(),
        "generic".into(),
    );
    let sql = "INSERT INTO v (\"values\", ";
    let items = complete(sql, sql.len(), &vcat, false);
    assert!(items.iter().any(|c| c.label == "n"), "{:?}", labels(&items));
}

#[test]
fn a_parenthesized_body_after_as_restarts_the_ladder() {
    for sql in ["CREATE TABLE t AS (|", "CREATE OR REPLACE VIEW v AS (|"] {
        let items = at(sql);
        assert_eq!(pos(&items, "SELECT"), 0, "{sql}: {:?}", labels(&items));
    }
}

#[test]
fn copy_partition_list_offers_the_sources_columns() {
    let sql = "COPY events TO 'x.parquet' PARTITIONED BY (";
    let items = complete(sql, sql.len(), &catalog(), false);
    pos(&items, "user_id");
    pos(&items, "status");
    absent(&items, "name");
    absent(&items, "SELECT");
    let sql = "COPY (SELECT user_id, amount FROM events) TO 'x' PARTITIONED BY (us";
    let items = complete(sql, sql.len(), &catalog(), false);
    assert_eq!(labels(&items), vec!["user_id"]);
    let sql = "COPY s.events TO 'x' PARTITIONED BY (";
    let items = complete(sql, sql.len(), &catalog(), false);
    pos(&items, "user_id");
    let sql = "COPY events TO 'x' PARTITIONED BY (user_id, ";
    let items = complete(sql, sql.len(), &catalog(), false);
    assert!(
        pos(&items, "amount") < pos(&items, "user_id"),
        "{:?}",
        labels(&items)
    );
    pos(&items, "user_id");
    let sql = "COPY events TO 'x' OPTIONS (";
    assert!(complete(sql, sql.len(), &catalog(), false).is_empty());
}

#[test]
fn deallocate_prepare_still_offers_prepared_names() {
    let prepared = vec![PreparedSym {
        name: "spend".into(),
        params: Vec::new(),
    }];
    let cat = Catalog::build([], [], Arc::default(), prepared, "generic".into());
    let items = complete("DEALLOCATE PREPARE ", 19, &cat, false);
    assert_eq!(labels(&items), vec!["spend"]);
}

#[test]
fn lead_named_columns_never_govern() {
    fn col(name: &str) -> ColumnInfo {
        column_info(&Field::new(name, DataType::Utf8, true))
    }
    let cols = [
        col("set"),
        col("copy"),
        col("drop"),
        col("insert"),
        col("create"),
        col("prepare"),
        col("amount"),
    ];
    let cat = Catalog::build(
        [("jobs", &cols[..], false)],
        [],
        Arc::default(),
        Vec::new(),
        "generic".into(),
    );
    for lead in ["set", "copy", "drop", "insert", "create", "prepare"] {
        let sql = format!("SELECT {lead}, ");
        let items = complete(&format!("{sql} FROM jobs"), sql.len(), &cat, false);
        assert!(
            items.iter().any(|c| c.label == "amount"),
            "{sql}| lost its columns: {:?}",
            labels(&items)
        );
    }
}

#[test]
fn stored_as_offers_exactly_the_formats() {
    let sql = "CREATE EXTERNAL TABLE t STORED AS ";
    let items = complete(sql, sql.len(), &catalog(), false);
    assert_eq!(
        labels(&items),
        vec!["PARQUET", "CSV", "JSON", "NDJSON", "ARROW"]
    );
    assert_eq!(items[0].insert, "PARQUET ");
}

#[test]
fn options_keys_complete_inside_their_quotes() {
    let prefix = "CREATE EXTERNAL TABLE t STORED AS CSV OPTIONS ('";
    let sql = format!("{prefix}format.h')");
    let caret = prefix.len() + "format.h".len();
    let items = complete(&sql, caret, &catalog(), false);
    let p = pos(&items, "format.has_header");
    assert_eq!(items[p].insert, "format.has_header");
    assert_eq!(items[p].replace, prefix.len()..caret);
    assert_eq!(items[p].detail.as_deref(), Some("header row"));
    let sql = format!("{prefix}format.h");
    let items = complete(&sql, sql.len(), &catalog(), false);
    let p = pos(&items, "format.has_header");
    assert_eq!(items[p].replace, prefix.len()..sql.len());
    let sql = format!("{prefix}formatx");
    let caret = prefix.len() + "form".len();
    let items = complete(&sql, caret, &catalog(), false);
    let p = pos(&items, "format.has_header");
    assert_eq!(items[p].replace, prefix.len()..sql.len());
    let sql = prefix.to_string();
    let items = complete(&sql, sql.len(), &catalog(), false);
    pos(&items, "format.delimiter");
    pos(&items, "format.compression");
}

#[test]
fn options_keys_follow_the_written_format() {
    let at_open = |sql: &str| complete(sql, sql.len(), &catalog(), false);
    let items = at_open("CREATE EXTERNAL TABLE t STORED AS JSON OPTIONS ('");
    pos(&items, "format.newline_delimited");
    absent(&items, "format.has_header");
    let items = at_open("CREATE EXTERNAL TABLE t STORED AS NDJSON OPTIONS ('");
    pos(&items, "format.schema_infer_max_rec");
    absent(&items, "format.newline_delimited");
    assert!(at_open("CREATE EXTERNAL TABLE t STORED AS PARQUET OPTIONS ('").is_empty());
    assert!(at_open("CREATE EXTERNAL TABLE t OPTIONS ('").is_empty());
    let items = at_open("CREATE EXTERNAL TABLE t STORED AS CSV OPTIONS ('");
    absent(&items, "aws.region");
    absent(&items, "timeout");
}

#[test]
fn options_values_ride_the_keys_kind() {
    let at_open = |sql: &str| complete(sql, sql.len(), &catalog(), false);
    let items = at_open("CREATE EXTERNAL TABLE t STORED AS CSV OPTIONS ('format.has_header' '");
    assert_eq!(labels(&items), vec!["true", "false"]);
    let items = at_open("CREATE EXTERNAL TABLE t STORED AS CSV OPTIONS ('format.compression' '");
    assert_eq!(
        labels(&items),
        vec!["uncompressed", "gzip", "bzip2", "xz", "zstd"]
    );
    assert!(
        at_open("CREATE EXTERNAL TABLE t STORED AS CSV OPTIONS ('format.delimiter' '").is_empty()
    );
    assert!(at("COPY (SELECT 1) TO '|'").is_empty());
    assert!(at("SELECT 'format.h|' FROM events").is_empty());
}

#[test]
fn create_function_body_offers_arguments_and_functions_only() {
    let sql = "CREATE FUNCTION f(price DOUBLE, qty BIGINT) RETURNS DOUBLE RETURN ";
    let items = complete(sql, sql.len(), &catalog(), false);
    let p = pos(&items, "price");
    assert_eq!(items[p].detail.as_deref(), Some("argument"));
    pos(&items, "qty");
    pos(&items, "round");
    assert!(pos(&items, "price") < pos(&items, "round"));
    absent(&items, "events");
    absent(&items, "amount");
    let sql = "CREATE FUNCTION f(price DOUBLE, qty BIGINT) RETURNS DOUBLE RETURN price * ";
    let items = complete(sql, sql.len(), &catalog(), false);
    pos(&items, "qty");
}

#[test]
fn drop_function_offers_only_created_functions() {
    let mut created: FunctionSym = "my_udf".into();
    created.created = true;
    let cat = Catalog::build(
        [],
        [],
        Arc::new(FunctionCatalog {
            scalar: vec!["round".into(), created],
            aggregate: vec!["sum".into()],
            window: Vec::new(),
        }),
        Vec::new(),
        "generic".into(),
    );
    let items = complete("DROP FUNCTION ", 14, &cat, false);
    assert_eq!(labels(&items), vec!["my_udf"]);
    assert_eq!(
        items[0].insert, "my_udf",
        "the bare name — a DROP takes no call"
    );
    assert_eq!(items[0].detail.as_deref(), Some("session function"));
}

#[test]
fn statement_continuations_offer_the_next_clause() {
    let items = at("CREATE |");
    assert_eq!(pos(&items, "TABLE"), 0, "{:?}", labels(&items));
    pos(&items, "EXTERNAL TABLE");
    pos(&items, "OR REPLACE");
    let items = at("DROP |");
    assert_eq!(pos(&items, "TABLE"), 0, "{:?}", labels(&items));
    pos(&items, "VIEW");
    pos(&items, "FUNCTION");
    let items = at("CREATE TABLE t |");
    assert_eq!(pos(&items, "AS"), 0, "{:?}", labels(&items));
    let items = at("CREATE EXTERNAL TABLE t |");
    assert_eq!(pos(&items, "STORED AS"), 0, "{:?}", labels(&items));
    pos(&items, "LOCATION");
    pos(&items, "PARTITIONED BY");
    pos(&items, "OPTIONS");
    let items = at("COPY events |");
    assert_eq!(pos(&items, "TO"), 0, "{:?}", labels(&items));
    let items = at("INSERT INTO events |");
    assert_eq!(pos(&items, "SELECT"), 0, "{:?}", labels(&items));
    pos(&items, "VALUES");
    let items = at("CREATE FUNCTION f(x BIGINT) |");
    assert_eq!(pos(&items, "RETURNS"), 0, "{:?}", labels(&items));
    pos(&items, "RETURN");
    assert!(at("DROP TABLE events |").is_empty());
}

#[test]
fn query_tails_inside_statements_keep_full_query_completion() {
    let items = at("INSERT INTO events SELECT user_id FROM |");
    pos(&items, "events");
    pos(&items, "users");
    let items = at("CREATE TABLE t AS SELECT amount FROM events WHERE |");
    assert_eq!(
        items[0].kind,
        CompletionKind::Column,
        "{:?}",
        labels(&items)
    );
    pos(&items, "amount");
    let items = at("COPY (SELECT amount FROM events WHERE |");
    pos(&items, "amount");
}

#[test]
fn statement_vocabulary_never_leaks_into_query_positions() {
    let items = at("SELECT * FROM events WHERE |");
    absent(&items, "datafusion.execution.batch_size");
    absent(&items, "STORED AS");
    absent(&items, "LOCATION");
    absent(&items, "format.has_header");
}

#[test]
fn function_first_argument_offers_columns() {
    let items = at("SELECT sum(| FROM events");
    assert_eq!(
        items[0].kind,
        CompletionKind::Column,
        "{:?}",
        labels(&items)
    );
    pos(&items, "amount");
}

#[test]
fn function_later_arguments_offer_columns_after_comma() {
    let items = at("SELECT round(amount, | FROM events");
    assert_eq!(
        items[0].kind,
        CompletionKind::Column,
        "{:?}",
        labels(&items)
    );
}

#[test]
fn nested_call_arguments_filter_like_any_operand() {
    let items = at("SELECT sum(round(am| FROM events");
    assert_eq!(pos(&items, "amount"), 0, "{:?}", labels(&items));
}

#[test]
fn predicate_side_call_arguments_prefer_columns_over_functions() {
    let items = at("SELECT * FROM events WHERE lower(s|");
    assert!(
        pos(&items, "status") < pos(&items, "sum"),
        "{:?}",
        labels(&items)
    );
}

const TORTURE: &[&str] = &[
    "SELECT user_id, sum(amount) OVER (PARTITION BY user_id ORDER BY ts) AS running, \
     lag(amount, 1) OVER (ORDER BY ts) AS prev FROM events \
     WHERE status IN ('ok', 'refund') QUALIFY running > 100 \
     ORDER BY user_id, ts DESC LIMIT 100",
    "SELECT t.user_id, u.name FROM (SELECT user_id, count(*) AS n FROM events \
     GROUP BY user_id) t JOIN users u ON t.user_id = u.user_id \
     WHERE t.n > (SELECT avg(amount) FROM events WHERE status = 'ok')",
    "WITH base AS (SELECT user_id, amount FROM events WHERE status = 'ok'), \
     agg AS (SELECT user_id, sum(amount) AS total FROM base GROUP BY user_id) \
     SELECT u.name, a.total FROM agg a JOIN users u ON u.user_id = a.user_id \
     ORDER BY a.total DESC NULLS LAST",
    "-- daily rollup\nSELECT ts, amount FROM events /* raw tier */ WHERE amount > 0 \
     UNION ALL\nSELECT NULL, total FROM spenders -- aggregated tier",
    "SELECT CASE WHEN amount > 100 THEN 'big' WHEN amount > 10 THEN 'mid' \
     ELSE 'small' END AS bucket, count(*) AS n FROM events \
     GROUP BY bucket HAVING count(*) > 5",
    "SELECT name FROM users WHERE user_id IN (SELECT user_id FROM spenders); \
     SELECT status, ",
    "CREATE EXTERNAL TABLE hits STORED AS CSV LOCATION 'lake/' \
     OPTIONS ('format.has_header' 'true', 'format.delimiter' ';') PARTITIONED BY (year INT)",
    "INSERT INTO scratch SELECT user_id, amount FROM events WHERE status = 'ok'",
    "COPY (SELECT user_id, sum(amount) AS total FROM events GROUP BY user_id) \
     TO 'out/spend.parquet' STORED AS PARQUET PARTITIONED BY (user_id)",
    "CREATE FUNCTION usd(price DOUBLE, rate DOUBLE) RETURNS DOUBLE RETURN price * rate",
    "SET datafusion.execution.batch_size = 1024; SELECT amount FROM events",
    "SELECT ts FROM events; SET datafusion.exec",
];

#[test]
fn torture_sweep_every_caret_position() {
    let cat = catalog();
    for sql in TORTURE {
        for caret in 0..=sql.len() {
            if !sql.is_char_boundary(caret) {
                continue;
            }
            let items = complete(sql, caret, &cat, false);
            assert!(items.len() <= 50, "cap breached at {caret} in {sql:?}");
            for c in &items {
                assert!(
                    c.replace.start <= c.replace.end && c.replace.end <= sql.len(),
                    "bad replace span {:?} at {caret} in {sql:?}",
                    c.replace
                );
                assert!(!c.label.is_empty(), "empty label at {caret}");
            }
        }
    }
}

#[test]
fn torture_probes_window_query() {
    let sql = "SELECT user_id, sum(amount) OVER (PARTITION BY ";
    let items = complete(sql, sql.len(), &catalog(), false);
    assert_eq!(
        items[0].kind,
        CompletionKind::Column,
        "{:?}",
        labels(&items)
    );
    let sql = "SELECT user_id FROM events QUALIFY user_id > 100 o";
    let items = complete(sql, sql.len(), &catalog(), false);
    pos(&items, "ORDER BY");
    absent(&items, "FROM");
}

#[test]
fn torture_probes_cte_of_cte() {
    let sql = "WITH base AS (SELECT user_id FROM events), agg AS (SELECT user_id FROM ba";
    let items = complete(sql, sql.len(), &catalog(), false);
    pos(&items, "base");
    let sql = "WITH base AS (SELECT user_id, amount FROM events), \
               agg AS (SELECT user_id FROM base) SELECT agg.";
    let items = complete(sql, sql.len(), &catalog(), false);
    assert_eq!(labels(&items), vec!["user_id"]);
}

#[test]
fn torture_probes_subquery_positions() {
    let sql = "SELECT name FROM users WHERE user_id > (SELECT avg(amount) FROM ev";
    let items = complete(sql, sql.len(), &catalog(), false);
    pos(&items, "events");
    let sql = "SELECT t. FROM (SELECT user_id FROM events) t";
    let items = complete(sql, "SELECT t.".len(), &catalog(), false);
    assert_eq!(labels(&items), vec!["user_id"]);
}

#[test]
fn torture_probes_union_and_comments() {
    let sql = "SELECT ts FROM events UNION ALL ";
    let items = complete(sql, sql.len(), &catalog(), false);
    assert_eq!(pos(&items, "SELECT"), 0, "{:?}", labels(&items));
    let sql = "-- roll|up\nSELECT ts FROM events";
    let caret = sql.find('|').unwrap();
    let clean = sql.replace('|', "");
    assert!(complete(&clean, caret, &catalog(), false).is_empty());
}

#[test]
fn no_completions_inside_strings_or_comments() {
    assert!(at("SELECT 'ab|c' FROM events").is_empty());
    assert!(at("SELECT * FROM events -- co|mment").is_empty());
    assert!(at("SELECT * /* |note */ FROM events").is_empty());
    assert!(at("SELECT 'ab|").is_empty());
}

#[test]
fn completions_resume_after_a_closed_string() {
    let items = at("SELECT 'x', s| FROM events");
    pos(&items, "status");
}

/// **Qualified names over a database connection** (DB-06) — the offer grows a catalog segment,
/// then a schema segment, and stops where the network would begin.
mod qualified {
    use super::*;
    use crate::sql::symbols::{DatabaseSym, RelationSym, SchemaSym};

    fn relation(name: &str, view: bool) -> RelationSym {
        RelationSym {
            name: name.into(),
            view,
        }
    }

    /// The fixture's catalog plus one live database, `pg`, with two enabled schemas. `orders`
    /// deliberately shares a bare name with nothing in the workspace and `users` deliberately
    /// does — the workspace fixture has a `users` table, which is what proves the two namespaces
    /// are answered apart.
    fn with_pg() -> Catalog {
        catalog().with_databases(vec![DatabaseSym {
            name: "pg".into(),
            schemas: vec![
                SchemaSym {
                    name: "public".into(),
                    relations: vec![
                        relation("orders", false),
                        relation("users", false),
                        relation("big_orders", true),
                        relation("Mixed Case", false),
                    ],
                },
                SchemaSym {
                    name: "analytics".into(),
                    relations: vec![relation("sessions", false)],
                },
            ],
        }])
    }

    /// A second connection that has never answered: a name from the def, and nothing under it.
    fn unconnected() -> Catalog {
        catalog().with_databases(vec![DatabaseSym {
            name: "warehouse".into(),
            schemas: Vec::new(),
        }])
    }

    fn offer(sql_with_caret: &str, cat: &Catalog) -> Vec<Completion> {
        let caret = sql_with_caret.find('|').expect("caret marker");
        complete(&sql_with_caret.replace('|', ""), caret, cat, false)
    }

    #[test]
    fn a_catalog_name_is_offered_at_a_relation_target() {
        let items = offer("SELECT * FROM |", &with_pg());
        let p = pos(&items, "pg");
        assert_eq!(items[p].detail.as_deref(), Some("database"));
        assert!(
            pos(&items, "events") < p && pos(&items, "spenders") < p,
            "a qualifier ranks behind everything that can stand alone: {:?}",
            labels(&items)
        );
    }

    #[test]
    fn a_connection_that_has_not_answered_still_offers_its_name() {
        let items = offer("SELECT * FROM ware|", &unconnected());
        pos(&items, "warehouse");
        assert!(
            offer("SELECT * FROM warehouse.|", &unconnected()).is_empty(),
            "and nothing under it"
        );
    }

    #[test]
    fn the_catalog_segment_offers_its_schemas() {
        let items = offer("SELECT * FROM pg.|", &with_pg());
        assert_eq!(labels(&items), vec!["public", "analytics"]);
        assert_eq!(
            items[pos(&items, "public")].detail.as_deref(),
            Some("pg · schema")
        );
    }

    #[test]
    fn the_schema_segment_offers_its_relations() {
        let items = offer("SELECT * FROM pg.public.|", &with_pg());
        let p = pos(&items, "big_orders");
        assert_eq!(items[p].kind, CompletionKind::View);
        assert_eq!(items[p].detail.as_deref(), Some("view"));
        assert_eq!(
            items[pos(&items, "orders")].kind,
            CompletionKind::Table,
            "{:?}",
            labels(&items)
        );
        absent(&items, "sessions");
    }

    #[test]
    fn a_relation_whose_spelling_is_the_servers_inserts_quoted() {
        let items = offer("SELECT * FROM pg.public.Mixed|", &with_pg());
        assert_eq!(items[pos(&items, "Mixed Case")].insert, "\"Mixed Case\"");
    }

    /// The head of the chain decides the namespace, so a remote qualifier never answers with a
    /// workspace table that happens to share the bare name — and a remote relation's columns are
    /// an introspection, so the third segment offers nothing at all.
    #[test]
    fn a_remote_qualifier_is_never_answered_by_the_workspace() {
        assert!(offer("SELECT pg.public.users.| FROM pg.public.users", &with_pg()).is_empty());
        assert!(offer("SELECT * FROM pg.marketing.|", &with_pg()).is_empty());
        let items = offer("SELECT users.| FROM users", &with_pg());
        let mut columns = labels(&items);
        columns.sort_unstable();
        assert_eq!(
            columns,
            vec!["guid", "name", "user_id"],
            "the workspace table"
        );
    }

    /// One segment is what a relation or an alias is written as and a catalog never is, so an
    /// in-scope name wins a collision there.
    #[test]
    fn a_single_segment_prefers_a_relation_in_scope() {
        let clashing = catalog().with_databases(vec![DatabaseSym {
            name: "users".into(),
            schemas: vec![SchemaSym {
                name: "public".into(),
                relations: vec![relation("orders", false)],
            }],
        }]);
        let items = offer("SELECT users.| FROM users", &clashing);
        pos(&items, "guid");
        absent(&items, "public");
    }

    /// Nothing about the qualified offer reaches the network — it is data on the snapshot, which
    /// is what §7's "synchronous by construction" costs and buys.
    #[test]
    fn a_project_with_no_database_offers_exactly_what_it_did_before() {
        assert!(catalog().databases.is_empty());
        let items = offer("SELECT * FROM |", &catalog());
        absent(&items, "pg");
    }

    /// **A connection's relations are offered where a relation goes** (DB-09), not only behind a
    /// qualifier — the offer catching up with the fact that a bare name resolves. The detail names
    /// the schema it came from, because the label alone cannot say which source it is.
    #[test]
    fn a_remote_relation_is_offered_bare_at_a_relation_target() {
        let items = offer("SELECT * FROM |", &with_pg());
        let p = pos(&items, "orders");
        assert_eq!(items[p].insert, "orders", "a resolvable name inserts bare");
        assert_eq!(items[p].detail.as_deref(), Some("pg.public · table"));
        let v = pos(&items, "big_orders");
        assert_eq!(items[v].kind, CompletionKind::View);
        assert_eq!(items[v].detail.as_deref(), Some("pg.public · view"));
    }

    /// **A name the project's own catalog holds is offered under its qualified name.** The
    /// workspace fixture has a `users` table, so a bare `users` is *its* — the connection's
    /// relation is a different thing and says so, rather than losing its row to the pool's
    /// one-row-per-name rule or offering a spelling that reaches the other source.
    #[test]
    fn a_remote_relation_the_workspace_shadows_is_offered_qualified() {
        let items = offer("SELECT * FROM |", &with_pg());
        let remote = pos(&items, "pg.public.users");
        assert_eq!(items[remote].insert, "pg.public.users");
        assert_eq!(items[remote].detail.as_deref(), Some("pg.public · table"));
        let workspace = pos(&items, "users");
        assert_eq!(
            items[workspace].insert, "users",
            "and the project's own keeps the bare name it answers to"
        );
    }

    /// Two **shown** schemas holding one name is the tie the resolver refuses, so neither is
    /// offered under the bare spelling that would be refused — both are offered qualified.
    #[test]
    fn a_name_two_shown_schemas_hold_is_offered_qualified() {
        let both = catalog().with_databases(vec![DatabaseSym {
            name: "pg".into(),
            schemas: vec![
                SchemaSym {
                    name: "public".into(),
                    relations: vec![relation("sessions", false)],
                },
                SchemaSym {
                    name: "analytics".into(),
                    relations: vec![relation("sessions", false)],
                },
            ],
        }]);
        let items = offer("SELECT * FROM |", &both);
        pos(&items, "pg.public.sessions");
        pos(&items, "pg.analytics.sessions");
        absent(&items, "sessions");
    }

    /// The server's own spelling, quoted the way a statement has to say it — `quote_verbatim`'s
    /// rule reaching the bare offer as well as the qualified one.
    #[test]
    fn a_remote_relation_that_needs_quoting_gets_it() {
        let items = offer("SELECT * FROM |", &with_pg());
        let p = pos(&items, "Mixed Case");
        assert_eq!(items[p].insert, "\"Mixed Case\"");
    }
}
