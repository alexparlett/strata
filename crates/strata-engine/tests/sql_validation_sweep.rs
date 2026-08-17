//! Validation sweep (P2-23 acceptance): the validator against a wide corpus of
//! query shapes over a realistic multi-table catalog.
//!
//! Four properties:
//! 1. **No false positives** — every valid query produces zero diagnostics, with a
//!    guard that each corpus entry genuinely plans (so the corpus can't rot).
//! 2. **Engine agreement on faults** — every bad-name query produces exactly the
//!    expected spans, and the real planner rejects it too (the resolver never
//!    invents an error the engine wouldn't).
//! 3. **Mid-edit tolerance** — half-written drafts stay quiet.
//! 4. **Prefix torture** — every prefix of every valid query validates without
//!    panicking, and every emitted span is a well-formed slice of the buffer.

use std::sync::Arc;

use datafusion::arrow::array::{
    ArrayRef, Date32Array, Float64Array, Int64Array, ListBuilder, StringArray, StringBuilder,
    StructArray,
};
use datafusion::arrow::datatypes::{DataType, Field, Fields, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::{SessionConfig, SessionContext};
use strata_engine::sql::{validate, FunctionCatalog};
use strata_engine::statements::pipeline::Pipeline;
use strata_engine::{Capability, CapabilityPolicyProvider};
use strata_model::Diagnostic;

/// A catalog shaped like a real project: plain tables, a keyword-named table,
/// a struct + list table, and a view.
async fn fixture() -> SessionContext {
    let mut config = SessionConfig::new().with_information_schema(true);
    config.options_mut().sql_parser.collect_spans = true;
    let ctx = SessionContext::new_with_config(config);

    let t = RecordBatch::try_new(
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
    ctx.register_batch("t", t).unwrap();

    let users = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("user_id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("email", DataType::Utf8, true),
            Field::new("created_at", DataType::Date32, true),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1_i64, 2])),
            Arc::new(StringArray::from(vec!["ann", "bob"])),
            Arc::new(StringArray::from(vec!["a@x.io", "b@x.io"])),
            Arc::new(Date32Array::from(vec![19000, 19100])),
        ],
    )
    .unwrap();
    ctx.register_batch("users", users).unwrap();

    let orders = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("order_id", DataType::Int64, false),
            Field::new("user_id", DataType::Int64, false),
            Field::new("amount", DataType::Float64, true),
            Field::new("status", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![10_i64, 11])),
            Arc::new(Int64Array::from(vec![1_i64, 2])),
            Arc::new(Float64Array::from(vec![9.5, 12.0])),
            Arc::new(StringArray::from(vec!["open", "paid"])),
        ],
    )
    .unwrap();
    ctx.register_batch("orders", orders).unwrap();

    let event = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("day", DataType::Int64, false),
            Field::new("kind", DataType::Utf8, true),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1_i64])),
            Arc::new(StringArray::from(vec!["click"])),
        ],
    )
    .unwrap();
    ctx.register_batch("event", event).unwrap();

    let address_fields = Fields::from(vec![
        Field::new("city", DataType::Utf8, true),
        Field::new("zip", DataType::Utf8, true),
    ]);
    let address = StructArray::from(vec![
        (
            Arc::new(Field::new("city", DataType::Utf8, true)),
            Arc::new(StringArray::from(vec!["berlin", "york"])) as ArrayRef,
        ),
        (
            Arc::new(Field::new("zip", DataType::Utf8, true)),
            Arc::new(StringArray::from(vec!["10115", "YO1"])) as ArrayRef,
        ),
    ]);
    let mut tags = ListBuilder::new(StringBuilder::new());
    tags.values().append_value("vip");
    tags.append(true);
    tags.values().append_value("new");
    tags.append(true);
    let tags = tags.finish();
    let customers = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("address", DataType::Struct(address_fields), true),
            Field::new(
                "tags",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1_i64, 2])),
            Arc::new(address) as ArrayRef,
            Arc::new(tags) as ArrayRef,
        ],
    )
    .unwrap();
    ctx.register_batch("customers", customers).unwrap();

    let df = ctx
        .sql("CREATE VIEW v_users AS SELECT user_id, name FROM users")
        .await
        .expect("create view");
    df.collect().await.expect("apply view");
    ctx
}

