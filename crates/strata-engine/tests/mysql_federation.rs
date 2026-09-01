//! **A source, against a real `MySQL`** (EA-20) — the registry's second conformance run.
//!
//! `PostgreSQL` proved the `DataSource` seam once. One backend proves a trait can be *written*; it
//! cannot prove the trait is in the right place, because every accommodation the one implementation
//! needed is indistinguishable from a general one. This is the second run of the same phases
//! against a server that differs in the ways a backend really differs — a two-level namespace
//! rather than three, backticks rather than double quotes, a JSON path string rather than a chain
//! of operators, and errno codes rather than `SQLSTATE`s — and everything it drives past the
//! fixture is the **generic** path: connect, enumerate, resolve, federate, qualify, profile,
//! refuse. Point the fixture at your own server and change the kind the def names, and these are
//! the phases your source is held to.
//!
//! **A real server rather than a mock**, the same argument as `postgres_federation.rs`: the
//! unparser's output is judged by the *server*, and a stand-in accepting whatever we sent would
//! assert nothing. That argument is at its sharpest for the JSON family, whose whole content is a
//! claim about what two expressions mean on two sides — so [`json_pushdown`] asks the **same
//! question of the same documents** locally and remotely and asserts the answers are equal, rather
//! than asserting a spelling.
//!
//! The fixture is seeded by the image's own entrypoint (`with_init_sql` runs it through the `mysql`
//! client inside the container), which is a lower layer than anything under test and needs no
//! second driver in the dependency tree.
//!
//! **It drives `Sources::connect`, the real entry point, password and all**: `keyring_core::mock`
//! is this binary's store, so no real Keychain is touched.
//!
//! **Deliberately not `#[ignore]`d**, for the reason the other two are not. One test, sequential
//! phases, container held for the duration: a second `#[tokio::test]` would race this one for the
//! single cloud worker.
//!
//! **Both sides of the write gate are driven.** The fixture connects `read_only: true`, which is
//! the shipped default, so every phase before [`remote_writes`] runs through exactly the data
//! source a user gets and [`statement_policy`] asserts that each write is refused by name. The
//! write phases then re-connect with the toggle off — which is all the data source editor's Save
//! does — and drive the two statements DataFusion can plan and the ones only the server can run.
//! The refusals are the **def's** `read_only`, never a kind-wide restriction: `SourceKind::WRITABLE`
//! is `true` for `MySQL`, which is what offers that toggle at all.
//!
//! Gated on the `mysql` feature, because the source it drives rides that feature: an engine built
//! without it has no `MySQL` in its tree, and neither does this.
#![cfg(feature = "mysql")]

use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::Path;
use std::sync::Once;
use std::time::{Duration, Instant};
use std::{env, fs, process};

use datafusion::arrow::datatypes::{DataType, Field};
use keyring_core::mock;
use strata_arrow::column_info;
use strata_arrow::profile::Profiled;

use strata_engine::profile;
use strata_engine::sources::mysql::settings::PASSWORD as PASSWORD_KEY;
use strata_engine::sources::{
    put_secret, SchemaListingView, SchemaVisibility, SourceDetail, SourceListing,
};
use strata_engine::{sql, Engine, RunOutcome, RunTag, SourceDefs, StoreEffect, WsId};
use strata_model::{
    Cell, CsvRead, SecretRef, Secrets, SourceDef, SourceFormat, StatKey, TableDef, TableOrigin,
};
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::mysql::Mysql;

/// The account the test logs in as — created by [`SEED`], not the image's own `root`, because a
/// grant-scoped account is what makes the enumeration's privilege filter observable: it can read
/// two of the server's three databases and write to one of them.
const USER: &str = "app";
const PASSWORD: &str = "sekret";
/// How queries address the data source — the catalog half of `catalog.schema.table`.
const CATALOG: &str = "my";

/// The fixture, run by the image's entrypoint before it accepts connections.
///
/// **Three databases, and the account can read two of them.** `hidden` is what makes the
/// enumeration's privilege filter a fact rather than a claim: `information_schema.tables` shows an
/// account only what it holds a privilege on, so a listing that includes `hidden` would mean the
/// query is asking the wrong question.
///
/// The grants are the fixture too: `shop` is writable and `analytics` is not, so a write the
/// server refuses has somewhere to be refused.
///
/// `shop.notes` carries a plain `TEXT` column, which is what shows the driver's text-result gap is
/// not about JSON at all.
///
/// `shop` and `analytics` both hold an `orders`, which is the ambiguity [`unqualified_names`]
/// needs; `shop.big_orders` is a **view**, so the tree's Tables / Views split has something to be
/// about; `shop.events` carries a `JSON` column whose documents [`json_pushdown`] asks both sides
/// about — including a key whose value is `null` and a row whose document is `NULL`, the two
/// places `MySQL`'s answer differs from a bare `->>`.
const SEED: &str = "\
CREATE DATABASE shop;
CREATE DATABASE analytics;
CREATE DATABASE hidden;
CREATE TABLE shop.orders (id INT PRIMARY KEY, customer INT, total INT, tags JSON);
INSERT INTO shop.orders VALUES
  (1, 10, 99, '{\"channel\":\"web\"}'),
  (2, 10, 10, '{\"channel\":\"store\"}'),
  (3, 20, 42, '{\"channel\":\"web\"}');
CREATE TABLE shop.customers (id INT PRIMARY KEY, name VARCHAR(32));
INSERT INTO shop.customers VALUES (10, 'acme'), (20, 'globex');
CREATE VIEW shop.big_orders AS SELECT id, total FROM shop.orders WHERE total > 50;
CREATE TABLE shop.events (id INT PRIMARY KEY, payload JSON);
INSERT INTO shop.events VALUES
  (1, '{\"type\":\"click\",\"source\":{\"campaign\":\"spring\"},\"z\":null}'),
  (2, '{\"type\":\"view\",\"source\":{\"campaign\":\"winter\"}}'),
  (3, '{\"type\":\"click\",\"bot\":true,\"z\":\"null\"}'),
  (4, NULL);
CREATE TABLE shop.notes (id INT PRIMARY KEY, body TEXT);
INSERT INTO shop.notes VALUES (1, 'shipped late');
CREATE TABLE analytics.sessions (id INT PRIMARY KEY, minutes INT);
INSERT INTO analytics.sessions VALUES (1, 5), (2, 9);
CREATE TABLE analytics.orders (id INT PRIMARY KEY);
CREATE TABLE hidden.secret (id INT PRIMARY KEY);
CREATE USER 'app'@'%' IDENTIFIED BY 'sekret';
GRANT SELECT, INSERT, UPDATE, DELETE, CREATE, DROP, CREATE VIEW ON shop.* TO 'app'@'%';
GRANT SELECT ON analytics.* TO 'app'@'%';
FLUSH PRIVILEGES;
";

