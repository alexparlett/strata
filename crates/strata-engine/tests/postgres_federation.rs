//! **A database connection, against a real `PostgreSQL`** (DB-02) — the half no unit test can
//! reach.
//!
//! Everything the Postgres arm does is a round trip: the pool's construction *is* the probe, the
//! catalog is built from an introspection query, a provider is built from a second one, and the
//! point of the whole workstream — a same-source subplan leaving as one SQL statement — only
//! exists once a server has parsed and answered it. A unit test can prove a `ConnectionDef`
//! yields a well-formed parameter map, and nothing beyond that.
//!
//! **A real server rather than a mock**, the same argument as `object_store_minio.rs`: the
//! unparser's output is judged by the *server*, and a stand-in accepting whatever we sent would
//! assert nothing. The fixture is seeded over raw `tokio-postgres`, a lower layer than the pool and
//! factory under test.
//!
//! **It drives `Engine::connect`, the real entry point, password and all**, because the keystore
//! bridge (`SecretRef::derived` → `KeystorePassword` → one read per pool connection) is the
//! genuinely new machinery. What it substitutes is the *keystore*, not the bridge:
//! `keyring_core::mock` is this binary's store, so no real Keychain is touched — that round trip
//! stays `tests/secret_keystore.rs`'s, a separate binary because a process has one default store.
//!
//! **Deliberately not `#[ignore]`d**, for the reason the MinIO test is not. One test, sequential
//! phases, container held for the duration: a second `#[tokio::test]` would race this one for the
//! single cloud worker.
//!
//! **This is the SQL ring's conformance run.** Everything it drives past the fixture is the
//! generic path — connect, enumerate, resolve, federate, write, dispatch — reached through
//! `DataSource` and the registry, so a source of your own registered under its own kind is
//! exercised by the same phases: point the fixture at your server and change the kind the def
//! names.
//!
//! Gated on the `postgres` feature, because the source it drives rides that feature: an engine
//! built without it has no `PostgreSQL` in its tree, and neither does this.
#![cfg(feature = "postgres")]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;
use std::path::Path;
use std::sync::Once;
use std::time::{Duration, Instant};
use std::{env, fs, process};

use datafusion::arrow::datatypes::{DataType, Field};
use keyring_core::mock;
use strata_arrow::column_info;
use strata_arrow::profile::Profiled;
use strata_core::project::ProjectDefs;

use strata_engine::profile::{aggregates, profile_sql};
use strata_engine::register::{register_project, table_spec, RegOutcome};
use strata_engine::sources::postgres::settings::PASSWORD as PASSWORD_KEY;
use strata_engine::sources::{migrate_secrets, put_secret, SchemaVisibility};
use strata_engine::{
    sql, stopped_on_purpose, Connections, Engine, RunOutcome, RunTag, StoreEffect, ViewMeta, WsId,
};
use strata_model::{
    Cell, ConnectionDef, CsvRead, Provider, SourceDef, SourceFormat, StatKey, TableDef,
    TableOrigin, ViewDef,
};
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;

/// The image's own defaults (`testcontainers_modules::postgres` starts it with them).
const USER: &str = "postgres";
const PASSWORD: &str = "postgres";
const DATABASE: &str = "postgres";
/// How queries address the connection — the catalog half of `catalog.schema.table`.
const CATALOG: &str = "pg";

/// The fixture. Two tables in `public` that can be joined, one in a schema of its own (so the
/// three-part addressing and the schema-visibility scoping have something to be about), one
/// **view** (which the crate's `pg_tables` listing would have missed — one of the three reasons
/// the enumeration is ours), and a `jsonb` column (which the crate's default type action would
/// have refused, taking the whole table with it).
///
/// `public.events` is DB-08's own: a `jsonb` column with a nested object under it, so a chained
/// accessor has a path to walk and a key only one row has.
const SEED: &str = "\
CREATE TABLE public.orders (id INT PRIMARY KEY, customer INT, total INT, tags JSONB);
INSERT INTO public.orders VALUES
  (1, 10, 99, '{\"channel\":\"web\"}'),
  (2, 10, 10, '{\"channel\":\"store\"}'),
  (3, 20, 42, '{\"channel\":\"web\"}');
CREATE TABLE public.customers (id INT PRIMARY KEY, name TEXT);
INSERT INTO public.customers VALUES (10, 'acme'), (20, 'globex');
CREATE VIEW public.big_orders AS SELECT id, total FROM public.orders WHERE total > 50;
CREATE TABLE public.events (id INT PRIMARY KEY, payload JSONB);
INSERT INTO public.events VALUES
  (1, '{\"type\":\"click\",\"source\":{\"campaign\":\"spring\"}}'),
  (2, '{\"type\":\"view\",\"source\":{\"campaign\":\"winter\"}}'),
  (3, '{\"type\":\"click\",\"source\":{\"campaign\":\"spring\"},\"bot\":true}');
CREATE SCHEMA analytics;
CREATE TABLE analytics.sessions (id INT PRIMARY KEY, minutes INT);
INSERT INTO analytics.sessions VALUES (1, 5), (2, 9);
";

/// See `object_store_minio.rs` — same budget, same reason (a hosted runtime hands this account
/// one worker at a time, and an overlap is a handover rather than a queue).
const CAPACITY_RETRY_BUDGET: Duration = Duration::from_secs(90);
const CAPACITY_RETRY_GAP: Duration = Duration::from_secs(10);

/// Is this failure the runtime saying **busy**, rather than saying no? The MinIO test's own
/// predicate, for its own reason — two spellings, one fault, and everything else falls through
/// to the panic, because "no runtime" must never look like "the code is fine".
fn at_capacity(err: &impl Display) -> bool {
    let msg = err.to_string();
    msg.contains("too many concurrent requests") || msg.contains("IncompleteMessage")
}

/// A running `PostgreSQL`, and the port it answers on. The container must be held for the test's
/// duration — dropping it stops the server.
async fn postgres() -> (ContainerAsync<Postgres>, u16) {
    let deadline = Instant::now() + CAPACITY_RETRY_BUDGET;
    let container = loop {
        match Postgres::default().start().await {
            Ok(container) => break container,
            Err(err) if at_capacity(&err) && Instant::now() < deadline => {
                eprintln!("container runtime is at capacity, retrying: {err}");
                tokio::time::sleep(CAPACITY_RETRY_GAP).await;
            }
            Err(err) => panic!("PostgreSQL starts (is a Docker runtime available?): {err}"),
        }
    };
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("PostgreSQL's port");
    (container, port)
}

/// Seed the database over the raw driver — deliberately a different layer from the pool and
/// factory under test.
async fn seed(port: u16) {
    let (client, connection) = tokio_postgres::connect(
        &format!("host=127.0.0.1 port={port} user={USER} password={PASSWORD} dbname={DATABASE}"),
        tokio_postgres::NoTls,
    )
    .await
    .expect("connect to seed");
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("seed connection ended: {e}");
        }
    });
    client.batch_execute(SEED).await.expect("seed the database");
}

/// The connection under test. `sslmode=disable` because the container serves plain TCP; the two
/// verifying modes are the provider crate's emulation and would need a certificate this fixture
/// has no way to produce.
///
/// **Read-only**, which is the shipped default — [`writable`] is what the write phases connect
/// with, so every phase before them is driven through exactly the connection a user gets.
fn connection(port: u16, catalog: &str, schemas: &[&str]) -> ConnectionDef {
    ConnectionDef {
        address: format!("127.0.0.1:{port}/{DATABASE}"),
        name: catalog.into(),
        provider: Provider::Source(SourceDef {
            kind: "postgres".into(),
            config: BTreeMap::from([
                ("user".to_string(), USER.to_string()),
                ("sslmode".to_string(), "disable".to_string()),
            ]),
            secrets: BTreeSet::from([PASSWORD_KEY.to_string()]),
            schemas: schemas.iter().map(|s| (*s).to_string()).collect(),
            read_only: true,
        }),
        client_config: BTreeMap::new(),
    }
}

/// The same connection opted in to writes (DB-10) — the one setting that separates them, so a
/// re-connect with this is exactly the user turning the toggle off.
fn writable(port: u16, catalog: &str, schemas: &[&str]) -> ConnectionDef {
    let mut def = connection(port, catalog, schemas);
    if let Provider::Source(source) = &mut def.provider {
        source.read_only = false;
    }
    def
}

/// **The keystore this binary uses**, installed once: an in-memory store, so the bridge under
/// test is real and the platform Keychain is untouched. See the module docs.
fn mocked_keystore() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        keyring_core::set_default_store(mock::Store::new().expect("a mock keystore"));
    });
}

/// File `value` under the slot `conn` derives, exactly as the connection editor will — the
/// def stores no reference, so this is the only place the id exists.
fn store_password(conn: &ConnectionDef, value: &str) {
    put_secret(conn, PASSWORD_KEY, value).expect("this machine's keystore answers");
}

/// Run `sql` and hand back its first page as text, row by row.
async fn rows(engine: &Engine, tag: u128, sql: &str) -> Vec<Vec<String>> {
    let (output, _) = engine
        .query(WsId(1), RunTag(tag), sql.to_string(), 200)
        .await
        .unwrap_or_else(|e| panic!("run '{sql}': {e}"));
    output
        .rows
        .iter()
        .map(|row| row.iter().map(|cell: &Cell| cell.text.clone()).collect())
        .collect()
}