/// The registered-function names, as the engine snapshots them at startup —
/// validation in production never runs with an empty function catalog.
fn function_catalog(ctx: &SessionContext) -> FunctionCatalog {
    let state = ctx.state();
    FunctionCatalog {
        scalar: state
            .scalar_functions()
            .keys()
            .map(|n| n.as_str().into())
            .collect(),
        aggregate: state
            .aggregate_functions()
            .keys()
            .map(|n| n.as_str().into())
            .collect(),
        window: state
            .window_functions()
            .keys()
            .map(|n| n.as_str().into())
            .collect(),
    }
}

async fn run(ctx: &SessionContext, sql: &str) -> Vec<Diagnostic> {
    let policy = CapabilityPolicyProvider::new(Capability::full());
    validate(&Pipeline::new(ctx, &policy), &function_catalog(ctx), sql).await
}

/// Whether the engine itself accepts `sql` — the same parse→plan→analyze chain
/// the validator's dry-plan uses.
async fn engine_accepts(ctx: &SessionContext, sql: &str) -> Result<(), String> {
    let state = ctx.state();
    let plan = state
        .create_logical_plan(sql)
        .await
        .map_err(|e| e.to_string())?;
    state.optimize(&plan).map(|_| ()).map_err(|e| e.to_string())
}

fn messages(out: &[Diagnostic]) -> String {
    format!("{:?}", out.iter().map(|d| &d.message).collect::<Vec<_>>())
}

/// Every diagnostic span must be a well-formed slice of the buffer.
fn assert_spans_wellformed(sql: &str, out: &[Diagnostic]) {
    for d in out {
        if let Some(span) = &d.span {
            assert!(
                span.start < span.end
                    && span.end <= sql.len()
                    && sql.is_char_boundary(span.start)
                    && sql.is_char_boundary(span.end),
                "malformed span {span:?} in {sql:?} ({})",
                d.message
            );
        }
    }
}