/// The same documents as `shop.events`, as a local file — so [`json_pushdown`] can ask one
/// question of two sides.
///
/// Row 4's payload is empty, which a CSV read makes NULL: the document that is `NULL` rather than
/// the key whose value is.
const LOCAL_EVENTS: &str = "id,payload\n\
1,\"{\"\"type\"\":\"\"click\"\",\"\"source\"\":{\"\"campaign\"\":\"\"spring\"\"},\"\"z\"\":null}\"\n\
2,\"{\"\"type\"\":\"\"view\"\",\"\"source\"\":{\"\"campaign\"\":\"\"winter\"\"}}\"\n\
3,\"{\"\"type\"\":\"\"click\"\",\"\"bot\"\":true,\"\"z\"\":\"\"null\"\"}\"\n\
4,\n";

/// See `object_store_minio.rs` — same budget, same reason (a hosted runtime hands this account
/// one worker at a time, and an overlap is a handover rather than a queue).
const CAPACITY_RETRY_BUDGET: Duration = Duration::from_secs(90);
const CAPACITY_RETRY_GAP: Duration = Duration::from_secs(10);

/// How long the seed is given to finish after the image reports itself ready — see [`ready`].
const SEED_BUDGET: Duration = Duration::from_secs(120);
const SEED_GAP: Duration = Duration::from_secs(2);

/// Is this failure the runtime saying **busy**, rather than saying no? The MinIO test's own
/// predicate, for its own reason — two spellings, one fault, and everything else falls through
/// to the panic, because "no runtime" must never look like "the code is fine".
fn at_capacity(err: &impl Display) -> bool {
    let msg = err.to_string();
    msg.contains("too many concurrent requests") || msg.contains("IncompleteMessage")
}

/// A running `MySQL` seeded with [`SEED`], and the port it answers on. The container must be held
/// for the test's duration — dropping it stops the server.
async fn mysql() -> (ContainerAsync<Mysql>, u16) {
    let deadline = Instant::now() + CAPACITY_RETRY_BUDGET;
    let container = loop {
        let image = Mysql::default().with_init_sql(SEED.to_string().into_bytes());
        match image.start().await {
            Ok(container) => break container,
            Err(err) if at_capacity(&err) && Instant::now() < deadline => {
                eprintln!("container runtime is at capacity, retrying: {err}");
                tokio::time::sleep(CAPACITY_RETRY_GAP).await;
            }
            Err(err) => panic!("MySQL starts (is a Docker runtime available?): {err}"),
        }
    };
    let port = container
        .get_host_port_ipv4(3306)
        .await
        .expect("MySQL's port");
    (container, port)
}

/// Wait until the seed has run, by asking for the thing the seed creates last.
///
/// The image reports "ready for connections" from a server that has already run the init scripts,
/// but it prints the same line from the temporary server it runs *during* them; a connect that
/// lands between the two finds no `reader` account and no `shop`. So readiness is the fixture's
/// own answer rather than a log line, and this fails loud on the budget rather than letting the
/// phases below explain it as a bug in the code they test.
async fn ready(engine: &Engine, port: u16) {
    let conn = source(port, "preflight", &["shop"]);
    store_password(&conn, PASSWORD);
    let deadline = Instant::now() + SEED_BUDGET;
    loop {
        match engine.sources().connect(conn.clone()).await {
            Ok(())
                if schemas_of(engine, "preflight")
                    .iter()
                    .any(|s| s.name == "shop") =>
            {
                break
            }
            other => {
                assert!(
                    Instant::now() < deadline,
                    "the seed never finished: {other:?}"
                );
                tokio::time::sleep(SEED_GAP).await;
            }
        }
    }
    let _ = engine.sources().disconnect(&conn.named());
}

/// The data source under test. `ssl=disabled` because the container's certificate is
/// self-generated; the two encrypting modes are the driver's and would need a certificate this
/// fixture has no way to produce.
///
/// **Read-only**, which is the shipped default — [`writable`] is what the write phases connect
/// with, so every phase before them is driven through exactly the data source a user gets.
fn source(port: u16, catalog: &str, schemas: &[&str]) -> SourceDef {
    SourceDef {
        kind: "mysql".into(),
        name: catalog.into(),
        config: BTreeMap::from([
            ("address".to_string(), format!("127.0.0.1:{port}")),
            ("user".to_string(), USER.to_string()),
            ("ssl".to_string(), "disabled".to_string()),
        ]),
        secrets: Secrets::Filed(BTreeMap::from([(
            PASSWORD_KEY.to_string(),
            SecretRef::derived("test-password", catalog),
        )])),
        schemas: schemas.iter().map(|s| (*s).to_string()).collect(),
        read_only: true,
    }
}

/// The same data source opted in to writes — the one setting that separates them, so a re-connect
/// with this is exactly the user turning the toggle off.
fn writable(port: u16, catalog: &str, schemas: &[&str]) -> SourceDef {
    let mut def = source(port, catalog, schemas);
    def.read_only = false;
    def
}

/// **The keystore this binary uses**, installed once: an in-memory store, so the bridge under
/// test is real and the platform Keychain is untouched.
fn mocked_keystore() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        keyring_core::set_default_store(mock::Store::new().expect("a mock keystore"));
    });
}

/// File `value` under the slot `conn` records, exactly as the data source editor will.
fn store_password(conn: &SourceDef, value: &str) {
    put_secret(conn, PASSWORD_KEY, value).expect("this machine's keystore answers");
}

/// Run `sql` and hand back its first page as text, row by row.
async fn rows(engine: &Engine, tag: u128, sql: &str) -> Vec<Vec<String>> {
    engine
        .ws(WsId(1))
        .query(RunTag(tag), sql.to_string(), 200)
        .await
        .unwrap_or_else(|e| panic!("run '{sql}': {e}"))
        .output
        .rows
        .iter()
        .map(|row| row.iter().map(|cell: &Cell| cell.text.clone()).collect())
        .collect()
}

/// Both plan texts, for the pushdown assertions — the same unclipped read the plan view makes.
async fn explain(engine: &Engine, tag: u128, sql: &str) -> String {
    let plan = engine
        .ws(WsId(1))
        .explain(RunTag(tag), sql.to_string())
        .await
        .unwrap_or_else(|e| panic!("explain '{sql}': {e}"));
    format!("{}\n{}", plan.logical_text, plan.physical_text)
}