/// Both plan texts, for the pushdown assertions.
///
/// `Engine::explain`, **not** a `SELECT`-shaped `EXPLAIN` through [`rows`]: a result cell is
/// clipped to `DISPLAY_CHARS` for the grid, so a deep physical plan loses its tail — which is
/// exactly where `VirtualExecutionPlan` sits. This is the same unclipped read the plan view
/// makes.
async fn explain(engine: &Engine, tag: u128, sql: &str) -> String {
    let plan = engine
        .explain(WsId(1), RunTag(tag), sql.to_string())
        .await
        .unwrap_or_else(|e| panic!("explain '{sql}': {e}"));
    format!("{}\n{}", plan.logical_text, plan.physical_text)
}

#[tokio::test]
async fn a_database_connection_registers_a_federated_catalog() {
    mocked_keystore();
    let (_pg, port) = postgres().await;
    seed(port).await;

    let engine = Engine::builder().build();
    let conn = connection(port, CATALOG, &["public"]);
    store_password(&conn, PASSWORD);

    let missing = connection(port, "no_password", &["public"]);
    store_password(&missing, "");
    let why = engine
        .connect(missing.clone())
        .await
        .expect_err("no password is stored for it");
    assert!(
        why.contains("No password is stored on this machine") && why.contains(&missing.named()),
        "the refusal names the machine and the connection: {why}"
    );

    let closed = {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = taken.local_addr().expect("an address").port();
        drop(taken);
        port
    };
    let elsewhere = connection(closed, "elsewhere", &["public"]);
    store_password(&elsewhere, PASSWORD);
    let why = engine
        .connect(elsewhere)
        .await
        .expect_err("nothing is listening there");
    assert!(
        why.contains(&format!("127.0.0.1:{closed}")),
        "the refusal names the address to fix: {why}"
    );

    let wrong_password = connection(port, "wrong_password", &["public"]);
    store_password(&wrong_password, "not-it");
    let why = engine
        .connect(wrong_password)
        .await
        .expect_err("the password is wrong");
    assert!(why.contains(USER), "the refusal names the user: {why}");

    let reserved = connection(port, "strata", &["public"]);
    store_password(&reserved, PASSWORD);
    let why = engine
        .connect(reserved)
        .await
        .expect_err("'strata' is the workspace's own catalog");
    assert!(why.contains("strata"), "{why}");

    assert!(
        engine.source_listing(&conn).is_none(),
        "a refused connection registers nothing"
    );

    engine
        .connect(conn.clone())
        .await
        .expect("the connection registers its catalog");

    enumeration(&engine, port).await;
    qualified_offer(&engine, &conn).await;
    pushdown(&engine).await;
    profiling(&engine).await;
    let fixtures = env::temp_dir().join(format!("strata-pg-{}", process::id()));
    mixed_plan(&engine, &fixtures).await;
    exotic_types_and_refusals(&engine).await;
    json_pushdown(&engine, &fixtures).await;
    statement_policy(&engine, &fixtures).await;
    unqualified_names(&engine, port).await;
    remote_writes(&engine, port).await;
    remote_statements(&engine, port).await;
    remote_source_into_a_workspace_table(&engine, &fixtures).await;
    cross_source_views(port, &fixtures).await;
    reconnect_and_disconnect(&engine, port).await;

    let _ = fs::remove_dir_all(&fixtures);
}

/// **What the catalog says it holds** — a *phase*, called in sequence, not a test of its own
/// (a second `#[tokio::test]` would race this one for the single container worker).
async fn enumeration(engine: &Engine, port: u16) {
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
            vec!["analytics".to_string(), "sessions".to_string()],
            vec!["public".to_string(), "big_orders".to_string()],
            vec!["public".to_string(), "customers".to_string()],
            vec!["public".to_string(), "events".to_string()],
            vec!["public".to_string(), "orders".to_string()],
        ],
        "every schema the role can see, and every relation in them"
    );

    let (catalog, listing) = engine
        .source_listing(&connection(port, CATALOG, &["public", "warehouse"]))
        .expect("a live database has a listing");
    assert_eq!(catalog, CATALOG);
    assert_eq!(
        listing
            .iter()
            .map(|schema| (schema.name.as_str(), schema.visibility))
            .collect::<Vec<_>>(),
        vec![
            ("analytics", SchemaVisibility::NotEnabled),
            ("public", SchemaVisibility::Live),
            ("warehouse", SchemaVisibility::EnabledButMissing),
        ]
    );
    assert_eq!(
        listing
            .iter()
            .find(|schema| schema.name == "public")
            .map(|schema| schema
                .relations
                .iter()
                .map(|r| (r.name.as_str(), r.view))
                .collect::<Vec<_>>()),
        Some(vec![
            ("big_orders", true),
            ("customers", false),
            ("events", false),
            ("orders", false)
        ]),
        "a remote view is listed as one, which pg_tables could not have said"
    );

    assert_eq!(
        rows(
            engine,
            2,
            &format!("SELECT minutes FROM {CATALOG}.analytics.sessions ORDER BY 1")
        )
        .await,
        vec![vec!["5".to_string()], vec!["9".to_string()]]
    );
}

/// **What completion offers for this connection, and that the name it hands over runs** (DB-06)
/// — a phase of the test above.
///
/// The offer is unit-tested against a hand-built listing next door; what only a server can settle
/// is that the two halves agree — that the names `Engine::database_syms` carries are the names
/// the catalog actually resolves, rendered the way [`sql::qualified`] renders them. Which is also
/// the tree gestures' half: they wrap this same address in `SELECT *` / `CREATE VIEW`.
///
/// The offers are compared **sorted**, because what only a server can pin is *which* names the
/// offer holds; their ranking is `complete/tests.rs`'s and needs no server.
async fn qualified_offer(engine: &Engine, conn: &ConnectionDef) {
    let catalog = sql::Catalog::default().with_databases(engine.database_syms([conn]));
    let offer = |sql: &str| {
        let mut labels = sql::complete(sql, sql.len(), &catalog, false)
            .into_iter()
            .map(|c| c.label)
            .collect::<Vec<_>>();
        labels.sort();
        labels
    };

    assert_eq!(
        offer(&format!("SELECT * FROM {CATALOG}.")),
        vec!["public".to_string()],
        "the enabled schemas, and only those — `analytics` is on the server and off the def"
    );
    assert_eq!(
        offer(&format!("SELECT * FROM {CATALOG}.public.")),
        vec![
            "big_orders".to_string(),
            "customers".to_string(),
            "events".to_string(),
            "orders".to_string()
        ],
        "every relation the listing holds, remote view included"
    );
    assert!(
        offer(&format!("SELECT * FROM {CATALOG}.public.orders.")).is_empty(),
        "a remote relation's columns are an introspection, so the chain stops here"
    );

    let address = sql::qualified([CATALOG, "public", "orders"]);
    assert_eq!(
        rows(
            engine,
            40,
            &format!("SELECT * FROM {address} ORDER BY id LIMIT 1")
        )
        .await,
        vec![vec![
            "1".to_string(),
            "10".to_string(),
            "99".to_string(),
            "{\"channel\": \"web\"}".to_string()
        ]],
        "the address the tree's gestures compose is one the engine runs"
    );

    let not_enabled = sql::qualified([CATALOG, "analytics", "sessions"]);
    assert_eq!(
        rows(
            engine,
            41,
            &format!("SELECT minutes FROM {not_enabled} ORDER BY 1 LIMIT 1")
        )
        .await,
        vec![vec!["5".to_string()]],
        "a schema the def does not show still resolves — visibility, not policy"
    );
}