const VALID: &[&str] = &[
    "SELECT id, name FROM t",
    "SELECT * FROM t",
    "SELECT t.id, t.name FROM t",
    "SELECT DISTINCT status FROM orders",
    "SELECT id AS the_id, name AS the_name FROM t ORDER BY the_id DESC",
    "SELECT id FROM t LIMIT 5 OFFSET 1",
    "SELECT \"name\" FROM t",
    "SELECT u.\"name\" FROM users u",
    "SELECT day, kind FROM event",
    "SELECT event.day FROM event",
    "SELECT id + 1, -id, id * 2 FROM t",
    "SELECT name || '!' FROM t",
    "SELECT CAST(id AS VARCHAR) AS c1, TRY_CAST(name AS INT) AS c2, id::text AS c3 FROM t",
    "SELECT CASE WHEN id > 1 THEN name ELSE 'x' END FROM t",
    "SELECT CASE status WHEN 'open' THEN 1 ELSE 0 END FROM orders",
    "SELECT id FROM t WHERE id BETWEEN 1 AND 5",
    "SELECT id FROM t WHERE name LIKE 'a%' OR name ILIKE '%B%'",
    "SELECT id FROM t WHERE name IS NULL OR name IS NOT NULL",
    "SELECT id FROM t WHERE name IS DISTINCT FROM 'a'",
    "SELECT id FROM t WHERE id IN (1, 2, 3) AND name NOT IN ('x')",
    "SELECT coalesce(name, 'anon'), nullif(name, ''), abs(id) FROM t",
    "SELECT round(amount, 1) FROM orders",
    "SELECT substr(name, 1, 2), upper(name), length(name) FROM t",
    "SELECT trim(BOTH ' ' FROM name) FROM t",
    "SELECT extract(YEAR FROM created_at) FROM users",
    "SELECT date_part('month', created_at) FROM users",
    "SELECT now(), current_date",
    "SELECT created_at + INTERVAL '1 day' FROM users",
    "SELECT count(*) FROM t",
    "SELECT count(DISTINCT status) FROM orders",
    "SELECT status, sum(amount), avg(amount), min(amount), max(amount) FROM orders GROUP BY status",
    "SELECT status FROM orders GROUP BY status HAVING count(*) > 0",
    "SELECT status, count(*) FROM orders GROUP BY 1 ORDER BY 2 DESC",
    "SELECT status, sum(amount) AS total FROM orders GROUP BY status ORDER BY total",
    "SELECT upper(status), count(*) FROM orders GROUP BY upper(status)",
    "SELECT status, user_id, sum(amount) FROM orders GROUP BY ROLLUP (status, user_id)",
    "SELECT count(*) FILTER (WHERE amount > 10) FROM orders",
    "SELECT id, row_number() OVER (ORDER BY id) FROM t",
    "SELECT user_id, sum(amount) OVER (PARTITION BY user_id ORDER BY order_id) FROM orders",
    "SELECT rank() OVER (PARTITION BY status ORDER BY amount DESC) FROM orders",
    "SELECT sum(amount) OVER (ORDER BY order_id ROWS BETWEEN 1 PRECEDING AND CURRENT ROW) FROM orders",
    "SELECT u.name, o.amount FROM users u JOIN orders o ON u.user_id = o.user_id",
    "SELECT u.name FROM users u LEFT JOIN orders o ON u.user_id = o.user_id AND o.amount > 0",
    "SELECT u.name FROM users u RIGHT OUTER JOIN orders o ON u.user_id = o.user_id",
    "SELECT u.name FROM users u FULL OUTER JOIN orders o ON u.user_id = o.user_id",
    "SELECT * FROM users CROSS JOIN t",
    "SELECT name FROM users JOIN orders USING (user_id)",
    "SELECT users.name FROM users NATURAL JOIN orders",
    "SELECT a.name, b.name FROM users a JOIN users b ON a.user_id = b.user_id",
    "SELECT u.name, o.amount, t.id FROM users u, orders o, t WHERE u.user_id = o.user_id AND t.id = u.user_id",
    "SELECT od.amount FROM orders od WHERE od.status = 'open'",
    "SELECT id FROM t WHERE id IN (SELECT user_id FROM orders)",
    "SELECT id FROM t WHERE id NOT IN (SELECT user_id FROM orders WHERE amount > 100)",
    "SELECT id FROM t WHERE EXISTS (SELECT 1 FROM users u WHERE u.user_id = t.id)",
    "SELECT id FROM t WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.user_id = t.id)",
    "SELECT (SELECT max(amount) FROM orders) AS top FROM t",
    "SELECT u.name, (SELECT max(o.amount) FROM orders o WHERE o.user_id = u.user_id) FROM users u",
    "SELECT id FROM (SELECT id FROM t) x",
    "SELECT x.id FROM (SELECT id, name FROM t WHERE id > 0) x WHERE x.id > 1",
    "SELECT * FROM (SELECT * FROM (SELECT id FROM t) a) b",
    "SELECT a + b FROM (VALUES (1, 2), (3, 4)) v(a, b)",
    "SELECT big.total FROM (SELECT user_id, sum(amount) AS total FROM orders GROUP BY user_id) big",
    "WITH c AS (SELECT id, name FROM t) SELECT c.id, c.name FROM c",
    "WITH c AS (SELECT id FROM t), d AS (SELECT id FROM c) SELECT id FROM d",
    "WITH c(x, y) AS (SELECT id, name FROM t) SELECT x, y FROM c",
    "WITH t AS (SELECT id FROM t) SELECT id FROM t",
    "WITH RECURSIVE r AS (SELECT 1 AS n UNION ALL SELECT n + 1 FROM r WHERE n < 3) SELECT n FROM r",
    "SELECT * FROM (WITH inner_c AS (SELECT id FROM t) SELECT id FROM inner_c) outer_q",
    "WITH c AS (SELECT user_id FROM orders) SELECT name FROM users WHERE user_id IN (SELECT user_id FROM c)",
    "SELECT id FROM t UNION SELECT user_id FROM users",
    "SELECT id FROM t UNION ALL SELECT user_id FROM users ORDER BY id LIMIT 3",
    "SELECT id FROM t EXCEPT SELECT user_id FROM users",
    "SELECT id FROM t INTERSECT SELECT user_id FROM users",
    "SELECT id FROM t UNION ALL (SELECT user_id FROM users UNION ALL SELECT order_id FROM orders)",
    "SELECT user_id, name FROM v_users",
    "SELECT v.name FROM v_users v JOIN orders o ON v.user_id = o.user_id",
    "SELECT address['city'] FROM customers",
    "SELECT tags[1] FROM customers",
    "SELECT c.id FROM customers c WHERE c.address['zip'] = '10115'",
    "EXPLAIN SELECT id FROM t",
    "EXPLAIN SELECT u.name FROM users u JOIN orders o ON u.user_id = o.user_id",
    "DESCRIBE t",
    "SHOW TABLES",
    "SELECT id -- trailing note\nFROM t\nWHERE id > 0 /* block */ ORDER BY id",
    "SELECT ';' FROM t",
];