#[tokio::test]
async fn a_mysql_source_registers_a_federated_catalog() {
    mocked_keystore();
    let (_my, port) = mysql().await;

    let engine = Engine::builder().build();
    ready(&engine, port).await;

    let conn = source(port, CATALOG, &["shop"]);
    store_password(&conn, PASSWORD);

    let missing = source(port, "no_password", &["shop"]);
    store_password(&missing, "");
    let why = engine
        .sources()
        .connect(missing.clone())
        .await
        .expect_err("no password is stored for it")
        .to_string();
    assert!(
        why.contains("No password is stored on this machine") && why.contains(&missing.named()),
        "the refusal names the machine and the data source: {why}"
    );

    let closed = {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = taken.local_addr().expect("an address").port();
        drop(taken);
        port
    };
    let elsewhere = source(closed, "elsewhere", &["shop"]);
    store_password(&elsewhere, PASSWORD);
    let why = engine
        .sources()
        .connect(elsewhere)
        .await
        .expect_err("nothing is listening there")
        .to_string();
    assert!(
        why.contains(&format!("127.0.0.1:{closed}")),
        "the refusal names the address to fix: {why}"
    );

    let wrong_password = source(port, "wrong_password", &["shop"]);
    store_password(&wrong_password, "not-it");
    let why = engine
        .sources()
        .connect(wrong_password)
        .await
        .expect_err("the password is wrong")
        .to_string();
    assert!(why.contains(USER), "the refusal names the user: {why}");

    let mut with_database = source(port, "with_database", &["shop"]);
    with_database
        .config
        .insert("address".to_string(), format!("127.0.0.1:{port}/shop"));
    store_password(&with_database, PASSWORD);
    let why = engine
        .sources()
        .connect(with_database)
        .await
        .expect_err("a MySQL data source is a whole server")
        .to_string();
    assert!(
        why.contains("source.database.table"),
        "…and the refusal says where the database went instead: {why}"
    );

    let reserved = source(port, "strata", &["shop"]);
    store_password(&reserved, PASSWORD);
    let why = engine
        .sources()
        .connect(reserved)
        .await
        .expect_err("'strata' is the workspace's own catalog")
        .to_string();
    assert!(why.contains("strata"), "{why}");

    assert!(
        !live(&engine, &conn),
        "a refused data source registers nothing"
    );

    engine
        .sources()
        .connect(conn.clone())
        .await
        .expect("the data source registers its catalog");

    enumeration(&engine).await;
    qualified_offer(&engine).await;
    pushdown(&engine).await;
    profiling(&engine).await;
    let fixtures = env::temp_dir().join(format!("strata-my-{}", process::id()));
    mixed_plan(&engine, &fixtures).await;
    json_pushdown(&engine, &fixtures).await;
    statement_policy(&engine, &fixtures).await;
    unqualified_names(&engine, port).await;
    remote_writes(&engine, port).await;
    remote_statements(&engine, port).await;
    reconnect_and_disconnect(&engine, port).await;

    let _ = fs::remove_dir_all(&fixtures);
}

/// **What the catalog says it holds** — a *phase*, called in sequence, not a test of its own.
///
/// The claim that only a server can settle: a `MySQL` **database** arrives as a schema, so
/// `information_schema.tables` on the Strata side answers about `my.shop.orders` — three parts over
/// a server that has two — and the account's grants are what bound the listing.
async fn enumeration(engine: &Engine) {
    let names = rows(
        engine,
        1,
        &format!(
            "SELECT table_schema, table_name FROM information_schema.tables \
             WHERE table_catalog = '{CATALOG}' AND table_schema <> 'information_schema' \
             ORDER BY 1, 2"
        ),
    )
    .await;
    assert_eq!(
        names,
        vec![
            vec!["analytics".to_string(), "orders".to_string()],
            vec!["analytics".to_string(), "sessions".to_string()],
            vec!["shop".to_string(), "big_orders".to_string()],
            vec!["shop".to_string(), "customers".to_string()],
            vec!["shop".to_string(), "events".to_string()],
            vec!["shop".to_string(), "notes".to_string()],
            vec!["shop".to_string(), "orders".to_string()],
        ],
        "every database the account was granted, and every relation in them — and 'hidden' is \
         not among them, which is the server's own privilege filter rather than a predicate of \
         ours"
    );

    let _ = engine
        .sources()
        .show_schemas(CATALOG, &["shop".to_string(), "warehouse".to_string()]);
    let listing = schemas_of(engine, CATALOG);
    assert_eq!(
        listing
            .iter()
            .map(|schema| (schema.name.as_str(), schema.visibility))
            .collect::<Vec<_>>(),
        vec![
            ("analytics", SchemaVisibility::NotEnabled),
            ("shop", SchemaVisibility::Live),
            ("warehouse", SchemaVisibility::EnabledButMissing),
        ]
    );
    assert_eq!(
        listing
            .iter()
            .find(|schema| schema.name == "shop")
            .map(|schema| schema
                .relations
                .iter()
                .map(|r| (r.name.as_str(), r.view))
                .collect::<Vec<_>>()),
        Some(vec![
            ("big_orders", true),
            ("customers", false),
            ("events", false),
            ("notes", false),
            ("orders", false)
        ]),
        "a remote view is listed as one, off information_schema's own TABLE_TYPE"
    );
    let _ = engine
        .sources()
        .show_schemas(CATALOG, &["shop".to_string()]);

    assert_eq!(
        rows(
            engine,
            2,
            &format!("SELECT minutes FROM {CATALOG}.analytics.sessions ORDER BY 1")
        )
        .await,
        vec![vec!["5".to_string()], vec!["9".to_string()]],
        "a database the def does not show still resolves — visibility, not policy"
    );
}

/// Is `conn` a data source this engine holds live right now? — the snapshot's own answer.
fn live(engine: &Engine, conn: &SourceDef) -> bool {
    engine
        .sources()
        .listing()
        .source(&conn.named())
        .is_some_and(SourceListing::live)
}

/// What the data source called `name` shows, scoped and tagged — the one read every surface makes.
fn schemas_of(engine: &Engine, name: &str) -> Vec<SchemaListingView> {
    match engine
        .sources()
        .listing()
        .source(name)
        .map(|source| source.detail.clone())
    {
        Some(SourceDetail::Catalog { schemas, .. }) => schemas,
        other => panic!("'{name}' is not a live database: {other:?}"),
    }
}

/// **What completion offers for this data source, and that the name it hands over runs** — a phase
/// of the test above.
///
/// The offer is unit-tested against a hand-built listing next door; what only a server can settle
/// is that the two halves agree — that the names `Sources::database_syms` carries are the names the
/// catalog actually resolves.
async fn qualified_offer(engine: &Engine) {
    let catalog = sql::Symbols::build([], [], engine.lang().bundle(), String::new());
    let offer = |sql: &str| {
        let mut labels = sql::complete(&catalog, sql, sql.len(), false)
            .into_iter()
            .map(|c| c.label)
            .collect::<Vec<_>>();
        labels.sort();
        labels
    };

    assert_eq!(
        offer(&format!("SELECT * FROM {CATALOG}.")),
        vec!["shop".to_string()],
        "the enabled databases, and only those"
    );
    assert_eq!(
        offer(&format!("SELECT * FROM {CATALOG}.shop.")),
        vec![
            "big_orders".to_string(),
            "customers".to_string(),
            "events".to_string(),
            "notes".to_string(),
            "orders".to_string()
        ],
        "every relation the listing holds, remote view included"
    );

    let address = sql::SessionName::qualified([CATALOG, "shop", "orders"]).to_string();
    assert_eq!(
        rows(
            engine,
            40,
            &format!("SELECT id, customer, total FROM {address} ORDER BY id LIMIT 1")
        )
        .await,
        vec![vec!["1".to_string(), "10".to_string(), "99".to_string()]],
        "the address the tree's gestures compose is one the engine runs"
    );

    assert!(
        !offer("INSERT INTO ").contains(&"orders".to_string()),
        "a read-only connection is offered at no write position: {:?}",
        offer("INSERT INTO ")
    );
}