/// **Reads, and what actually reaches the server** — a phase of the test above.
async fn pushdown(engine: &Engine) {
    assert_eq!(
        rows(
            engine,
            3,
            &format!("SELECT id FROM {CATALOG}.public.orders ORDER BY id")
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
        &format!("SELECT id FROM {CATALOG}.public.orders WHERE customer = 20"),
    )
    .await;
    assert!(
        filtered.contains("base_sql=")
            && filtered.contains("WHERE")
            && filtered.contains("customer"),
        "the filter did not reach the remote statement:\n{filtered}"
    );

    let joined = explain(
        engine,
        5,
        &format!(
            "SELECT c.name, sum(o.total) FROM {CATALOG}.public.orders o \
             JOIN {CATALOG}.public.customers c ON c.id = o.customer GROUP BY c.name"
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
                "SELECT c.name, sum(o.total) FROM {CATALOG}.public.orders o \
                 JOIN {CATALOG}.public.customers c ON c.id = o.customer \
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
}

/// **Profiling a remote relation** (DB-07) — a phase of the test above.
///
/// The claim this settles is the one only a server can: that the whole aggregate really does
/// federate into **one** remote statement, and that `PostgreSQL` really does run it. What the
/// expression set *is* stays pinned next door in `engine::profile`'s own tests, against
/// DataFusion's `PostgreSQL` dialect, so a working tree with no container still fails if somebody
/// adds an aggregate the wire cannot carry.
///
/// **Both statements here are built from the set rather than typed out**, and that is the whole
/// discipline of this phase: a hand-written one explains itself, so an aggregate added to
/// `Profiled::wanted`'s Database arm that renders and then fails on the server would leave every
/// assertion below green.
///
/// `orders` is the right subject: `total` is numeric, which is the only column kind whose set
/// differs, and `tags` is `jsonb` — arriving as `Utf8` — so the scan also has to survive the type
/// mapping DB-02 chose.
async fn profiling(engine: &Engine) {
    let name = format!("{CATALOG}.public.orders");

    // **Built from the set, never typed out.** A hand-written statement here would explain
    // itself: an aggregate added to `Profiled::wanted`'s Database arm that the unparser renders
    // and the server lacks would federate, fail, and leave this phase green — which is the one
    // thing it exists to catch. Same discipline as
    // `unsplit_expression_set_fails_on_the_server` below.
    let columns = [
        column_info(&Field::new("total", DataType::Int32, true)),
        column_info(&Field::new("tags", DataType::Utf8, true)),
    ];
    let (exprs, _) = aggregates(&columns, Profiled::Database);
    let statement = profile_sql(&name, &exprs);
    assert!(
        !statement.is_empty(),
        "every expression in the remote set has to unparse before it can federate"
    );

    let plan = explain(engine, 50, statement.trim_end_matches(';')).await;
    let federated: Vec<&str> = plan
        .lines()
        .filter(|line| line.contains("VirtualExecutionPlan"))
        .collect();
    assert_eq!(
        federated.len(),
        1,
        "the profile's aggregate did not federate into one remote node:\n{plan}"
    );
    for expected in ["count(", "min(", "max(", "avg("] {
        assert!(
            federated[0].to_lowercase().contains(expected),
            "the remote statement is missing {expected}…):\n{}",
            federated[0]
        );
    }
    assert!(
        !federated[0].to_lowercase().contains("percentile"),
        "…and the median never reached the wire:\n{}",
        federated[0]
    );

    let profile = engine
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
    // Compared as a **number**, not as its rendering: `avg(integer)` is `numeric` on the server
    // and the connector maps that to a decimal, so how many digits reach `ArrayFormatter` is a
    // property of the type mapping rather than of the answer. The value is what this pins.
    let mean: f64 = fact(StatKey::Mean).parse().expect("a numeric mean");
    assert!(
        (mean - (99.0 + 10.0 + 42.0) / 3.0).abs() < 1e-9,
        "avg over an integer column comes back as a number: {mean}"
    );
    assert!(
        !total.iter().any(|s| s.key == StatKey::Median),
        "the median is absent **by design** — no PostgreSQL spelling, and no per-expression \
         fallback to catch one: {total:?}"
    );
    // **The jsonb column is why the remote set stops at a distinct count for strings.** DB-02 maps
    // it to `Utf8`, so it is indistinguishable from `text` here, and PostgreSQL has no
    // `min(jsonb)` — an ordered aggregate on it would fail this whole scan rather than one column.
    let tags = profile
        .cols
        .get("tags")
        .expect("the jsonb column is profiled");
    assert!(
        tags.iter().any(|s| s.key == StatKey::Distinct),
        "…counted: {tags:?}"
    );
    assert!(
        !tags
            .iter()
            .any(|s| matches!(s.key, StatKey::Min | StatKey::Max)),
        "…and never ordered: {tags:?}"
    );

    assert!(
        profile.sql.contains(&format!("FROM {name};")),
        "and 'view as query' hands over the server's own spelling: {}",
        profile.sql
    );
    let rerun = rows(engine, 51, profile.sql.trim_end_matches(';')).await;
    assert_eq!(rerun.len(), 1, "…which runs: {}", profile.sql);

    unsplit_expression_set_fails_on_the_server(engine, &name).await;
}

/// **Why the expression set is split at all**, pinned rather than argued.
///
/// Without this, deleting [`Profiled`] and profiling every relation with the workspace set would
/// leave the whole suite green: the phase above only asserts that the *remote* set works. This
/// asserts the other half — that federation is not a fallback. It sweeps the aggregate into one
/// remote statement or none, and the server has no `approx_percentile_cont`, so the scan fails
/// **whole**: not a missing median, a missing profile.
///
/// The statement is built from the workspace set's own expressions rather than typed out, so it
/// stays the thing profiling would actually have sent.
async fn unsplit_expression_set_fails_on_the_server(engine: &Engine, name: &str) {
    let columns = [column_info(&Field::new("total", DataType::Int32, true))];
    let (exprs, _) = aggregates(&columns, Profiled::Workspace);
    let sql = profile_sql(name, &exprs);

    // Two ways the median is fatal, and only the unparser can say which this build is. If it
    // refuses the expression outright, `profile_sql` answers empty and no federated statement
    // could ever have carried it — the claim is already settled. Otherwise it renders a function
    // name, and the server is what refuses.
    if sql.is_empty() {
        return;
    }
    assert!(
        sql.contains("percentile"),
        "the workspace set is what carries the median: {sql}"
    );

    let why = engine
        .query(
            WsId(1),
            RunTag(52),
            sql.trim_end_matches(';').to_string(),
            200,
        )
        .await
        .expect_err("the workspace set must not survive a trip to PostgreSQL");
    assert!(
        !stopped_on_purpose(&why),
        "a real failure, not a cancel: {why}"
    );
}

/// **The mixed plan** — a **file** table joined onto a remote one; a phase of the test above.
///
/// A real local table, not a `VALUES` list: a `Values` node carries no table provider, so
/// federation finds nothing to disagree with and sweeps the whole join into the remote
/// statement — measured, and the plan then is not mixed at all. What this workstream exists for
/// is cross-joining files onto a live database, so the fixture has to be a file.
async fn mixed_plan(engine: &Engine, dir: &Path) {
    fs::create_dir_all(dir).expect("a fixture folder");
    fs::write(dir.join("tiers.csv"), "customer,tier\n10,gold\n20,silver\n").expect("the fixture");
    engine
        .register(table_spec(
            dir,
            &TableDef {
                name: "tiers".into(),
                format: SourceFormat::Csv(CsvRead::default()),
                connection: None,
                sources: vec!["tiers.csv".into()],
                partition_cols: Vec::new(),
                origin: TableOrigin::External,
            },
            &Connections::default(),
        ))
        .await
        .expect("a local file table");

    let mixed_sql = format!(
        "SELECT t.tier, count(*) FROM {CATALOG}.public.orders o \
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

/// **`jsonb`, a user's own window, `IN (subquery)` and read-only** — the things pinned as they
/// are today; a phase of the test above.
async fn exotic_types_and_refusals(engine: &Engine) {
    assert!(
        engine
            .query(
                WsId(1),
                RunTag(20),
                format!(
                    "SELECT id, row_number() OVER () FROM \
                     (SELECT id FROM {CATALOG}.public.orders)"
                ),
                200,
            )
            .await
            .is_err(),
        "if this starts passing, DataFusion's unparser has been fixed — drop the gap from the \
         workstream README and this comment with it"
    );

    assert_eq!(
        rows(
            engine,
            9,
            &format!("SELECT tags FROM {CATALOG}.public.orders ORDER BY id")
        )
        .await,
        vec![
            vec!["{\"channel\": \"web\"}".to_string()],
            vec!["{\"channel\": \"store\"}".to_string()],
            vec!["{\"channel\": \"web\"}".to_string()],
        ],
        "a jsonb column arrives as JSON text rather than making its table unreadable"
    );

    assert_eq!(
        rows(
            engine,
            11,
            &format!(
                "SELECT id FROM {CATALOG}.public.orders WHERE customer IN \
                 (SELECT id FROM {CATALOG}.public.customers WHERE name = 'globex')"
            )
        )
        .await,
        vec![vec!["3".to_string()]]
    );
    assert!(
        engine
            .query(
                WsId(1),
                RunTag(12),
                format!(
                    "SELECT id IN (SELECT customer FROM {CATALOG}.public.orders) FROM \
                     {CATALOG}.public.customers"
                ),
                200,
            )
            .await
            .is_err(),
        "an InSubquery in projection position has never been plannable"
    );

    let Err(why) = engine
        .run(
            WsId(1),
            RunTag(13),
            format!("INSERT INTO {CATALOG}.public.orders VALUES (4, 30, 1, '{{}}')"),
            200,
        )
        .await
    else {
        panic!("v1 is read-only against a database");
    };
    assert!(!why.is_empty(), "the refusal says something: {why}");
}

/// **JSON accessors over a remote column** (DB-08) — a phase of the test above.
///
/// The rewrite is judged by the *server*, which is the whole reason this lives here: a unit test
/// can pin the operator syntax the mapping table produces (and does, next to the table), but only
/// `PostgreSQL` can say that `->>` over a `jsonb` column means what `json_as_text` means over the
/// text that column arrives as.
///
/// Two spellings here are the **parser's** business rather than this task's, and both are the same
/// before and after it. `->>` has to be parenthesised against a comparison: sqlparser gives every
/// Postgres-style operator `PgOther` precedence (16), which is *looser* than `Eq` (20), so a bare
/// `payload ->> 'type' = 'click'` binds as `payload ->> ('type' = 'click')` and fails type coercion
/// — locally and federated alike. And `json_contains` is written by name rather than as `?`, which
/// is what the planner turns `?` into: under DataFusion's default parser dialect a `?` tokenizes as
/// a placeholder.
async fn json_pushdown(engine: &Engine, dir: &Path) {
    let clicks = format!(
        "SELECT id FROM {CATALOG}.public.events WHERE (payload ->> 'type') = 'click' ORDER BY id"
    );
    assert_eq!(
        rows(engine, 50, &clicks).await,
        vec![vec!["1".to_string()], vec!["3".to_string()]],
        "the headline query answers"
    );

    let plan = explain(engine, 51, &clicks).await;
    let federated: Vec<&str> = plan
        .lines()
        .filter(|line| line.contains("VirtualExecutionPlan"))
        .collect();
    assert_eq!(federated.len(), 1, "the whole read federates:\n{plan}");
    assert!(
        federated[0].contains("json_as_text"),
        "the unparser writes the UDF call, which is what there is to rewrite:\n{}",
        federated[0]
    );
    let Some((_, rewritten)) = federated[0].split_once("rewritten_executor_sql=") else {
        panic!("the rewrite did not run:\n{}", federated[0]);
    };
    assert!(
        rewritten.contains("->>") && !rewritten.contains("json_as_text"),
        "the statement that leaves carries the operator, not the function: {rewritten}"
    );

    assert_eq!(
        rows(
            engine,
            52,
            &format!(
                "SELECT payload -> 'source' ->> 'campaign' FROM {CATALOG}.public.events \
                 WHERE id = 1"
            )
        )
        .await,
        vec![vec!["spring".to_string()]],
        "a chained path walks the object on the server"
    );

    assert_eq!(
        rows(
            engine,
            53,
            &format!(
                "SELECT id FROM {CATALOG}.public.events WHERE json_contains(payload, 'bot') \
                 ORDER BY id"
            )
        )
        .await,
        vec![vec!["3".to_string()]],
        "and a containment test asks it too"
    );

    let Err(why) = engine
        .query(
            WsId(1),
            RunTag(54),
            format!("SELECT payload -> 'type' FROM {CATALOG}.public.events"),
            200,
        )
        .await
    else {
        panic!("'->' returns a union type no PostgreSQL expression produces");
    };
    assert!(
        why.contains("'json_get'")
            && why.contains("'->>'")
            && why.contains(&format!("'{CATALOG}'")),
        "the refusal names the function, the spelling that works and the connection: {why}"
    );

    let Err(why) = engine
        .query(
            WsId(1),
            RunTag(55),
            format!(
                "SELECT id FROM {CATALOG}.public.events WHERE json_get_str(payload, 'type') \
                 = 'click'"
            ),
            200,
        )
        .await
    else {
        panic!("'json_get_str' is NULL for a non-string, where '->>' stringifies one");
    };
    assert!(
        why.contains("'json_get_str'") && why.contains("CREATE TABLE"),
        "an unmapped member names itself and the way out: {why}"
    );
    assert!(
        !why.contains("does not exist"),
        "…and it is Strata's sentence rather than a raw SQLSTATE: {why}"
    );

    fs::create_dir_all(dir).expect("a fixture folder");
    fs::write(
        dir.join("local_events.csv"),
        "id,payload\n1,\"{\"\"type\"\":\"\"click\"\"}\"\n",
    )
    .expect("the fixture");
    engine
        .register(table_spec(
            dir,
            &TableDef {
                name: "local_events".into(),
                format: SourceFormat::Csv(CsvRead::default()),
                connection: None,
                sources: vec!["local_events.csv".into()],
                partition_cols: Vec::new(),
                origin: TableOrigin::External,
            },
            &Connections::default(),
        ))
        .await
        .expect("a local table with a JSON text column");
    assert_eq!(
        rows(
            engine,
            56,
            "SELECT payload ->> 'type', payload -> 'type', json_get_str(payload, 'type'), \
             json_length(payload) FROM local_events"
        )
        .await,
        vec![vec![
            "click".to_string(),
            "\"click\"".to_string(),
            "click".to_string(),
            "1".to_string()
        ]],
        "none of this reaches a local column: the rewrite rides the remote executor only"
    );
}

/// **A reconnect replaces, a rename does not, and a disconnect stops resolving** — a phase of the
/// test above.
///
/// The middle one is the one worth pinning. `Live` is keyed by the connection's **name**, so a
/// rename is a new key rather than a displacement, and it cannot be otherwise: two connections
/// may share an identity and differ only by name, so nothing the engine can see tells a renamed
/// connection from a second one to the same server. Retiring the old catalog is therefore the
/// renaming gesture's own `Engine::disconnect`, which is what the connection editor's Save makes.
///
/// The rename goes through [`migrate_secrets`] rather than storing a second password, because
/// that is what a rename *is* now: the keystore slot is derived from the connection's name, so
/// moving the name moves the entry, and a rename that skipped this funnel would leave the
/// connection unable to log in. Last phase of the test, so the old name's empty slot is nobody's
/// problem afterwards.
async fn reconnect_and_disconnect(engine: &Engine, port: u16) {
    let was = connection(port, CATALOG, &["public"]);
    let renamed = connection(port, "warehouse", &["public"]);
    migrate_secrets(&was, &renamed).expect("this machine's keystore answers");
    engine
        .connect(renamed.clone())
        .await
        .expect("the same connection under a new catalog name");
    assert_eq!(
        rows(engine, 14, "SELECT count(*) FROM warehouse.public.orders").await,
        vec![vec!["3".to_string()]]
    );
    assert!(
        engine
            .query(
                WsId(1),
                RunTag(15),
                format!("SELECT id FROM {CATALOG}.public.orders"),
                200,
            )
            .await
            .is_ok(),
        "the old name is still registered until something retires it: two connections may share \
         an identity, so nothing the engine sees tells a rename from a second connection"
    );

    engine.disconnect(&was.named());
    assert!(
        engine
            .query(
                WsId(1),
                RunTag(16),
                format!("SELECT id FROM {CATALOG}.public.orders"),
                200,
            )
            .await
            .is_err(),
        "and retiring it is the renaming gesture's own call, which is what Save makes"
    );

    engine.disconnect(&renamed.named());
    assert!(
        engine
            .query(
                WsId(1),
                RunTag(17),
                "SELECT id FROM warehouse.public.orders".to_string(),
                200,
            )
            .await
            .is_err(),
        "a forgotten connection's catalog must stop resolving"
    );
    assert!(
        engine.source_listing(&renamed).is_none(),
        "…and it is no longer a live database"
    );
}

/// **The statement policy over a real remote catalog** (DB-03) — a phase of the test above.
///
/// The unit tests drive every intercepted kind against a fake catalog, which is the right place for
/// a checklist. What only a server can show is that the names being refused genuinely *resolve*:
/// against a fake catalog a wrong refusal and a right one both look like an error.
///
/// **The data root has to be set first**, which is the point of writing this against the real entry
/// point: `CREATE EXTERNAL TABLE` refuses a rootless engine before it looks at the target, so that
/// row would otherwise assert nothing about the catalog. (A CTAS no longer does — since DB-10 it
/// resolves the target first, because a remote one needs no project folder at all.)
async fn statement_policy(engine: &Engine, dir: &Path) {
    engine.set_data_dir(dir);
    for sql in [
        format!("DROP TABLE {CATALOG}.public.orders"),
        format!("DROP VIEW {CATALOG}.public.big_orders"),
        format!("CREATE TABLE {CATALOG}.public.mine (id INT)"),
        format!("CREATE VIEW {CATALOG}.public.mine AS SELECT 1 AS id"),
        format!(
            "CREATE EXTERNAL TABLE {CATALOG}.public.mine STORED AS PARQUET LOCATION 'x.parquet'"
        ),
    ] {
        let Err(why) = engine.run(WsId(1), RunTag(21), sql.clone(), 200).await else {
            panic!("'{sql}' was not refused");
        };
        assert!(
            why.contains(&format!("database connection '{CATALOG}'")),
            "'{sql}' must name the connection: {why}"
        );
    }

    for sql in [
        format!("INSERT INTO {CATALOG}.public.orders VALUES (9, 9, 9, NULL)"),
        format!("CREATE TABLE {CATALOG}.public.mine AS SELECT 1 AS id"),
    ] {
        let Err(why) = engine.run(WsId(1), RunTag(25), sql.clone(), 200).await else {
            panic!("'{sql}' was not refused");
        };
        assert!(
            why.contains("read-only") && why.contains("Read only"),
            "'{sql}' must name the setting that would allow it: {why}"
        );
    }

    assert_eq!(
        rows(
            engine,
            22,
            &format!("SELECT count(*) FROM {CATALOG}.public.orders")
        )
        .await,
        vec![vec!["3".to_string()]]
    );

    let Err(why) = engine
        .run(
            WsId(1),
            RunTag(23),
            "SELECT * FROM __snap_1".to_string(),
            200,
        )
        .await
    else {
        panic!("the workspace's snapshot namespace is reserved");
    };
    assert!(why.contains("__snap_"), "{why}");
    let Err(why) = engine
        .run(
            WsId(1),
            RunTag(24),
            format!("SELECT * FROM {CATALOG}.public.__snap_1"),
            200,
        )
        .await
    else {
        panic!("the server has no such relation");
    };
    assert!(
        !why.contains("reserved"),
        "a remote relation is not in Strata's reserved namespace: {why}"
    );
}

/// **Unqualified names** (DB-09) — a phase of the test above, and the half a fake catalog cannot
/// show: whether a bare name genuinely *reaches* the server's relation.
///
/// Runs after [`statement_policy`], which set the data root a workspace table needs, and leaves
/// the fixture as it found it.
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
            .run(
                WsId(1),
                RunTag(42),
                "SELECT * FROM sessions".to_string(),
                200,
            )
            .await
            .is_err(),
        "a schema the connection does not show must not capture a bare name"
    );
    assert_eq!(
        rows(
            engine,
            43,
            &format!("SELECT count(*) FROM {CATALOG}.analytics.sessions")
        )
        .await,
        vec![vec!["2".to_string()]],
        "…and writing it in full still resolves, which is the half that scoping never bounded"
    );

    engine.show_schemas(&connection(port, CATALOG, &["public", "analytics"]));
    assert_eq!(
        rows(engine, 44, "SELECT count(*) FROM sessions").await,
        vec![vec!["2".to_string()]],
        "showing the schema is what puts it in reach of a bare name"
    );
    engine.show_schemas(&connection(port, CATALOG, &["public"]));
    assert!(
        engine
            .run(
                WsId(1),
                RunTag(45),
                "SELECT * FROM sessions".to_string(),
                200,
            )
            .await
            .is_err(),
        "and hiding it again takes it back out"
    );

    let Err(why) = engine
        .run(
            WsId(1),
            RunTag(43),
            "INSERT INTO customers VALUES (30, 'x')".to_string(),
            200,
        )
        .await
    else {
        panic!("a write to a bare name that resolves remote was accepted");
    };
    assert!(
        why.contains(&format!("database connection '{CATALOG}'")),
        "a write target is refused as remote, not as missing: {why}"
    );

    engine
        .run(
            WsId(1),
            RunTag(44),
            "CREATE VIEW remote_orders AS SELECT id, total FROM orders".to_string(),
            200,
        )
        .await
        .expect("the view is created over the resolved name");
    engine
        .run(
            WsId(1),
            RunTag(45),
            "CREATE TABLE orders AS SELECT 1 AS id, 1 AS total".to_string(),
            200,
        )
        .await
        .expect("a workspace table may take a name the database also has");
    assert_eq!(
        rows(engine, 46, "SELECT count(*) FROM orders").await,
        vec![vec!["1".to_string()]],
        "and from then on the workspace's own table is what the bare name means"
    );
    assert_eq!(
        rows(
            engine,
            47,
            &format!("SELECT count(*) FROM {CATALOG}.public.orders")
        )
        .await,
        vec![vec!["3".to_string()]],
        "the qualified name still reaches across"
    );

    let Ok(RunOutcome::Statement(report)) = engine
        .run(WsId(1), RunTag(48), "DROP TABLE orders".to_string(), 200)
        .await
    else {
        panic!("the workspace table drops");
    };
    assert!(
        !report.message.contains("remote_orders"),
        "dropping a same-named workspace table must not name a view that never read it: {}",
        report.message
    );
    engine
        .run(
            WsId(1),
            RunTag(49),
            "DROP VIEW remote_orders".to_string(),
            200,
        )
        .await
        .expect("the view drops");

    ambiguous_names(engine, port).await;
}

/// One name in two schemas of one database: refused by name, with both addresses in the sentence.
///
/// **Both schemas are shown**, which is what makes this a tie at all. Its own function because it
/// moves the fixture — a relation added server-side is only visible after a reconnect, and the
/// phase has to put it back.
async fn ambiguous_names(engine: &Engine, port: u16) {
    let (client, driver) = tokio_postgres::connect(
        &format!("host=127.0.0.1 port={port} user={USER} password={PASSWORD} dbname={DATABASE}"),
        tokio_postgres::NoTls,
    )
    .await
    .expect("a raw client to add a second 'orders'");
    tokio::spawn(async move {
        if let Err(e) = driver.await {
            eprintln!("fixture connection ended: {e}");
        }
    });
    let conn = connection(port, CATALOG, &["public", "analytics"]);

    client
        .batch_execute("CREATE TABLE analytics.orders (id INT PRIMARY KEY);")
        .await
        .expect("a second relation of the same name");
    engine.connect(conn.clone()).await.expect("re-enumerates");

    let Err(why) = engine
        .run(WsId(1), RunTag(50), "SELECT * FROM orders".to_string(), 200)
        .await
    else {
        panic!("two relations of that name and one of them was picked");
    };
    assert!(
        why.contains(&format!("{CATALOG}.public.orders"))
            && why.contains(&format!("{CATALOG}.analytics.orders")),
        "the refusal names every candidate: {why}"
    );

    assert_eq!(
        rows(
            engine,
            51,
            &format!("SELECT count(*) FROM {CATALOG}.analytics.orders")
        )
        .await,
        vec![vec!["0".to_string()]],
        "and qualifying it is the fix the message asks for"
    );

    engine.show_schemas(&connection(port, CATALOG, &["public"]));
    assert_eq!(
        rows(engine, 52, "SELECT count(*) FROM orders").await,
        vec![vec!["3".to_string()]],
        "with `analytics` hidden the name has one candidate again"
    );

    client
        .batch_execute("DROP TABLE analytics.orders;")
        .await
        .expect("put the fixture back");
    engine
        .connect(connection(port, CATALOG, &["public"]))
        .await
        .expect("re-enumerates");
}

/// **Writing into a database** (DB-10) — a phase of the test above, and the one no fake catalog
/// can stand in for: an insert is only real once a server has taken it.
///
/// Opting in is a **re-connect with the toggle off**, which is exactly what the connection editor's
/// Save does, so nothing here reaches past the def to arrange it.
async fn remote_writes(engine: &Engine, port: u16) {
    let conn = writable(port, CATALOG, &["public"]);
    engine
        .connect(conn.clone())
        .await
        .expect("the same connection, opted in to writes");

    engine
        .run(
            WsId(1),
            RunTag(60),
            "CREATE TABLE loaders AS SELECT * FROM (VALUES (10, 'gold'), (20, 'silver')) \
             AS t(customer, tier)"
                .to_string(),
            200,
        )
        .await
        .expect("a workspace table to join across");

    let RunOutcome::Statement(report) = engine
        .run(
            WsId(1),
            RunTag(61),
            format!(
                "CREATE TABLE {CATALOG}.public.loaded AS SELECT t.tier, o.total \
                 FROM loaders t JOIN {CATALOG}.public.orders o ON t.customer = o.customer"
            ),
            200,
        )
        .await
        .expect("a cross-source result materializes as a server table")
    else {
        panic!("CREATE TABLE AS ran as a query");
    };
    assert_eq!(report.message, "Table 'pg.public.loaded' created, 3 rows");
    assert_eq!(report.count, Some(3));
    assert_eq!(report.effect, Some(StoreEffect::RemoteRelationsChanged));

    let (_, listing) = engine
        .source_listing(&conn)
        .expect("a live database has a listing");
    assert!(
        listing
            .iter()
            .find(|schema| schema.name == "public")
            .is_some_and(|schema| schema.relations.iter().any(|r| r.name == "loaded")),
        "the tree and completion see it with no manual refresh"
    );
    assert_eq!(
        rows(
            engine,
            62,
            &format!("SELECT count(*) FROM {CATALOG}.public.loaded")
        )
        .await,
        vec![vec!["3".to_string()]],
        "and the rows are on the server"
    );

    inserts_land(engine).await;
    ctas_name_semantics(engine).await;
    failed_ctas_leaves_nothing(engine, &conn).await;
    cancelled_ctas_leaves_nothing(engine, port).await;
    agent_stays_read_only(engine).await;

    engine
        .run(WsId(1), RunTag(78), "DROP TABLE loaders".to_string(), 200)
        .await
        .expect("put the workspace back");
    engine
        .connect(connection(port, CATALOG, &["public"]))
        .await
        .expect("and the connection back to read-only");
    let Err(why) = engine
        .run(
            WsId(1),
            RunTag(79),
            format!("INSERT INTO {CATALOG}.public.loaded VALUES ('after', 1)"),
            200,
        )
        .await
    else {
        panic!("the toggle is what allows the write, and it is off again");
    };
    assert!(why.contains("read-only"), "{why}");
}

/// **The statements the server runs** — a phase of the test above, and the one only a server can
/// settle: a spliced statement is either `PostgreSQL`'s own SQL or it is a syntax error.
///
/// The unit tests next door pin the rewrite byte for byte; what is here is that the rewritten text
/// parses, does what it says, and leaves the app's view of the database correct afterwards.
async fn remote_statements(engine: &Engine, port: u16) {
    let conn = writable(port, CATALOG, &["public"]);
    engine
        .connect(conn.clone())
        .await
        .expect("opted in to writes");

    let RunOutcome::Statement(report) = engine
        .run(
            WsId(1),
            RunTag(90),
            format!(
                "CREATE VIEW {CATALOG}.public.barrier_view WITH (security_barrier = true) AS \
                 SELECT id, total FROM {CATALOG}.public.orders WHERE total > 0"
            ),
            200,
        )
        .await
        .expect("the server takes a view Strata cannot model")
    else {
        panic!("CREATE VIEW ran as a query");
    };
    assert_eq!(report.message, "View 'public.barrier_view' created on 'pg'");
    assert_eq!(report.count, None);
    assert_eq!(report.effect, Some(StoreEffect::RemoteRelationsChanged));

    let (_, listing) = engine.source_listing(&conn).expect("a live listing");
    assert!(
        listing
            .iter()
            .find(|schema| schema.name == "public")
            .is_some_and(|schema| schema
                .relations
                .iter()
                .any(|r| r.name == "barrier_view" && r.view)),
        "the tree sees the view, as a view, with no manual refresh"
    );
    assert!(
        !rows(
            engine,
            91,
            &format!("SELECT id FROM {CATALOG}.public.barrier_view")
        )
        .await
        .is_empty(),
        "and it reads"
    );

    clause_fidelity_survives_dispatch(port).await;
    server_typed_columns(engine).await;
    remote_dml_reports_the_servers_count(engine).await;
    workspace_dml_says_where_it_works(engine).await;
    a_remote_drop_names_its_readers(engine, port).await;
    remote_bodies_stay_inside_the_connection(engine).await;
    remote_statements_stay_refused_to_an_agent(engine).await;

    engine
        .connect(connection(port, CATALOG, &["public"]))
        .await
        .expect("the connection back to read-only");
    let Err(why) = engine
        .run(
            WsId(1),
            RunTag(99),
            format!("DROP VIEW {CATALOG}.public.barrier_view"),
            200,
        )
        .await
    else {
        panic!("the toggle is what allows the statement, and it is off again");
    };
    assert!(why.contains("read-only"), "{why}");
}

/// **The clause Strata does not model reaches the server intact** — the whole claim of splicing
/// the buffer rather than re-rendering a parsed statement, asserted where only the server can
/// answer: in its own catalog.
async fn clause_fidelity_survives_dispatch(port: u16) {
    let client = raw_client(port, "a raw client to read the server's own catalog").await;
    let options: Option<Vec<String>> = client
        .query_one(
            "SELECT reloptions FROM pg_catalog.pg_class WHERE relname = 'barrier_view'",
            &[],
        )
        .await
        .expect("the view is there")
        .get(0);
    assert_eq!(
        options,
        Some(vec!["security_barrier=true".to_string()]),
        "the storage parameter travelled verbatim"
    );
}

/// **A column list in the server's own type vocabulary**, which is the half DataFusion cannot
/// plan: `jsonb` has no Arrow mapping, so a statement asking to be planned would be refused before
/// anything reached the connection.
async fn server_typed_columns(engine: &Engine) {
    let RunOutcome::Statement(report) = engine
        .run(
            WsId(1),
            RunTag(92),
            format!(
                "CREATE TABLE {CATALOG}.public.typed (id INT, payload jsonb, made timestamptz)"
            ),
            200,
        )
        .await
        .expect("the server judges its own types")
    else {
        panic!("CREATE TABLE ran as a query");
    };
    assert_eq!(report.message, "Table 'public.typed' created on 'pg'");

    let described = engine
        .describe_remote(format!("{CATALOG}.public.typed"))
        .await
        .expect("the new relation describes")
        .expect("and the listing already has it");
    assert_eq!(
        described
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "payload", "made"],
        "and the app sees it with no manual refresh"
    );
}

/// **`UPDATE` and `DELETE` report the server's own affected-row count**, confirmed by read-back —
/// the one number nothing on this side could have computed.
async fn remote_dml_reports_the_servers_count(engine: &Engine) {
    for (tag, sql) in [
        (
            93,
            format!("INSERT INTO {CATALOG}.public.typed (id) VALUES (1), (2), (3)"),
        ),
        (
            94,
            format!("UPDATE {CATALOG}.public.typed SET id = id + 10 WHERE id > 1"),
        ),
    ] {
        engine
            .run(WsId(1), RunTag(tag), sql.clone(), 200)
            .await
            .unwrap_or_else(|e| panic!("'{sql}': {e}"));
    }

    let RunOutcome::Statement(report) = engine
        .run(
            WsId(1),
            RunTag(95),
            format!("UPDATE {CATALOG}.public.typed SET id = 0 WHERE id > 5"),
            200,
        )
        .await
        .expect("updated")
    else {
        panic!("UPDATE ran as a query");
    };
    assert_eq!(report.message, "Updated 2 rows in 'public.typed' on 'pg'");
    assert_eq!(report.count, Some(2));
    assert_eq!(report.effect, None, "rows are not relations");

    let RunOutcome::Statement(report) = engine
        .run(
            WsId(1),
            RunTag(96),
            format!("DELETE FROM {CATALOG}.public.typed WHERE id = 0"),
            200,
        )
        .await
        .expect("deleted")
    else {
        panic!("DELETE ran as a query");
    };
    assert_eq!(report.message, "Deleted 2 rows from 'public.typed' on 'pg'");
    assert_eq!(report.count, Some(2));
    assert_eq!(
        rows(
            engine,
            97,
            &format!("SELECT count(*) FROM {CATALOG}.public.typed")
        )
        .await,
        vec![vec!["1".to_string()]],
        "the server's count is the truth, and the read-back agrees"
    );
}

/// A workspace table is refused in its own words rather than as an unsupported statement, because
/// the same verb works one qualifier away.
async fn workspace_dml_says_where_it_works(engine: &Engine) {
    engine
        .run(
            WsId(1),
            RunTag(100),
            "CREATE TABLE local_rows AS SELECT 1 AS n".to_string(),
            200,
        )
        .await
        .expect("a workspace table");
    for sql in [
        "UPDATE local_rows SET n = 2",
        "DELETE FROM local_rows WHERE n = 1",
    ] {
        let Err(why) = engine.run(WsId(1), RunTag(101), sql.to_string(), 200).await else {
            panic!("'{sql}' is not something a workspace table can take");
        };
        assert!(
            why.contains("database connection") && why.contains("CREATE TABLE AS"),
            "'{sql}': {why}"
        );
    }
    engine
        .run(
            WsId(1),
            RunTag(102),
            "DROP TABLE local_rows".to_string(),
            200,
        )
        .await
        .expect("put the workspace back");
}

/// A remote `DROP` names the workspace views left invalid without cascading, and the relation
/// stops answering — the cached provider goes with the listing, so a re-query gets the
/// reconciliation's sentence rather than rows.
async fn a_remote_drop_names_its_readers(engine: &Engine, port: u16) {
    engine
        .create_view(
            "over_typed".to_string(),
            format!("SELECT id FROM {CATALOG}.public.typed"),
        )
        .await
        .expect("a workspace view over the remote table");

    let RunOutcome::Statement(report) = engine
        .run(
            WsId(1),
            RunTag(103),
            format!("DROP TABLE {CATALOG}.public.typed"),
            200,
        )
        .await
        .expect("dropped")
    else {
        panic!("DROP TABLE ran as a query");
    };
    assert_eq!(
        report.message,
        "Table 'public.typed' dropped on 'pg'. 1 view is left invalid: 'over_typed'"
    );
    assert_eq!(report.effect, Some(StoreEffect::RemoteRelationsChanged));

    let conn = writable(port, CATALOG, &["public"]);
    let (_, listing) = engine.source_listing(&conn).expect("a live listing");
    assert!(
        listing
            .iter()
            .find(|schema| schema.name == "public")
            .is_some_and(|schema| !schema.relations.iter().any(|r| r.name == "typed")),
        "the tree lost it with no manual refresh"
    );
    assert!(
        engine
            .query(
                WsId(1),
                RunTag(104),
                format!("SELECT id FROM {CATALOG}.public.typed"),
                200,
            )
            .await
            .is_err(),
        "and the cached provider went with it, so nothing answers for the relation"
    );
    engine
        .drop_view("over_typed".to_string())
        .await
        .expect("put the workspace back");
}

/// A statement that runs on the server may only name that server's relations, refused **by name**
/// otherwise — a workspace table, and a name left bare because nothing in the connection has it.
async fn remote_bodies_stay_inside_the_connection(engine: &Engine) {
    for (sql, named) in [
        (
            format!(
                "CREATE VIEW {CATALOG}.public.crossed AS SELECT n FROM (SELECT 1 AS n) t \
                 WHERE n IN (SELECT id FROM missing_everywhere)"
            ),
            "missing_everywhere",
        ),
        (
            format!("CREATE VIEW {CATALOG}.public.crossed AS SELECT id FROM public.orders"),
            "public.orders",
        ),
    ] {
        let Err(why) = engine.run(WsId(1), RunTag(105), sql.clone(), 200).await else {
            panic!("'{sql}' reaches outside the connection");
        };
        assert!(why.contains(named), "'{sql}': {why}");
    }
}

/// The agent surface is unmoved: every statement this phase runs is refused to it.
async fn remote_statements_stay_refused_to_an_agent(engine: &Engine) {
    for sql in [
        format!("CREATE VIEW {CATALOG}.public.agent_view AS SELECT 1 AS n"),
        format!("DROP TABLE {CATALOG}.public.barrier_view"),
        format!("UPDATE {CATALOG}.public.orders SET total = 0"),
        format!("DELETE FROM {CATALOG}.public.orders"),
    ] {
        let refusals = engine
            .policy_verdicts(sql.clone())
            .await
            .unwrap_or_else(|e| panic!("'{sql}': {e}"));
        assert_eq!(refusals.len(), 1, "'{sql}' is refused to an agent");
    }
}

/// A raw driver connection to the fixture, held by the caller's task for as long as it is used.
async fn raw_client(port: u16, why: &str) -> tokio_postgres::Client {
    let (client, driver) = tokio_postgres::connect(
        &format!("host=127.0.0.1 port={port} user={USER} password={PASSWORD} dbname={DATABASE}"),
        tokio_postgres::NoTls,
    )
    .await
    .unwrap_or_else(|e| panic!("{why}: {e}"));
    tokio::spawn(async move {
        if let Err(e) = driver.await {
            eprintln!("fixture connection ended: {e}");
        }
    });
    client
}

/// **Every shape of `INSERT` a remote target can take**: literal rows, a local source, a remote
/// one, and a **bare** name that only the connection has — which is DB-10's other half, the write
/// target resolving exactly as a read does.
///
/// One loop rather than four blocks, because what is under test is that they answer identically:
/// the same wording, the same count, and no effect, since an insert changes no listing.
async fn inserts_land(engine: &Engine) {
    for (tag, sql, count) in [
        (
            63,
            format!("INSERT INTO {CATALOG}.public.loaded VALUES ('bronze', 1)"),
            1u64,
        ),
        (
            64,
            format!("INSERT INTO {CATALOG}.public.loaded SELECT tier, 0 FROM loaders"),
            2,
        ),
        (
            65,
            format!(
                "INSERT INTO {CATALOG}.public.loaded SELECT name, id FROM \
                 {CATALOG}.public.customers"
            ),
            2,
        ),
        (66, "INSERT INTO loaded VALUES ('bare', 9)".to_string(), 1),
    ] {
        let RunOutcome::Statement(report) = engine
            .run(WsId(1), RunTag(tag), sql.clone(), 200)
            .await
            .unwrap_or_else(|e| panic!("'{sql}': {e}"))
        else {
            panic!("'{sql}' ran as a query");
        };
        assert_eq!(report.count, Some(count), "'{sql}'");
        assert_eq!(
            report.message,
            format!(
                "Inserted {count} row{} into 'pg.public.loaded'",
                if count == 1 { "" } else { "s" }
            ),
            "a bare target reports the address it reached"
        );
        assert_eq!(report.effect, None, "an INSERT changes no listing");
    }
    assert_eq!(
        rows(
            engine,
            67,
            &format!("SELECT count(*) FROM {CATALOG}.public.loaded")
        )
        .await,
        vec![vec!["9".to_string()]],
        "every insert landed, the bare-name one included"
    );

    let Err(why) = engine
        .run(
            WsId(1),
            RunTag(68),
            format!("INSERT OVERWRITE INTO {CATALOG}.public.loaded VALUES ('x', 1)"),
            200,
        )
        .await
    else {
        panic!("a statement that empties a server table is not v1");
    };
    assert!(why.contains("replaces rows"), "{why}");
}

/// **A remote CTAS answers about a name the way the local one does**, against the server as it is
/// now rather than against the connect-time enumeration — with `OR REPLACE` refused, because it
/// would drop a server table.
async fn ctas_name_semantics(engine: &Engine) {
    let Err(why) = engine
        .run(
            WsId(1),
            RunTag(69),
            format!("CREATE TABLE {CATALOG}.public.loaded AS SELECT 1 AS n"),
            200,
        )
        .await
    else {
        panic!("the relation is already there");
    };
    assert_eq!(why, "Table 'pg.public.loaded' already exists");

    let RunOutcome::Statement(report) = engine
        .run(
            WsId(1),
            RunTag(70),
            format!("CREATE TABLE IF NOT EXISTS {CATALOG}.public.loaded AS SELECT 1 AS n"),
            200,
        )
        .await
        .expect("reported rather than refused")
    else {
        panic!("ran as a query");
    };
    assert_eq!(report.message, "Table 'pg.public.loaded' already exists");
    assert_eq!(report.effect, None, "and nothing changed");

    let Err(why) = engine
        .run(
            WsId(1),
            RunTag(71),
            format!("CREATE OR REPLACE TABLE {CATALOG}.public.loaded AS SELECT 1 AS n"),
            200,
        )
        .await
    else {
        panic!("replacing a server table is not v1 either");
    };
    assert!(why.contains("Drop it on the server first"), "{why}");

    let RunOutcome::Statement(report) = engine
        .run(
            WsId(1),
            RunTag(74),
            format!("CREATE OR REPLACE TABLE {CATALOG}.public.replaceable AS SELECT 1 AS n"),
            200,
        )
        .await
        .expect("over a free name there is nothing to replace, so it creates")
    else {
        panic!("ran as a query");
    };
    assert_eq!(
        report.message,
        "Table 'pg.public.replaceable' created, 1 row"
    );
}

/// **A CTAS whose insert fails leaves no table behind.** The create lands, the fill does not, and
/// the rollback takes the relation back off the server — otherwise the user is left with an empty
/// table under a name they believe holds their result.
///
/// The failure is a cast the *values* refuse: the plan's schema is `Int32`, so the server table is
/// created with an `INTEGER` column, and 'acme' only fails once rows are actually moving. A
/// literal would have been folded away at planning, before anything was created.
async fn failed_ctas_leaves_nothing(engine: &Engine, conn: &ConnectionDef) {
    let Err(why) = engine
        .run(
            WsId(1),
            RunTag(72),
            format!(
                "CREATE TABLE {CATALOG}.public.doomed AS \
                 SELECT CAST(name AS INT) AS n FROM {CATALOG}.public.customers"
            ),
            200,
        )
        .await
    else {
        panic!("'acme' is not an integer");
    };
    assert!(!why.contains("already exists"), "{why}");

    let (_, listing) = engine.source_listing(conn).expect("still live");
    assert!(
        listing
            .iter()
            .find(|schema| schema.name == "public")
            .is_some_and(|schema| schema.relations.iter().all(|r| r.name != "doomed")),
        "the half-made table went with the failure"
    );
    assert!(
        engine
            .query(
                WsId(1),
                RunTag(73),
                format!("SELECT n FROM {CATALOG}.public.doomed"),
                200,
            )
            .await
            .is_err(),
        "…and nothing resolves under its name"
    );
}

/// **A cancelled CTAS takes its table with it.** A cancel aborts the task, so the future is
/// *dropped* mid-fill and no error path runs — without `write::Created`'s guard the server would
/// keep an empty table under the name the user chose, and the retry would then refuse it as
/// already existing. The local half has the same guard and the same test
/// (`a_cancelled_spool_takes_its_staging_directory_with_it`).
///
/// The fill is deliberately slow — `InsertBuilder` renders every row as a literal, so a hundred
/// thousand of them take seconds over TCP — and the create is one statement, so the cancel lands
/// between them. The rollback is *spawned* by the guard's `Drop`, so the assertion waits for it
/// rather than reading once.
async fn cancelled_ctas_leaves_nothing(engine: &Engine, port: u16) {
    let (client, driver) = tokio_postgres::connect(
        &format!("host=127.0.0.1 port={port} user={USER} password={PASSWORD} dbname={DATABASE}"),
        tokio_postgres::NoTls,
    )
    .await
    .expect("a raw client to watch the server directly");
    tokio::spawn(async move {
        if let Err(e) = driver.await {
            eprintln!("cancel-watch connection ended: {e}");
        }
    });

    let running = engine.run(
        WsId(9),
        RunTag(80),
        format!(
            "CREATE TABLE {CATALOG}.public.abandoned AS \
             SELECT value AS n FROM generate_series(1, 100000)"
        ),
        200,
    );
    let cancelling = async {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !relation_exists(&client, "abandoned").await {
            assert!(
                Instant::now() < deadline,
                "the CTAS never created its table"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        engine.cancel(WsId(9), RunTag(80))
    };
    let (settled, cancelled) = tokio::join!(running, cancelling);
    assert!(
        cancelled.is_some(),
        "the CTAS is the workspace's in-flight call"
    );
    let Err(why) = settled else {
        panic!("a cancelled run does not report success");
    };
    assert!(stopped_on_purpose(&why), "{why}");

    let deadline = Instant::now() + Duration::from_secs(30);
    while relation_exists(&client, "abandoned").await {
        assert!(
            Instant::now() < deadline,
            "the cancelled CTAS left its table on the server"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// **Loading a remote relation into a workspace table** (DB-12) — the fourth direction, and a
/// phase of the test above.
///
/// Only a server can reach the fault: the plan's every scan belongs to the connection, so
/// `datafusion-federation` swept the whole plan up including the node that *writes*, and the
/// unparser was then asked for SQL a write has no spelling in. A fake catalog federates nothing and
/// would pass either way.
///
/// **All three writing statements are here**, because the fault was never the `INSERT`'s: a CTAS
/// spooling a remote query and a typed `COPY` reading one carry a `CopyTo` at the same root and
/// failed the same way — the task's premise that a CTAS was "the working spelling" was wrong, and
/// this phase is what says so.
///
/// The connection is **read-only** here — [`remote_writes`] put it back — because pulling rows in
/// is a read of the database and must need no opt-in.
///
/// The target's columns are deliberately named apart from the source's, which is what makes the
/// planner stack its renaming projection on the query's own — the shape the unparser renders as a
/// derived table whose outer references still carry the scan's qualifier.
async fn remote_source_into_a_workspace_table(engine: &Engine, dir: &Path) {
    engine
        .run(
            WsId(1),
            RunTag(90),
            format!(
                "CREATE TABLE local_customers AS SELECT id AS customer_id, name AS customer_name \
                 FROM {CATALOG}.public.customers WHERE false"
            ),
            200,
        )
        .await
        .expect("an empty internal table carrying the remote relation's types");

    let RunOutcome::Statement(report) = engine
        .run(
            WsId(1),
            RunTag(91),
            format!("INSERT INTO local_customers SELECT id, name FROM {CATALOG}.public.customers"),
            200,
        )
        .await
        .expect("a remote source lands in a workspace table")
    else {
        panic!("INSERT ran as a query");
    };
    assert_eq!(report.message, "Inserted 2 rows into 'local_customers'");
    assert_eq!(report.count, Some(2));
    assert_eq!(
        report.effect,
        Some(StoreEffect::RescanTable {
            name: "local_customers".into()
        }),
        "the row count is still the scan driver's to re-read"
    );
    assert_eq!(
        rows(
            engine,
            92,
            "SELECT customer_name FROM local_customers ORDER BY customer_id"
        )
        .await,
        vec![vec!["acme".to_string()], vec!["globex".to_string()]],
        "and the rows read back out of the project's own files"
    );

    engine
        .run(
            WsId(1),
            RunTag(93),
            format!(
                "CREATE TABLE local_spanning AS SELECT t.tier, o.total FROM tiers t \
                 JOIN {CATALOG}.public.orders o ON t.customer = o.customer WHERE false"
            ),
            200,
        )
        .await
        .expect("a table for the cross-source half");

    let RunOutcome::Statement(report) = engine
        .run(
            WsId(1),
            RunTag(94),
            format!(
                "INSERT INTO local_spanning SELECT t.tier, o.total FROM tiers t \
                 JOIN {CATALOG}.public.orders o ON t.customer = o.customer"
            ),
            200,
        )
        .await
        .expect("a file joined onto the database lands too")
    else {
        panic!("INSERT ran as a query");
    };
    assert_eq!(report.message, "Inserted 3 rows into 'local_spanning'");
    assert_eq!(
        rows(engine, 95, "SELECT count(*) FROM local_spanning").await,
        vec![vec!["3".to_string()]],
        "only the remote side federated, and every row still arrived"
    );

    let RunOutcome::Statement(report) = engine
        .run(
            WsId(1),
            RunTag(96),
            format!("CREATE TABLE local_orders AS SELECT id, total FROM {CATALOG}.public.orders"),
            200,
        )
        .await
        .expect("a CTAS reads the connection and spools the result")
    else {
        panic!("CREATE TABLE AS ran as a query");
    };
    assert_eq!(report.message, "Table 'local_orders' created, 3 rows");

    let out = dir.join("remote_copy.parquet");
    engine
        .run(
            WsId(1),
            RunTag(97),
            format!(
                "COPY (SELECT id FROM {CATALOG}.public.orders) TO '{}'",
                out.display()
            ),
            200,
        )
        .await
        .expect("and a typed COPY may take its source from one");
    assert!(out.exists(), "the export wrote its file");

    for sql in [
        "DROP TABLE local_customers",
        "DROP TABLE local_spanning",
        "DROP TABLE local_orders",
    ] {
        engine
            .run(WsId(1), RunTag(98), sql.to_string(), 200)
            .await
            .unwrap_or_else(|e| panic!("'{sql}': {e}"));
    }
}

/// Whether `public.<name>` is on the server right now, asked outside the engine entirely.
async fn relation_exists(client: &tokio_postgres::Client, name: &str) -> bool {
    client
        .query_one(
            "SELECT to_regclass($1) IS NOT NULL",
            &[&format!("public.{name}")],
        )
        .await
        .expect("ask the server")
        .get(0)
}

/// **The agent is still read-only**, with a writable connection registered and the two write
/// statements otherwise working. The parity matrix in `sql::validate` pins the classification;
/// what this pins is that no part of DB-10 reached past it.
async fn agent_stays_read_only(engine: &Engine) {
    for sql in [
        format!("INSERT INTO {CATALOG}.public.loaded VALUES ('agent', 1)"),
        format!("CREATE TABLE {CATALOG}.public.agent_made AS SELECT 1 AS n"),
    ] {
        let refusals = engine
            .policy_verdicts(sql.clone())
            .await
            .unwrap_or_else(|e| panic!("'{sql}': {e}"));
        assert_eq!(refusals.len(), 1, "'{sql}' is refused to an agent");
    }
}

/// **The cross-source view** — the load-bearing case: one workspace def whose dependencies span
/// a file and a database. A phase of the test above.
///
/// Driven on a **second engine** through the real registration pass, because three of the four
/// things under test are about replay: dropping the local table names the view as a dependent; its
/// recorded dependencies carry the remote name qualified and the workspace half bare, where
/// recording by bare component would make both read as this project's tables; it re-registers
/// *after* the connection, which is why connections are the pass's first phase; and with the remote
/// half taken away server-side it settles `Failed` naming the connection. Nothing observes that
/// removal, so the reconciliation is the next pass.
async fn cross_source_views(port: u16, dir: &Path) {
    let (client, driver) = tokio_postgres::connect(
        &format!("host=127.0.0.1 port={port} user={USER} password={PASSWORD} dbname={DATABASE}"),
        tokio_postgres::NoTls,
    )
    .await
    .expect("a raw client to move the fixture under the app's feet");
    tokio::spawn(async move {
        if let Err(e) = driver.await {
            eprintln!("fixture connection ended: {e}");
        }
    });
    client
        .batch_execute("CREATE TABLE public.transient (id INT PRIMARY KEY);")
        .await
        .expect("a relation to take away");

    let defs = ProjectDefs {
        connections: vec![connection(port, CATALOG, &["public"])],
        tables: vec![TableDef {
            name: "tiers".into(),
            format: SourceFormat::Csv(CsvRead::default()),
            connection: None,
            sources: vec!["tiers.csv".into()],
            partition_cols: Vec::new(),
            origin: TableOrigin::External,
        }],
        views: vec![
            ViewDef {
                name: "spanning".into(),
                sql: format!(
                    "SELECT t.tier, o.total FROM tiers t JOIN {CATALOG}.public.orders o \
                     ON t.customer = o.customer"
                ),
            },
            ViewDef {
                name: "over_transient".into(),
                sql: format!("SELECT id FROM {CATALOG}.public.transient"),
            },
        ],
        ..ProjectDefs::default()
    };

    let engine = Engine::builder().build();
    let outcomes = replay(&engine, dir, &defs).await;

    let spanning = view_meta(&outcomes, "spanning").expect("the cross-source view re-registers");
    assert_eq!(spanning.tables, vec!["tiers".to_string()]);
    assert_eq!(
        spanning.remote,
        vec![format!("{CATALOG}.public.orders")],
        "the remote half is recorded whole"
    );

    let Ok(dropped) = engine
        .run(WsId(1), RunTag(30), "DROP TABLE tiers".to_string(), 200)
        .await
    else {
        panic!("the workspace table drops");
    };
    let RunOutcome::Statement(report) = dropped else {
        panic!("DROP TABLE ran as a query");
    };
    assert!(
        report.message.contains("'spanning'"),
        "the cross-source view is a dependent of its file half: {}",
        report.message
    );

    client
        .batch_execute("DROP TABLE public.transient;")
        .await
        .expect("take the relation away");
    let engine = Engine::builder().build();
    let outcomes = replay(&engine, dir, &defs).await;
    let why = view_error(&outcomes, "over_transient").expect("the view can no longer plan");
    assert!(
        why.contains(&format!("{CATALOG}.public.transient"))
            && why.contains(&format!("database connection '{CATALOG}'"))
            && why.contains("Refresh the catalog"),
        "the row names the relation, the connection and the fix: {why}"
    );
}

/// One whole-project registration pass, collected.
async fn replay(engine: &Engine, root: &Path, defs: &ProjectDefs) -> Vec<RegOutcome> {
    let mut out = Vec::new();
    register_project(engine, root, defs, |o| out.push(o)).await;
    out
}

/// What the pass answered for the view `name`.
fn view_meta<'a>(outcomes: &'a [RegOutcome], name: &str) -> Option<&'a ViewMeta> {
    outcomes.iter().find_map(|o| match o {
        RegOutcome::View { name: n, result } if n == name => result.as_ref().ok(),
        _ => None,
    })
}

fn view_error<'a>(outcomes: &'a [RegOutcome], name: &str) -> Option<&'a String> {
    outcomes.iter().find_map(|o| match o {
        RegOutcome::View { name: n, result } if n == name => result.as_ref().err(),
        _ => None,
    })
}