#[tokio::test]
async fn valid_queries_stay_clean() {
    let ctx = fixture().await;
    for sql in VALID {
        if let Err(e) = engine_accepts(&ctx, sql).await {
            panic!("corpus entry does not plan — fix the corpus: {sql:?}: {e}");
        }
        let out = run(&ctx, sql).await;
        assert!(
            out.is_empty(),
            "false positive on {sql:?}: {}",
            messages(&out)
        );
    }
}

/// `(sql, expected spanned texts, in order)` — the diagnostic count must match
/// exactly (no extra noise) and each span slices to the expected text.
const BAD: &[(&str, &[&str])] = &[
    ("SELECT nme, product_idd FROM t", &["nme", "product_idd"]),
    ("SELECT missing FROM nope", &["nope"]),
    (
        "SELECT missing FROM nope, also_nope",
        &["nope", "also_nope"],
    ),
    ("SELECT u.nme FROM users u", &["u.nme"]),
    ("SELECT users.nme FROM users", &["users.nme"]),
    ("SELECT x.user_id FROM users u", &["x.user_id"]),
    ("SELECT id FROM t WHERE nmae = 'x'", &["nmae"]),
    (
        "SELECT id FROM t WHERE id = 1 AND wrong > 2 OR also_wrong < 3",
        &["wrong", "also_wrong"],
    ),
    (
        "WITH c AS (SELECT user_id FROM users) SELECT nme FROM c",
        &["nme"],
    ),
    ("WITH c AS (SELECT bogus FROM users) SELECT 1", &["bogus"]),
    ("WITH c(x) AS (SELECT id FROM t) SELECT y FROM c", &["y"]),
    (
        "SELECT d.wrong FROM (SELECT user_id FROM users) d",
        &["d.wrong"],
    ),
    ("SELECT wrong FROM (VALUES (1, 2)) v(a, b)", &["wrong"]),
    ("SELECT wrong FROM (SELECT * FROM users) u", &["wrong"]),
    ("SELECT a FROM t UNION ALL SELECT b FROM users", &["a", "b"]),
    ("SELECT id FROM t ORDER BY wrongcol", &["wrongcol"]),
    ("SELECT id FROM t GROUP BY wrongcol", &["wrongcol"]),
    (
        "SELECT id, count(wrongcol) FROM t GROUP BY id",
        &["wrongcol"],
    ),
    (
        "SELECT id FROM t JOIN users ON t.id = users.wrong",
        &["users.wrong"],
    ),
    (
        "SELECT id FROM t WHERE id IN (SELECT wrong FROM users)",
        &["wrong"],
    ),
    (
        "SELECT id FROM t WHERE EXISTS (SELECT 1 FROM users u WHERE u.wrong = t.id)",
        &["u.wrong"],
    ),
    (
        "SELECT t2.wrong FROM t JOIN t AS t2 ON t.id = t2.id",
        &["t2.wrong"],
    ),
    ("SELECT wrong FROM v_users", &["wrong"]),
    ("EXPLAIN SELECT wrong FROM t", &["wrong"]),
    ("SELECT nme FROM t; SELECT idd FROM users", &["nme", "idd"]),
    (
        "SELECT sum(amount) OVER (PARTITION BY wrong ORDER BY order_id) FROM orders",
        &["wrong"],
    ),
    (
        "SELECT row_number() OVER (ORDER BY wrong) FROM t",
        &["wrong"],
    ),
    (
        "SELECT id FROM t WHERE t.id = users.user_id",
        &["users.user_id"],
    ),
    ("SELECT c.wrong['zip'] FROM customers c", &["c.wrong"]),
    ("SELECT wrong['zip'] FROM customers", &["wrong"]),
];