/// **Reads, and what actually reaches the server** — a phase of the test above.
async fn pushdown(engine: &Engine) {
    assert_eq!(
        rows(
            engine,
            3,
            &format!("SELECT id FROM {CATALOG}.shop.orders ORDER BY id")
        )
        .await,
        vec![
            vec!["1".to_string()],
            vec!["2".to_string()],
            vec!["3".to_string()]
        ]
    );

    let filtered = explain(
        engine,
        4,
        &format!("SELECT id FROM {CATALOG}.shop.orders WHERE customer = 20"),
    )
    .await;
    assert!(
        filtered.contains("base_sql=")
            && filtered.contains("WHERE")
            && filtered.contains("customer"),
        "the filter did not reach the remote statement:\n{filtered}"
    );
    assert!(
        filtered.contains("`shop`.`orders`"),
        "…and the relation is spelled the server's way:\n{filtered}"
    );

    let joined = explain(
        engine,
        5,
        &format!(
            "SELECT c.name, sum(o.total) FROM {CATALOG}.shop.orders o \
             JOIN {CATALOG}.shop.customers c ON c.id = o.customer GROUP BY c.name"
        ),
    )
    .await;
    let federated: Vec<&str> = joined
        .lines()
        .filter(|line| line.contains("VirtualExecutionPlan"))
        .collect();
    assert_eq!(
        federated.len(),
        1,
        "the join did not federate into one remote node:\n{joined}"
    );
    assert!(
        federated[0].to_uppercase().contains(" JOIN "),
        "the remote statement does not carry the join:\n{}",
        federated[0]
    );
    assert_eq!(
        rows(
            engine,
            6,
            &format!(
                "SELECT c.name, sum(o.total) FROM {CATALOG}.shop.orders o \
                 JOIN {CATALOG}.shop.customers c ON c.id = o.customer \
                 GROUP BY c.name ORDER BY 1"
            )
        )
        .await,
        vec![
            vec!["acme".to_string(), "109".to_string()],
            vec!["globex".to_string(), "42".to_string()]
        ],
        "and it answers correctly"
    );

    across_databases(engine).await;
}

/// **A join across two databases is one statement**, which is the difference a MySQL source's
/// shape actually makes — and the one thing about it worth measuring rather than assuming.
///
/// A MySQL database sits at the same level as a `PostgreSQL` schema and behaves like one
/// everywhere Strata looks: the enumeration files it under `Listing`'s namespace key, `schemas`
/// shows or hides it, a bare name searches the shown ones, and a query addresses it as the middle
/// part of `source.database.table`. What is **not** the same is what a source spans. A PG source is
/// one database, so its schemas are inside it and a query across two *databases* needs two
/// sources — two pools, two compute contexts, and therefore two remote statements with the join
/// run locally. A MySQL source is a whole server, so two of its "schemas" are two databases on one
/// connection: they share a compute context, and the join federates whole.
async fn across_databases(engine: &Engine) {
    let sql = format!(
        "SELECT o.id, s.minutes FROM {CATALOG}.shop.orders o \
         JOIN {CATALOG}.analytics.sessions s ON s.id = o.id ORDER BY 1"
    );
    let plan = explain(engine, 9, &sql).await;
    let federated: Vec<&str> = plan
        .lines()
        .filter(|line| line.contains("VirtualExecutionPlan"))
        .collect();
    assert_eq!(
        federated.len(),
        1,
        "two databases on one server are one connection, so the join federates whole:\n{plan}"
    );
    assert!(
        federated[0].contains("`shop`.`orders`") && federated[0].contains("`analytics`.`sessions`"),
        "…and the statement names both in full, because the server has no default database:\n{}",
        federated[0]
    );
    assert_eq!(
        rows(engine, 10, &sql).await,
        vec![
            vec!["1".to_string(), "5".to_string()],
            vec!["2".to_string(), "9".to_string()]
        ],
        "and it answers"
    );
}

/// **Profiling a remote relation** — a phase of the test above, and the second backend the same
/// expression set has to survive.
///
/// **Built from the set rather than typed out**, the discipline `postgres_federation.rs` states:
/// an aggregate added to `Profiled::wanted`'s Database arm that renders and then fails on a second
/// server would leave every assertion below green.
async fn profiling(engine: &Engine) {
    let name = format!("{CATALOG}.shop.orders");
    let columns = [
        column_info(&Field::new("total", DataType::Int32, true)),
        column_info(&Field::new("tags", DataType::Utf8, true)),
    ];
    let statement = profile::statement(&name, &columns, Profiled::Database);
    assert!(
        !statement.is_empty(),
        "every expression in the remote set has to unparse before it can federate"
    );

    let plan = explain(engine, 50, statement.trim_end_matches(';')).await;
    assert_eq!(
        plan.lines()
            .filter(|line| line.contains("VirtualExecutionPlan"))
            .count(),
        1,
        "the profile's aggregate did not federate into one remote node:\n{plan}"
    );

    let profile = engine
        .catalog()
        .profile(name.clone())
        .await
        .expect("the remote profile runs on the server");

    assert_eq!(profile.rows, 3);
    let total = profile.cols.get("total").expect("the numeric column");
    let fact = |key: StatKey| {
        total
            .iter()
            .find(|s| s.key == key)
            .map(|s| s.text.as_str())
            .unwrap_or_else(|| panic!("no {key:?} in {total:?}"))
    };
    assert_eq!(fact(StatKey::Nulls), "0");
    assert_eq!(fact(StatKey::Distinct), "3");
    assert_eq!(fact(StatKey::Min), "10");
    assert_eq!(fact(StatKey::Max), "99");
    let mean: f64 = fact(StatKey::Mean).parse().expect("a numeric mean");
    assert!(
        (mean - (99.0 + 10.0 + 42.0) / 3.0).abs() < 1e-9,
        "avg over an integer column comes back as a number: {mean}"
    );
    assert!(
        !total.iter().any(|s| s.key == StatKey::Median),
        "the median is absent **by design** — the remote set never carries it: {total:?}"
    );
}

/// **The mixed plan** — a **file** table joined onto a remote one; a phase of the test above.
///
/// A real local table, not a `VALUES` list: a `Values` node carries no table provider, so
/// federation finds nothing to disagree with and sweeps the whole join into the remote statement.
async fn mixed_plan(engine: &Engine, dir: &Path) {
    fs::create_dir_all(dir).expect("a fixture folder");
    fs::write(dir.join("tiers.csv"), "customer,tier\n10,gold\n20,silver\n").expect("the fixture");
    register_csv(engine, dir, "tiers", "tiers.csv").await;

    let mixed_sql = format!(
        "SELECT t.tier, count(*) FROM {CATALOG}.shop.orders o \
         JOIN tiers t ON t.customer = o.customer GROUP BY t.tier ORDER BY 1"
    );
    let mixed = explain(engine, 7, &mixed_sql).await;
    assert_eq!(
        mixed
            .lines()
            .filter(|line| line.contains("VirtualExecutionPlan"))
            .count(),
        1,
        "the remote side of a mixed join still federates:\n{mixed}"
    );
    assert!(
        mixed.contains("HashJoinExec") || mixed.contains("NestedLoopJoinExec"),
        "…and the join itself runs locally:\n{mixed}"
    );
    assert_eq!(
        rows(engine, 8, &mixed_sql).await,
        vec![
            vec!["gold".to_string(), "2".to_string()],
            vec!["silver".to_string(), "1".to_string()]
        ]
    );
}

