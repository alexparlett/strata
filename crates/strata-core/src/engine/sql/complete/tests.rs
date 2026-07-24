//! The completion suite: scalpels (one rule per test), the cohesion-review
//! fixes, join intelligence, and the torture corpus with its every-caret sweep.

use super::*;
use crate::engine::sql::FunctionCatalog;
use strata_model::ColumnInfo;

/// `events(user_id, amount, status, ts)` + `users(user_id, name, guid)` + a saved
/// view `spenders(user_id, total)` + a few functions.
fn catalog() -> Catalog {
    fn col(name: &str, dtype: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            dtype: dtype.into(),
            kind: strata_model::Kind::from_arrow(dtype),
            nullable: true,
            children: Vec::new(),
            stats: Vec::new(),
        }
    }
    let events = [
        col("user_id", "Int64"),
        col("amount", "Float64"),
        col("status", "Utf8"),
        col("ts", "Timestamp"),
    ];
    let users = [col("user_id", "Int64"), col("name", "Utf8"), col("guid", "Utf8")];
    let spenders = [col("user_id", "Int64"), col("total", "Float64")];
    Catalog::build(
        [
            ("events", &events[..]),
            ("users", &users[..]),
        ],
        [("spenders", &spenders[..])],
        FunctionCatalog {
            scalar: vec!["round".into(), "lower".into(), "set_bit".into()],
            aggregate: vec!["sum".into(), "count".into()],
            window: vec!["row_number".into()],
        },
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

// ---- ranking: the "good suggestions" acceptance ----

#[test]
fn own_column_beats_short_keywords() {
    // The Dioxus-era complaint: typing `s` buried `status` under SET/SOME/SORT.
    let items = at("SELECT s| FROM events");
    assert!(pos(&items, "status") < pos(&items, "sum"));
    for kw in ["SET", "SOME", "SORT"] {
        // Tail keywords need a ≥2-char prefix — at one char they're gone entirely.
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
    // No catalog symbols in a blank statement.
    absent(&items, "events");
    absent(&items, "round");
}

#[test]
fn from_target_ranked_by_written_projection() {
    // `name` + `guid` live in users — it floats; nothing is filtered out.
    let items = at("SELECT name, guid FROM |");
    assert_eq!(items[0].label, "users", "{:?}", labels(&items));
    pos(&items, "events");
    pos(&items, "spenders");
    // Qualified refs contribute their column part; aliases don't.
    let items = at("SELECT e.amount AS spend FROM |");
    assert_eq!(items[0].label, "events", "{:?}", labels(&items));
    // A view's columns rank it the same way.
    let items = at("SELECT total FROM |");
    assert_eq!(items[0].label, "spenders", "{:?}", labels(&items));
}

#[test]
fn fallback_columns_cluster_by_covering_table() {
    // `user_id` + `ts` are both in events; users/spenders cover only one.
    // The next suggestion clusters toward the table that could supply
    // everything written so far — events' other columns lead.
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
    // Rank only: the non-covering tables' columns are still offered.
    pos(&items, "total");
    // And a typed partial still filters within the clustering.
    let items = at("SELECT guid, n|");
    assert_eq!(items[pos(&items, "name")].detail.as_deref(), Some("users · Utf8"));
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
    pos(&items, "spenders"); // the view
}

#[test]
fn from_clause_offers_follow_keywords_first() {
    let items = at("SELECT * FROM events |");
    assert_eq!(pos(&items, "WHERE"), 0, "{:?}", labels(&items));
    assert!(pos(&items, "LEFT JOIN") < 8, "{:?}", labels(&items));
    // No columns/relations at this position.
    absent(&items, "user_id");
    absent(&items, "events");
}

// ---- continuation positions: the clause ladder ----

#[test]
fn select_star_offers_from_above_functions() {
    // The reported bug: `SELECT * f` ranked floor/flatten/… above FROM.
    let items = at("SELECT * f|");
    assert_eq!(pos(&items, "FROM"), 0, "{:?}", labels(&items));
    // A completed projection can't be followed by a fresh function call.
    absent(&items, "floor");
    absent(&items, "round");
}

#[test]
fn select_item_continuation_offers_from_then_as() {
    let items = at("SELECT sum(amount) | FROM events");
    // (FROM already written later in the buffer doesn't matter — the position
    // after a complete item still ladders forward.)
    assert_eq!(pos(&items, "FROM"), 0, "{:?}", labels(&items));
    assert_eq!(pos(&items, "AS"), 1, "{:?}", labels(&items));
    absent(&items, "amount");
    absent(&items, "sum");
}

#[test]
fn where_continuation_offers_boolean_ops_first() {
    let items = at("SELECT * FROM events WHERE amount > 5 a|");
    assert_eq!(pos(&items, "AND"), 0, "{:?}", labels(&items));
    absent(&items, "avg"); // no fresh operands after a complete one
}

#[test]
fn where_continuation_ladders_forward_only() {
    let items = at("SELECT * FROM events WHERE amount > 5 |");
    pos(&items, "GROUP BY");
    pos(&items, "ORDER BY");
    pos(&items, "LIMIT");
    // Never backwards down the ladder.
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
    // The number position offers nothing…
    assert!(at("SELECT * FROM events LIMIT |").is_empty());
    // …and after the number the ladder continues.
    let items = at("SELECT * FROM events LIMIT 5 |");
    assert_eq!(pos(&items, "OFFSET"), 0, "{:?}", labels(&items));
}

#[test]
fn multiplication_star_still_offers_operands() {
    let items = at("SELECT amount * | FROM events");
    pos(&items, "status"); // operand position — columns lead
    assert!(pos(&items, "status") < pos(&items, "FROM").min(items.len()));
}

#[test]
fn keyword_named_columns_end_items_too() {
    // `status` is a keyword to sqlparser but a column here — the continuation
    // test must treat it exactly like a plain identifier (the `SELECT * f`
    // bug's keyword-named sibling).
    let items = at("SELECT status f|");
    assert_eq!(pos(&items, "FROM"), 0, "{:?}", labels(&items));
    absent(&items, "floor");
}

#[test]
fn connectives_still_start_operands() {
    // After `AND` an operand begins — columns lead, FROM stays down-list.
    let items = at("SELECT * FROM events WHERE amount > 5 AND s|");
    assert!(
        pos(&items, "status") < pos(&items, "SELECT").min(items.len()),
        "{:?}",
        labels(&items)
    );
}

#[test]
fn dangling_decimal_stays_quiet() {
    // `1.` absorbs the dot into the number token — mid-literal, not a
    // qualifier; the guard keeps the popup shut (same stance as strings).
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
    // And after the relation, nothing — the statement is complete.
    assert!(at("DESCRIBE events |").is_empty());
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
    // `ui` → user_id (word-boundary) above guid (contiguous substring).
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
    // FROM (prefix) must beat any hump/substring/subsequence match.
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
    for sql in ["|", "SELECT cre| FROM events", "SELECT * FROM events ins|"] {
        let items = at(sql);
        absent(&items, "CREATE");
        absent(&items, "INSERT");
    }
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
    // Caret inside `status` (not at its end) — no partial token is recognized, so
    // this behaves like an empty-partial Expr/SelectList position.
    let sql = "SELECT status FROM events";
    let caret = "SELECT sta".len();
    let items = complete(sql, caret, &catalog(), false);
    assert!(items.iter().all(|c| c.replace == (caret..caret)));
}

#[test]
fn no_duplicate_kind_label_pairs() {
    let items = at("SELECT * FROM events uni|");
    let mut seen = std::collections::HashSet::new();
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

// ---- insert semantics ----

#[test]
fn keyword_accept_normalizes_to_upper_with_trailing_space() {
    // A keyword is always followed by something — accept types the space too.
    let items = at("SELECT * FROM events wher|");
    let p = pos(&items, "WHERE");
    assert_eq!(items[p].insert, "WHERE ");
}

#[test]
fn keyword_space_skipped_when_the_buffer_already_has_one() {
    // Re-completing a word mid-statement: whitespace already follows the span.
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
        ColumnInfo {
            name: name.into(),
            dtype: "Utf8".into(),
            kind: strata_model::Kind::Str,
            nullable: true,
            children: Vec::new(),
            stats: Vec::new(),
        }
    }
    let cols = [col("Amount USD"), col("order"), col("plain")];
    let cat = Catalog::build([("t", &cols[..])], [], FunctionCatalog::default());
    let items = complete("SELECT  FROM t", 7, &cat, false);
    let find = |l: &str| items.iter().find(|c| c.label == l).unwrap().insert.clone();
    assert_eq!(find("Amount USD"), "\"Amount USD\"");
    assert_eq!(find("order"), "\"order\""); // keyword collision
    assert_eq!(find("plain"), "plain");
}

#[test]
fn all_keywords_is_sorted_for_binary_search() {
    // `needs_quoting` binary-searches ALL_KEYWORDS — guard the assumption.
    assert!(ALL_KEYWORDS.windows(2).all(|w| w[0] <= w[1]));
}

// ---- written-demotion + join intelligence ----

#[test]
fn already_written_columns_sink_in_their_clause() {
    // The SELECT list: a projected column is the less likely next projection…
    let items = at("SELECT user_id, | FROM events");
    for fresh in ["amount", "status", "ts"] {
        assert!(
            pos(&items, fresh) < pos(&items, "user_id"),
            "{:?}",
            labels(&items)
        );
    }
    pos(&items, "user_id"); // …but transformations reuse columns: never filtered.
    // The same uniform rule in GROUP BY…
    let items = at("SELECT * FROM events GROUP BY status, |");
    assert!(pos(&items, "amount") < pos(&items, "status"), "{:?}", labels(&items));
    // …and (deliberately mildly) in WHERE.
    let items = at("SELECT * FROM events WHERE amount > 5 AND |");
    assert!(pos(&items, "ts") < pos(&items, "amount"), "{:?}", labels(&items));
    pos(&items, "amount");
}

#[test]
fn select_list_refs_do_not_demote_in_where() {
    // Filtering on a projected column is idiomatic — the demotion region is
    // the caret's own clause, not the select list.
    let with_projection = at("SELECT ts FROM events WHERE |");
    let plain = at("SELECT amount FROM events WHERE |");
    assert_eq!(labels(&with_projection), labels(&plain));
}

#[test]
fn written_relations_sink_in_join_targets() {
    let items = at("SELECT * FROM events e JOIN |");
    assert!(pos(&items, "users") < pos(&items, "events"), "{:?}", labels(&items));
    assert!(pos(&items, "spenders") < pos(&items, "events"), "{:?}", labels(&items));
    pos(&items, "events"); // self-joins stay possible
}

#[test]
fn union_branches_do_not_share_written_refs() {
    // Set-op branches repeat each other's shapes by design — branch 1's refs
    // must not demote (or coverage-boost) branch 2's fresh list.
    assert_eq!(
        labels(&at("SELECT amount FROM events UNION ALL SELECT |")),
        labels(&at("SELECT |"))
    );
}

#[test]
fn on_positions_prefer_cross_side_join_keys() {
    // `user_id` exists on both sides — the probable equi-key floats.
    let items = at("SELECT * FROM events e JOIN users u ON e.|");
    assert_eq!(items[0].label, "user_id", "{:?}", labels(&items));
    // And on the far side of the comparison, name + type affinity align.
    let items = at("SELECT * FROM events e JOIN users u ON e.user_id = u.|");
    assert_eq!(items[0].label, "user_id", "{:?}", labels(&items));
}

#[test]
fn comparison_rhs_prefers_matching_type_family() {
    // `amount` is Float64 → numeric candidates float; Utf8/Timestamp sink.
    let items = at("SELECT * FROM events WHERE amount > |");
    assert!(
        pos(&items, "user_id") < pos(&items, "status"),
        "{:?}",
        labels(&items)
    );
    assert!(pos(&items, "user_id") < pos(&items, "ts"), "{:?}", labels(&items));
    pos(&items, "status"); // casts stay possible — never filtered
}

#[test]
fn derived_table_aliases_resolve_like_inline_ctes() {
    let items = at("SELECT t.| FROM (SELECT user_id, amount FROM events) t");
    assert_eq!(labels(&items), vec!["amount", "user_id"]);
    // And their columns resolve in scope.
    let items = at("SELECT | FROM (SELECT user_id, amount FROM events) t");
    pos(&items, "user_id");
    pos(&items, "amount");
}

#[test]
fn subquery_tails_are_governed_by_the_outer_clause() {
    // The subquery's FROM must not leak governance into the outer WHERE: this
    // is an operand position (columns lead), not a From-continuation (which
    // would put WHERE/JOIN first and no columns at all).
    let items =
        at("SELECT name FROM users WHERE user_id > (SELECT avg(amount) FROM events) AND |");
    assert_eq!(items[0].kind, CompletionKind::Column, "{:?}", labels(&items));
    assert!(pos(&items, "name") < pos(&items, "WHERE"), "{:?}", labels(&items));
}

// ---- the cohesion-review fixes ----

#[test]
fn grammar_vocabulary_columns_insert_quoted() {
    // A column named `null` inserted bare selects the literal — silently
    // wrong data; `case` breaks the parse. The collision set unions the
    // model's own grammar tables, not just the parser's reserved words.
    fn col(name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            dtype: "Utf8".into(),
            kind: strata_model::Kind::Str,
            nullable: true,
            children: Vec::new(),
            stats: Vec::new(),
        }
    }
    let cols = [col("null"), col("case"), col("asc"), col("plain")];
    let cat = Catalog::build([("t", &cols[..])], [], FunctionCatalog::default());
    let items = complete("SELECT  FROM t", 7, &cat, false);
    let find = |l: &str| items.iter().find(|c| c.label == l).unwrap().insert.clone();
    assert_eq!(find("null"), "\"null\"");
    assert_eq!(find("case"), "\"case\"");
    assert_eq!(find("asc"), "\"asc\"");
    assert_eq!(find("plain"), "plain");
}

#[test]
fn alias_binding_positions_offer_nothing() {
    // A name is being invented — nothing existing completes it.
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
    // SHOW's nouns are unmodeled — silence beats a junk ladder offer.
    assert!(at("SHOW |").is_empty());
    assert!(at("SHOW TABLES |").is_empty());
}

#[test]
fn select_aliases_only_offered_where_sql_allows_them() {
    // `spend` is referenceable in ORDER BY (see the sibling test) but not
    // back inside the SELECT list or WHERE — the validator would squiggle it.
    let items = at("SELECT sum(amount) AS spend, sp| FROM events");
    absent(&items, "spend");
    let items = at("SELECT sum(amount) AS spend FROM events WHERE sp|");
    absent(&items, "spend");
}

#[test]
fn cte_internal_aliases_do_not_leak_into_the_main_scope() {
    // `inner_x` is r's column when r is in scope — but here the main query
    // reads `events`, so the CTE-internal alias must not surface as a
    // select-alias of the outer statement.
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
    // An unterminated quoted ident poisons the whole token stream — every
    // position would masquerade as a blank statement. Quiet beats mis-offer.
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
    // Spot contract with `validate::policy_block`: blocked statements' lead
    // keywords are never offered; allowed leads are never blocked. (The full
    // derivation is P2-23's resolver's job.)
    for blocked in ["CREATE", "INSERT", "UPDATE", "DELETE", "COPY", "SET", "RESET"] {
        assert!(
            BLOCKED_KEYWORDS.iter().any(|b| b.eq_ignore_ascii_case(blocked)),
            "{blocked} must be blocked"
        );
    }
    for allowed in ["SELECT", "WITH", "EXPLAIN", "SHOW", "DESCRIBE"] {
        assert!(
            !BLOCKED_KEYWORDS.iter().any(|b| b.eq_ignore_ascii_case(allowed)),
            "{allowed} must stay offered"
        );
    }
}

// ---- function-argument positions ----
// The value suggestions inside a call — what the accept-chain lands on after
// `sum(`. (Type-aware narrowing — only numeric columns for `sum` — needs the
// registry's signature metadata and belongs to P2-22.)

#[test]
fn function_first_argument_offers_columns() {
    let items = at("SELECT sum(| FROM events");
    assert_eq!(items[0].kind, CompletionKind::Column, "{:?}", labels(&items));
    pos(&items, "amount");
}

#[test]
fn function_later_arguments_offer_columns_after_comma() {
    let items = at("SELECT round(amount, | FROM events");
    assert_eq!(items[0].kind, CompletionKind::Column, "{:?}", labels(&items));
}

#[test]
fn nested_call_arguments_filter_like_any_operand() {
    let items = at("SELECT sum(round(am| FROM events");
    assert_eq!(pos(&items, "amount"), 0, "{:?}", labels(&items));
}

#[test]
fn predicate_side_call_arguments_prefer_columns_over_functions() {
    // `s` matches both the `status` column and `sum`/`set_bit` — the column
    // leads at an operand position, in a WHERE as in a SELECT.
    let items = at("SELECT * FROM events WHERE lower(s|");
    assert!(
        pos(&items, "status") < pos(&items, "sum"),
        "{:?}",
        labels(&items)
    );
}

// ---- torture corpus: realistic analyst SQL ----
// The unit tests above are scalpels — one rule per query. These are the
// stress tier: window functions, nested subqueries, CTE-of-CTE, unions,
// interleaved comments. Two layers: an every-caret sweep (the scanners must
// never panic and always respect the output invariants, at every byte of
// every query) and targeted probes at the nasty positions.

const TORTURE: &[&str] = &[
    // Window functions + QUALIFY + IN-list.
    "SELECT user_id, sum(amount) OVER (PARTITION BY user_id ORDER BY ts) AS running, \
     lag(amount, 1) OVER (ORDER BY ts) AS prev FROM events \
     WHERE status IN ('ok', 'refund') QUALIFY running > 100 \
     ORDER BY user_id, ts DESC LIMIT 100",
    // Derived table + join + scalar subquery in WHERE.
    "SELECT t.user_id, u.name FROM (SELECT user_id, count(*) AS n FROM events \
     GROUP BY user_id) t JOIN users u ON t.user_id = u.user_id \
     WHERE t.n > (SELECT avg(amount) FROM events WHERE status = 'ok')",
    // CTE referencing a CTE, joined against a table.
    "WITH base AS (SELECT user_id, amount FROM events WHERE status = 'ok'), \
     agg AS (SELECT user_id, sum(amount) AS total FROM base GROUP BY user_id) \
     SELECT u.name, a.total FROM agg a JOIN users u ON u.user_id = a.user_id \
     ORDER BY a.total DESC NULLS LAST",
    // Comments interleaved + UNION ALL across a table and a view.
    "-- daily rollup\nSELECT ts, amount FROM events /* raw tier */ WHERE amount > 0 \
     UNION ALL\nSELECT NULL, total FROM spenders -- aggregated tier",
    // CASE-heavy projection with GROUP BY / HAVING over the alias.
    "SELECT CASE WHEN amount > 100 THEN 'big' WHEN amount > 10 THEN 'mid' \
     ELSE 'small' END AS bucket, count(*) AS n FROM events \
     GROUP BY bucket HAVING count(*) > 5",
    // Multi-statement with a dangling second statement mid-edit.
    "SELECT name FROM users WHERE user_id IN (SELECT user_id FROM spenders); \
     SELECT status, ",
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
    // Inside OVER(PARTITION BY |) — an expression operand: columns lead.
    let sql = "SELECT user_id, sum(amount) OVER (PARTITION BY ";
    let items = complete(sql, sql.len(), &catalog(), false);
    assert_eq!(items[0].kind, CompletionKind::Column, "{:?}", labels(&items));
    // After `QUALIFY running > 100 ` — a continuation: the ladder onward.
    let sql = "SELECT user_id FROM events QUALIFY user_id > 100 o";
    let items = complete(sql, sql.len(), &catalog(), false);
    pos(&items, "ORDER BY");
    absent(&items, "FROM"); // never backwards up the ladder
}

#[test]
fn torture_probes_cte_of_cte() {
    // Inside the second CTE, the first CTE is a FROM target…
    let sql = "WITH base AS (SELECT user_id FROM events), agg AS (SELECT user_id FROM ba";
    let items = complete(sql, sql.len(), &catalog(), false);
    pos(&items, "base");
    // …and outside, both CTEs resolve, including dot-columns.
    let sql = "WITH base AS (SELECT user_id, amount FROM events), \
               agg AS (SELECT user_id FROM base) SELECT agg.";
    let items = complete(sql, sql.len(), &catalog(), false);
    assert_eq!(labels(&items), vec!["user_id"]);
}

#[test]
fn torture_probes_subquery_positions() {
    // A scalar subquery in WHERE restarts nothing — its FROM completes tables.
    let sql = "SELECT name FROM users WHERE user_id > (SELECT avg(amount) FROM ev";
    let items = complete(sql, sql.len(), &catalog(), false);
    pos(&items, "events");
    // A derived-table alias resolves like an inline CTE — its scraped
    // projection dot-completes.
    let sql = "SELECT t. FROM (SELECT user_id FROM events) t";
    let items = complete(sql, "SELECT t.".len(), &catalog(), false);
    assert_eq!(labels(&items), vec!["user_id"]);
}

#[test]
fn torture_probes_union_and_comments() {
    // After UNION ALL, a fresh statement position: SELECT leads.
    let sql = "SELECT ts FROM events UNION ALL ";
    let items = complete(sql, sql.len(), &catalog(), false);
    assert_eq!(pos(&items, "SELECT"), 0, "{:?}", labels(&items));
    // A caret inside the trailing comment stays quiet even with code after
    // it on other lines.
    let sql = "-- roll|up\nSELECT ts FROM events";
    let caret = sql.find('|').unwrap();
    let clean = sql.replace('|', "");
    assert!(complete(&clean, caret, &catalog(), false).is_empty());
}

// ---- suppression (guard) ----

#[test]
fn no_completions_inside_strings_or_comments() {
    assert!(at("SELECT 'ab|c' FROM events").is_empty());
    assert!(at("SELECT * FROM events -- co|mment").is_empty());
    assert!(at("SELECT * /* |note */ FROM events").is_empty());
    assert!(at("SELECT 'ab|").is_empty()); // unterminated string
}

#[test]
fn completions_resume_after_a_closed_string() {
    let items = at("SELECT 'x', s| FROM events");
    pos(&items, "status");
}