#[tokio::test]
async fn bad_names_all_reported_and_engine_agrees() {
    let ctx = fixture().await;
    for (sql, expected) in BAD {
        let out = run(&ctx, sql).await;
        assert_spans_wellformed(sql, &out);
        let spans: Vec<&str> = out
            .iter()
            .filter_map(|d| d.span.clone().map(|s| &sql[s]))
            .collect();
        assert_eq!(
            &spans,
            expected,
            "wrong faults for {sql:?}: {} (expected {expected:?})",
            messages(&out)
        );
        assert!(
            out.iter().all(Diagnostic::is_error),
            "non-error fault in {sql:?}"
        );
        assert!(
            engine_accepts(&ctx, sql).await.is_err(),
            "resolver flagged {sql:?} but the engine accepts it"
        );
    }
}

#[tokio::test]
async fn exact_case_quoted_misses_stay_engine_authoritative() {
    let ctx = fixture().await;
    let out = run(&ctx, "SELECT \"Name\" FROM t").await;
    assert!(!out.is_empty(), "quoted case miss must error");
    assert!(out.iter().all(Diagnostic::is_error));
}

const DRAFTS: &[&str] = &[
    "SELECT",
    "SELECT id,",
    "SELECT nmae, something",
    "SELECT id FROM",
    "SELECT id FROM t WHERE",
    "SELECT id FROM t WHERE id =",
    "SELECT id FROM t WHERE id BETWEEN",
    "SELECT id FROM t GROUP BY",
    "SELECT id FROM t ORDER BY",
    "SELECT id FROM t LIMIT",
    "SELECT * FROM t JOIN",
    "SELECT * FROM t LEFT JOIN",
    "SELECT * FROM t JOIN users ON",
    "SELECT * FROM t JOIN users ON t.id =",
    "WITH x AS (SELECT id FROM t)",
    "WITH x AS (SELECT id FROM t),",
    "WITH x AS (SELECT id FROM t) SELECT",
    "SELECT id FROM t UNION",
    "SELECT id FROM t UNION ALL SELECT",
    "SELECT draft_one, draft_two",
];

#[tokio::test]
async fn mid_edit_drafts_stay_quiet() {
    let ctx = fixture().await;
    for sql in DRAFTS {
        let out = run(&ctx, sql).await;
        assert!(
            out.is_empty(),
            "premature diagnostics on draft {sql:?}: {}",
            messages(&out)
        );
    }
}

#[tokio::test]
async fn every_prefix_of_every_valid_query_validates() {
    let ctx = fixture().await;
    for sql in VALID {
        for (i, _) in sql.char_indices().skip(1) {
            let prefix = &sql[..i];
            let out = run(&ctx, prefix).await;
            assert_spans_wellformed(prefix, &out);
        }
    }
}

#[tokio::test]
async fn non_ascii_text_never_panics() {
    let ctx = fixture().await;
    let queries = [
        "SELECT 'héllo wörld' FROM t",
        "SELECT name FROM t WHERE name = 'ünïcode'",
        "SELECT 'データ', nme FROM t",
        "SELECT '🦀' FROM t WHERE wrong = 1",
    ];
    for sql in queries {
        for (i, _) in sql.char_indices().skip(1) {
            let _ = run(&ctx, &sql[..i]).await;
        }
        let _ = run(&ctx, sql).await;
    }
}