/// Register `file` under `name` as a workspace CSV table.
async fn register_csv(engine: &Engine, dir: &Path, name: &str, file: &str) {
    engine
        .catalog()
        .register(engine.catalog().table_spec(
            dir,
            &TableDef {
                name: name.into(),
                format: SourceFormat::Csv(CsvRead::default()),
                source: None,
                paths: vec![file.into()],
                partition_cols: Vec::new(),
                origin: TableOrigin::External,
            },
            &SourceDefs::default(),
        ))
        .await
        .unwrap_or_else(|e| panic!("a local file table '{name}': {e}"));
}

/// **JSON accessors over a remote column** — a phase of the test above, and the one this whole
/// binary exists for.
///
/// The mapping is a claim about **meaning**, not about syntax: `json_as_text` over the text a JSON
/// column arrives as, and `JSON_UNQUOTE(NULLIF(JSON_EXTRACT(…), CAST('null' AS JSON)))` on the
/// server, have to answer the same thing about the same document. So this asks the same questions
/// of the same documents on both sides and compares the answers, rather than asserting a spelling —
/// which is what the dialect's own unit tests do, where no server is needed.
///
/// **The questions are asked as filters**, and that is not a stylistic choice: the provider crate
/// cannot read a *computed* text column back from `MySQL` at all (see [`the driver's text-result
/// gap`](a_projected_json_value_is_the_drivers_own_gap)), so a projected JSON lookup is a driver
/// error rather than an answer. A filter is answered on the server and never comes back as a
/// column, which is both the case that works today and the one the pushdown exists for.
///
/// The rows that carry the argument are `2` (no `z` at all) and the pair `1` / `3`, where `z` is
/// JSON `null` in one and the *string* `"null"` in the other. A bare `->>` renders both as the text
/// `null`, and `JSON_CONTAINS_PATH` is the only spelling that tells "the key is absent" from "its
/// value is null" the way `json_contains` does. Row `4`'s document is `NULL`, which is where
/// `JSON_CONTAINS_PATH` answers `NULL` and `json_contains` answers `false`.
async fn json_pushdown(engine: &Engine, dir: &Path) {
    fs::create_dir_all(dir).expect("a fixture folder");
    fs::write(dir.join("local_events.csv"), LOCAL_EVENTS).expect("the fixture");
    register_csv(engine, dir, "local_events", "local_events.csv").await;

    let mut tag = 60;
    for (predicate, expected, why) in [
        (
            "json_as_text(payload, 'type') = 'click'",
            vec!["1", "3"],
            "the headline lookup",
        ),
        (
            "json_as_text(payload, 'source', 'campaign') = 'spring'",
            vec!["1"],
            "a chained path walks the object on the server",
        ),
        (
            "json_as_text(payload, 'z') = 'null'",
            vec!["3"],
            "the string \"null\" is a value, and JSON null is not it — which a bare '->>' cannot \
             tell apart",
        ),
        (
            "json_as_text(payload, 'z') IS NULL",
            vec!["1", "2", "4"],
            "JSON null, an absent key and a NULL document all read as no value",
        ),
        (
            "json_contains(payload, 'z')",
            vec!["1", "3"],
            "a path resolves even when its value is null, and a NULL document contains nothing",
        ),
        (
            "json_contains(payload, 'missing')",
            Vec::new(),
            "…and an absent key resolves nowhere",
        ),
    ] {
        let expected: Vec<Vec<String>> =
            expected.iter().map(|id| vec![(*id).to_string()]).collect();
        let here = rows(
            engine,
            tag,
            &format!("SELECT id FROM local_events WHERE {predicate} ORDER BY id"),
        )
        .await;
        let there = rows(
            engine,
            tag + 1,
            &format!("SELECT id FROM {CATALOG}.shop.events WHERE {predicate} ORDER BY id"),
        )
        .await;
        assert_eq!(here, expected, "locally, '{predicate}': {why}");
        assert_eq!(
            there, expected,
            "and on the server, '{predicate}': {why} — the family has to answer the same thing \
             wherever it ran"
        );
        tag += 2;
    }

    let clicks = format!(
        "SELECT id FROM {CATALOG}.shop.events WHERE (payload ->> 'type') = 'click' ORDER BY id"
    );
    let plan = explain(engine, 80, &clicks).await;
    let federated: Vec<&str> = plan
        .lines()
        .filter(|line| line.contains("VirtualExecutionPlan"))
        .collect();
    assert_eq!(federated.len(), 1, "the whole read federates:\n{plan}");
    assert!(
        federated[0].contains("base_sql=")
            && federated[0].contains("JSON_EXTRACT")
            && !federated[0].contains("json_as_text"),
        "the statement that leaves carries the server's own expression, not the function:\n{}",
        federated[0]
    );
    assert!(
        !federated[0].contains("rewritten_executor_sql="),
        "and it leaves that way because it was unparsed that way, not rewritten afterwards:\n{}",
        federated[0]
    );

    let Err(why) = engine
        .ws(WsId(1))
        .query(
            RunTag(81),
            format!("SELECT payload -> 'type' FROM {CATALOG}.shop.events"),
            200,
        )
        .await
    else {
        panic!("'->' returns a union type no MySQL expression produces");
    };
    let why = why.to_string();
    assert!(
        why.contains("'json_get'")
            && why.contains("'->>'")
            && why.contains(&format!("'{CATALOG}'")),
        "the refusal names the function, the spelling that works and the data source: {why}"
    );

    let Err(why) = engine
        .ws(WsId(1))
        .query(
            RunTag(82),
            format!("SELECT json_length(payload) FROM {CATALOG}.shop.events"),
            200,
        )
        .await
    else {
        panic!("MySQL's own JSON_LENGTH is a different function, so this must not be sent");
    };
    let why = why.to_string();
    assert!(
        why.contains("'json_length'") && why.contains("CREATE TABLE"),
        "an unmapped member names itself and the way out: {why}"
    );
    assert!(
        !why.contains("does not exist"),
        "…and it is Strata's sentence rather than a raw errno: {why}"
    );

    let refused = explain(
        engine,
        83,
        &format!("SELECT id FROM {CATALOG}.shop.events WHERE json_get_str(payload, 'type') = 'x'"),
    )
    .await;
    assert!(
        refused.contains("VirtualExecutionPlan") && !refused.contains("base_sql="),
        "a plan whose statement cannot be written down shows no statement:\n{refused}"
    );

    let Err(why) = engine
        .ws(WsId(1))
        .query(
            RunTag(84),
            format!("SELECT json_as_text(payload, 'first-name') FROM {CATALOG}.shop.events"),
            200,
        )
        .await
    else {
        panic!("a key a MySQL path can only carry in quotes has no spelling here");
    };
    assert!(
        why.to_string().contains("'first-name'"),
        "the refusal names the key: {why}"
    );

    assert_eq!(
        rows(
            engine,
            85,
            "SELECT json_length(payload), json_get_str(payload, 'type') FROM local_events \
             WHERE id = 1"
        )
        .await,
        vec![vec!["3".to_string(), "click".to_string()]],
        "none of this reaches a local column: the rewrite rides the remote executor only"
    );

    a_projected_json_value_is_the_drivers_own_gap(engine).await;
}

/// **The driver cannot read a computed text column back from `MySQL`**, which is why every question
/// above is asked as a filter — pinned here so it fails loudly the day that stops being true.
///
/// **It is not a JSON problem**, and the last two assertions are what say so: a stored `TEXT`
/// column reads, and `upper()` over that same column does not. Nothing about that expression is
/// Strata's — it is the plainest possible query — so the gap is the driver's and the JSON mapping
/// merely reaches it on the ordinary path.
///
/// `datafusion-table-providers-mysql` maps `MYSQL_TYPE_BLOB` and refuses `MYSQL_TYPE_TINY_BLOB`,
/// `MEDIUM_BLOB` and `LONG_BLOB`, on the belief (its own comment says so) that the first covers the
/// whole `TEXT` family. It does — for a **stored** column. A *computed* one is reported by its
/// declared maximum length, so `JSON_UNQUOTE(…)` is `LONG_BLOB` and even `UPPER(<a TEXT column>)`
/// is `MEDIUM_BLOB`; both are refused. Nothing in Strata can spell around it: every `MySQL`
/// expression that yields the full text of a JSON value is a long-text expression, and the bounded
/// `CHAR(n)` cast that would be readable truncates silently.
///
/// So this is the driver's gap, not the mapping's, and `UPSTREAM_REPORTS.md` carries it. What the
/// user gets is the crate's own sentence, which names the type and its issue tracker.
async fn a_projected_json_value_is_the_drivers_own_gap(engine: &Engine) {
    let Err(why) = engine
        .ws(WsId(1))
        .query(
            RunTag(90),
            format!("SELECT json_as_text(payload, 'type') FROM {CATALOG}.shop.events"),
            200,
        )
        .await
    else {
        panic!(
            "the driver has learned to read MySQL's long-text results: drop this phase, drop the \
             UPSTREAM_REPORTS.md entry, and ask the questions above as projections"
        );
    };
    assert!(
        why.to_string().contains("MYSQL_TYPE_LONG_BLOB"),
        "…and while it has not, the failure is the driver's own, naming the type: {why}"
    );
    assert_eq!(
        rows(
            engine,
            91,
            &format!("SELECT id FROM {CATALOG}.shop.events WHERE id = 1")
        )
        .await,
        vec![vec!["1".to_string()]],
        "the document itself still reads: MYSQL_TYPE_JSON is mapped, and only the computed \
         text is not"
    );

    assert_eq!(
        rows(
            engine,
            92,
            &format!("SELECT body FROM {CATALOG}.shop.notes")
        )
        .await,
        vec![vec!["shipped late".to_string()]],
        "a stored TEXT column reads, because the server reports it as MYSQL_TYPE_BLOB"
    );
    let Err(why) = engine
        .ws(WsId(1))
        .query(
            RunTag(93),
            format!("SELECT upper(body) FROM {CATALOG}.shop.notes"),
            200,
        )
        .await
    else {
        panic!("the driver has learned to read MySQL's computed text results");
    };
    assert!(
        why.to_string().contains("MEDIUM_BLOB"),
        "**and none of this is about JSON**: uppercasing a plain TEXT column reports the same \
         family of type and is refused by the same arm, which is what makes the gap the driver's \
         rather than the JSON mapping's: {why}"
    );
}

/// **Every write refuses while the toggle is on, and the reserved namespace is the workspace's
/// own** — a phase of the test above.
///
/// The data source is connected `read_only: true`, the shipped default, so the two write
/// statements DataFusion can plan are refused here as well as the five the router intercepts —
/// **by the def's own gate**, not by anything about the kind. [`remote_writes`] is the other half:
/// it re-connects with the toggle off and every one of these statements lands. What each refusal
/// has to do is name the **data source**, so the sentence is about the target rather than about
/// SQL.
async fn statement_policy(engine: &Engine, dir: &Path) {
    engine.set_data_dir(dir);
    for sql in [
        format!("DROP TABLE {CATALOG}.shop.orders"),
        format!("DROP VIEW {CATALOG}.shop.big_orders"),
        format!("CREATE TABLE {CATALOG}.shop.mine (id INT)"),
        format!("CREATE VIEW {CATALOG}.shop.mine AS SELECT 1 AS id"),
        format!("CREATE EXTERNAL TABLE {CATALOG}.shop.mine STORED AS PARQUET LOCATION 'x.parquet'"),
        format!("INSERT INTO {CATALOG}.shop.orders VALUES (9, 9, 9, NULL)"),
        format!("CREATE TABLE {CATALOG}.shop.mine AS SELECT 1 AS id"),
    ] {
        let Err(why) = engine.ws(WsId(1)).run(RunTag(21), sql.clone(), 200).await else {
            panic!("'{sql}' was not refused");
        };
        assert!(
            why.to_string()
                .contains(&format!("data source '{CATALOG}'"))
                || why.to_string().contains(&format!("source '{CATALOG}'")),
            "'{sql}' must name the data source: {why}"
        );
    }

    assert_eq!(
        rows(
            engine,
            22,
            &format!("SELECT count(*) FROM {CATALOG}.shop.orders")
        )
        .await,
        vec![vec!["3".to_string()]],
        "and nothing it refused changed anything"
    );

    let Err(why) = engine
        .ws(WsId(1))
        .run(RunTag(23), "SELECT * FROM __snap_1".to_string(), 200)
        .await
    else {
        panic!("the workspace's snapshot namespace is reserved");
    };
    assert!(why.to_string().contains("__snap_"), "{why}");
    let Err(why) = engine
        .ws(WsId(1))
        .run(
            RunTag(24),
            format!("SELECT * FROM {CATALOG}.shop.__snap_1"),
            200,
        )
        .await
    else {
        panic!("the server has no such relation");
    };
    assert!(
        !why.to_string().contains("reserved"),
        "a remote relation is not in Strata's reserved namespace: {why}"
    );
}

/// **Unqualified names** — a phase of the test above, and the half a fake catalog cannot show:
/// whether a bare name genuinely *reaches* the server's relation.
///
/// Runs after [`statement_policy`], which set the data root a workspace table needs.
async fn unqualified_names(engine: &Engine, port: u16) {
    assert_eq!(
        rows(engine, 40, "SELECT count(*) FROM orders").await,
        vec![vec!["3".to_string()]],
        "a bare name only the database has reads the database"
    );
    assert_eq!(
        rows(engine, 41, "SELECT id FROM big_orders").await,
        vec![vec!["1".to_string()]],
        "a remote **view** resolves like any other relation"
    );
    assert!(
        engine
            .ws(WsId(1))
            .run(RunTag(42), "SELECT * FROM sessions".to_string(), 200)
            .await
            .is_err(),
        "a database the data source does not show must not capture a bare name"
    );

    let _ = engine
        .sources()
        .show_schemas(CATALOG, &["shop".to_string(), "analytics".to_string()]);
    assert_eq!(
        rows(engine, 43, "SELECT count(*) FROM sessions").await,
        vec![vec!["2".to_string()]],
        "showing the database is what puts it in reach of a bare name"
    );

    let Err(why) = engine
        .ws(WsId(1))
        .run(RunTag(44), "SELECT * FROM orders".to_string(), 200)
        .await
    else {
        panic!("two relations of that name and one of them was picked");
    };
    let why = why.to_string();
    assert!(
        why.contains(&format!("{CATALOG}.shop.orders"))
            && why.contains(&format!("{CATALOG}.analytics.orders")),
        "the refusal names every candidate: {why}"
    );

    let _ = engine
        .sources()
        .show_schemas(CATALOG, &["shop".to_string()]);
    assert_eq!(
        rows(engine, 45, "SELECT count(*) FROM orders").await,
        vec![vec!["3".to_string()]],
        "with `analytics` hidden the name has one candidate again"
    );

    let Err(why) = engine
        .ws(WsId(1))
        .run(
            RunTag(46),
            "INSERT INTO customers VALUES (30, 'x')".to_string(),
            200,
        )
        .await
    else {
        panic!("a write to a bare name that resolves remote was accepted");
    };
    assert!(
        why.to_string().contains(CATALOG),
        "a write target is refused as remote, not as missing: {why}"
    );

    engine
        .ws(WsId(1))
        .run(
            RunTag(47),
            "CREATE VIEW remote_orders AS SELECT id, total FROM orders".to_string(),
            200,
        )
        .await
        .expect("the view is created over the resolved name");
    assert_eq!(
        rows(engine, 48, "SELECT count(*) FROM remote_orders").await,
        vec![vec!["3".to_string()]],
        "…and it reads the server through the name it captured"
    );
    engine
        .ws(WsId(1))
        .run(RunTag(49), "DROP VIEW remote_orders".to_string(), 200)
        .await
        .expect("the view drops");

    let _ = port;
}

/// **Writing into a database** — a phase of the test above, and the one no fake catalog can stand
/// in for: an insert is only real once a server has taken it.
///
/// Opting in is a **re-connect with the toggle off**, which is exactly what the data source
/// editor's Save does, so nothing here reaches past the def to arrange it.
async fn remote_writes(engine: &Engine, port: u16) {
    let conn = writable(port, CATALOG, &["shop"]);
    engine
        .sources()
        .connect(conn.clone())
        .await
        .expect("the same data source, opted in to writes");

    let catalog = sql::Symbols::build([], [], engine.lang().bundle(), String::new());
    let offered: Vec<String> = sql::complete(&catalog, "INSERT INTO ", 12, false)
        .into_iter()
        .map(|c| c.label)
        .collect();
    assert!(
        offered.contains(&"customers".to_string()),
        "opting in is what puts the relations at a write position: {offered:?}"
    );

    let RunOutcome::Statement(report) = engine
        .ws(WsId(1))
        .run(
            RunTag(100),
            format!(
                "CREATE TABLE {CATALOG}.shop.loaded AS SELECT t.tier, o.total \
                 FROM tiers t JOIN {CATALOG}.shop.orders o ON t.customer = o.customer"
            ),
            200,
        )
        .await
        .expect("a cross-source result materializes as a server table")
    else {
        panic!("CREATE TABLE AS ran as a query");
    };
    assert_eq!(report.message, "Table 'my.shop.loaded' created, 3 rows");
    assert_eq!(report.count, Some(3));
    assert_eq!(report.effect, Some(StoreEffect::RemoteRelationsChanged));

    let listing = schemas_of(engine, &conn.named());
    assert!(
        listing
            .iter()
            .find(|schema| schema.name == "shop")
            .is_some_and(|schema| schema.relations.iter().any(|r| r.name == "loaded")),
        "the tree and completion see it with no manual refresh"
    );
    assert_eq!(
        rows(
            engine,
            101,
            &format!("SELECT count(*) FROM {CATALOG}.shop.loaded")
        )
        .await,
        vec![vec!["3".to_string()]],
        "and the rows are on the server"
    );

    let RunOutcome::Statement(report) = engine
        .ws(WsId(1))
        .run(
            RunTag(102),
            format!("INSERT INTO {CATALOG}.shop.loaded VALUES ('bronze', 1), ('tin', 2)"),
            200,
        )
        .await
        .expect("literal rows land")
    else {
        panic!("INSERT ran as a query");
    };
    assert_eq!(report.count, Some(2));
    let RunOutcome::Statement(report) = engine
        .ws(WsId(1))
        .run(
            RunTag(103),
            format!("INSERT INTO {CATALOG}.shop.loaded SELECT tier, 0 FROM tiers"),
            200,
        )
        .await
        .expect("a local table's rows land too")
    else {
        panic!("INSERT ran as a query");
    };
    assert_eq!(report.count, Some(2));
    assert_eq!(
        rows(
            engine,
            104,
            &format!("SELECT count(*) FROM {CATALOG}.shop.loaded")
        )
        .await,
        vec![vec!["7".to_string()]],
        "…and the server holds all of them"
    );

    ctas_name_semantics(engine).await;
    failed_ctas_leaves_nothing(engine, &conn).await;

    engine
        .sources()
        .connect(source(port, CATALOG, &["shop"]))
        .await
        .expect("the data source back to read-only");
    let Err(why) = engine
        .ws(WsId(1))
        .run(
            RunTag(109),
            format!("INSERT INTO {CATALOG}.shop.loaded VALUES ('after', 1)"),
            200,
        )
        .await
    else {
        panic!("the toggle is what allows the write, and it is off again");
    };
    assert!(why.to_string().contains("read-only"), "{why}");
}

/// **A CTAS answers about a name by trying to take it**, which is this backend's whole settlement:
/// with no transactional DDL there is nothing to ask a question inside, so the create is made
/// without `IF NOT EXISTS` and the server's own collision (errno 1050) is the answer.
///
/// The property that buys is asserted last and is the one that matters: a refused CTAS leaves the
/// relation it collided with **untouched**. A pre-check that could go stale would have adopted it,
/// and the rollback would then have dropped somebody else's table.
async fn ctas_name_semantics(engine: &Engine) {
    let Err(why) = engine
        .ws(WsId(1))
        .run(
            RunTag(105),
            format!("CREATE TABLE {CATALOG}.shop.loaded AS SELECT 1 AS n"),
            200,
        )
        .await
    else {
        panic!("the relation is already there");
    };
    assert_eq!(why.to_string(), "Table 'my.shop.loaded' already exists");

    let RunOutcome::Statement(report) = engine
        .ws(WsId(1))
        .run(
            RunTag(106),
            format!("CREATE TABLE IF NOT EXISTS {CATALOG}.shop.loaded AS SELECT 1 AS n"),
            200,
        )
        .await
        .expect("reported rather than refused")
    else {
        panic!("ran as a query");
    };
    assert_eq!(report.message, "Table 'my.shop.loaded' already exists");
    assert_eq!(report.effect, None, "and nothing changed");

    assert_eq!(
        rows(
            engine,
            107,
            &format!("SELECT count(*) FROM {CATALOG}.shop.loaded")
        )
        .await,
        vec![vec!["7".to_string()]],
        "the relation a CTAS collided with is left exactly as it was"
    );
}

/// **A CTAS whose insert fails leaves no table behind.** The create lands, the fill does not, and
/// the rollback takes the relation back off the server — otherwise the user is left with an empty
/// table under a name they believe holds their result.
///
/// The failure is DataFusion's own cast of a **local** column: `MySQL` would coerce `'gold'` to `0`
/// rather than refuse it, so a server-side cast could not fail this. Reading `tiers` locally puts
/// the cast on this side, where it fails once rows are moving — after the create.
async fn failed_ctas_leaves_nothing(engine: &Engine, conn: &SourceDef) {
    let Err(why) = engine
        .ws(WsId(1))
        .run(
            RunTag(108),
            format!(
                "CREATE TABLE {CATALOG}.shop.doomed AS SELECT CAST(tier AS INT) AS n FROM tiers"
            ),
            200,
        )
        .await
    else {
        panic!("'gold' is not an integer");
    };
    assert!(!why.to_string().contains("already exists"), "{why}");

    let listing = schemas_of(engine, &conn.named());
    assert!(
        listing
            .iter()
            .find(|schema| schema.name == "shop")
            .is_some_and(|schema| schema.relations.iter().all(|r| r.name != "doomed")),
        "the half-made table went with the failure"
    );
    assert!(
        engine
            .ws(WsId(1))
            .query(
                RunTag(110),
                format!("SELECT n FROM {CATALOG}.shop.doomed"),
                200
            )
            .await
            .is_err(),
        "…and nothing resolves under its name"
    );
}

/// **The statements the server runs** — a phase of the test above, and the one only a server can
/// settle: a spliced statement is either `MySQL`'s own SQL or it is a syntax error.
///
/// The identifier rule is the load-bearing half here. Every name Strata composes into one of these
/// goes out in **backticks** ([`server_ident`](strata_engine::sources::mysql)), which is neither
/// SQL's rule nor DataFusion's — a statement written with double quotes would be a syntax error on
/// a server in its default mode.
async fn remote_statements(engine: &Engine, port: u16) {
    let conn = writable(port, CATALOG, &["shop"]);
    engine
        .sources()
        .connect(conn.clone())
        .await
        .expect("opted in to writes");

    let RunOutcome::Statement(report) = engine
        .ws(WsId(1))
        .run(
            RunTag(120),
            format!(
                "CREATE VIEW {CATALOG}.shop.rich AS SELECT id, total FROM {CATALOG}.shop.orders \
                 WHERE total > 40"
            ),
            200,
        )
        .await
        .expect("the server takes a view of its own")
    else {
        panic!("CREATE VIEW ran as a query");
    };
    assert_eq!(report.message, "View 'shop.rich' created on 'my'");
    assert_eq!(report.effect, Some(StoreEffect::RemoteRelationsChanged));

    let listing = schemas_of(engine, &conn.named());
    assert!(
        listing
            .iter()
            .find(|schema| schema.name == "shop")
            .is_some_and(|schema| schema.relations.iter().any(|r| r.name == "rich" && r.view)),
        "the tree sees the view, as a view, with no manual refresh"
    );
    assert_eq!(
        rows(
            engine,
            121,
            &format!("SELECT id FROM {CATALOG}.shop.rich ORDER BY id")
        )
        .await,
        vec![vec!["1".to_string()], vec!["3".to_string()]],
        "and it reads"
    );

    let RunOutcome::Statement(report) = engine
        .ws(WsId(1))
        .run(
            RunTag(122),
            format!("UPDATE {CATALOG}.shop.loaded SET total = 99 WHERE tier = 'bronze'"),
            200,
        )
        .await
        .expect("the server runs its own DML")
    else {
        panic!("UPDATE ran as a query");
    };
    assert_eq!(
        report.count,
        Some(1),
        "…and reports the server's own affected-row count: {}",
        report.message
    );
    assert_eq!(
        rows(
            engine,
            123,
            &format!("SELECT total FROM {CATALOG}.shop.loaded WHERE tier = 'bronze'")
        )
        .await,
        vec![vec!["99".to_string()]],
        "confirmed by reading it back"
    );

    let RunOutcome::Statement(report) = engine
        .ws(WsId(1))
        .run(
            RunTag(124),
            format!("DELETE FROM {CATALOG}.shop.loaded WHERE total = 0"),
            200,
        )
        .await
        .expect("and a delete")
    else {
        panic!("DELETE ran as a query");
    };
    assert_eq!(report.count, Some(2));

    for sql in [
        format!("DROP VIEW {CATALOG}.shop.rich"),
        format!("DROP TABLE {CATALOG}.shop.loaded"),
    ] {
        engine
            .ws(WsId(1))
            .run(RunTag(125), sql.clone(), 200)
            .await
            .unwrap_or_else(|e| panic!("'{sql}': {e}"));
    }
    let listing = schemas_of(engine, &conn.named());
    assert!(
        listing
            .iter()
            .find(|schema| schema.name == "shop")
            .is_some_and(|schema| schema
                .relations
                .iter()
                .all(|r| r.name != "rich" && r.name != "loaded")),
        "and what the server no longer holds is out of the tree"
    );

    engine
        .sources()
        .connect(source(port, CATALOG, &["shop"]))
        .await
        .expect("the data source back to read-only");
    let Err(why) = engine
        .ws(WsId(1))
        .run(
            RunTag(126),
            format!("DROP TABLE {CATALOG}.shop.orders"),
            200,
        )
        .await
    else {
        panic!("the toggle is what allows the statement, and it is off again");
    };
    assert!(why.to_string().contains("read-only"), "{why}");
}

/// **A reconnect replaces, a rename does not, and a disconnect stops resolving** — a phase of the
/// test above.
///
/// **The rename owes the keystore nothing**, which is what recording the slot bought: the ref
/// travels in the def, so a renamed data source logs in with the password it already had. Last
/// phase, so the rename is nobody's problem afterwards.
async fn reconnect_and_disconnect(engine: &Engine, port: u16) {
    let was = source(port, CATALOG, &["shop"]);
    let renamed = SourceDef {
        name: "warehouse".into(),
        ..was.clone()
    };
    engine
        .sources()
        .connect(renamed.clone())
        .await
        .expect("the same data source under a new catalog name");
    assert_eq!(
        rows(engine, 14, "SELECT count(*) FROM warehouse.shop.orders").await,
        vec![vec!["3".to_string()]]
    );
    assert!(
        engine
            .ws(WsId(1))
            .query(
                RunTag(15),
                format!("SELECT id FROM {CATALOG}.shop.orders"),
                200
            )
            .await
            .is_ok(),
        "the old name is still registered until something retires it: two data sources may share \
         an identity, so nothing the engine sees tells a rename from a second source"
    );

    let _ = engine.sources().disconnect(&was.named());
    assert!(
        engine
            .ws(WsId(1))
            .query(
                RunTag(16),
                format!("SELECT id FROM {CATALOG}.shop.orders"),
                200
            )
            .await
            .is_err(),
        "and retiring it is the renaming gesture's own call, which is what Save makes"
    );

    let _ = engine.sources().disconnect(&renamed.named());
    assert!(
        engine
            .ws(WsId(1))
            .query(
                RunTag(17),
                "SELECT id FROM warehouse.shop.orders".to_string(),
                200
            )
            .await
            .is_err(),
        "a forgotten data source's catalog must stop resolving"
    );
    assert!(
        !live(engine, &renamed),
        "…and it is no longer a live database"
    );
}
