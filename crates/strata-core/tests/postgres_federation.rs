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

use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::Path;
use std::sync::Once;
use std::time::{Duration, Instant};
use std::{env, fs, process};

use keyring_core::mock;
use strata_core::engine::db::{SchemaVisibility, PG_PASSWORD};
use strata_core::engine::{sql, Engine, RunOutcome, RunTag, ViewMeta, WsId};
use strata_core::project::ProjectDefs;
use strata_core::register::{register_project, table_spec, RegOutcome};
use strata_core::secret::{Secret, SecretRef};
use strata_model::{
    Cell, ConnectionDef, CsvRead, PgPassword, PgSslMode, PgStore, Provider, SourceFormat, TableDef,
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
const SEED: &str = "\
CREATE TABLE public.orders (id INT PRIMARY KEY, customer INT, total INT, tags JSONB);
INSERT INTO public.orders VALUES
  (1, 10, 99, '{\"channel\":\"web\"}'),
  (2, 10, 10, '{\"channel\":\"store\"}'),
  (3, 20, 42, '{\"channel\":\"web\"}');
CREATE TABLE public.customers (id INT PRIMARY KEY, name TEXT);
INSERT INTO public.customers VALUES (10, 'acme'), (20, 'globex');
CREATE VIEW public.big_orders AS SELECT id, total FROM public.orders WHERE total > 50;
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
fn connection(port: u16, catalog: &str, schemas: &[&str]) -> ConnectionDef {
    ConnectionDef {
        address: format!("127.0.0.1:{port}/{DATABASE}"),
        provider: Provider::Postgres(PgStore {
            catalog: catalog.into(),
            user: USER.into(),
            sslmode: PgSslMode::Disable,
            sslrootcert: String::new(),
            password: PgPassword::Keystore,
            schemas: schemas.iter().map(|s| (*s).to_string()).collect(),
        }),
        client_config: BTreeMap::new(),
    }
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
    let key = SecretRef::derived(PG_PASSWORD, &conn.url());
    match Secret::new(value) {
        Some(secret) => key.put(&secret).expect("store the password"),
        None => key.delete().expect("clear the password"),
    }
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

    let engine = Engine::new(BTreeMap::new());
    let conn = connection(port, CATALOG, &["public"]);
    store_password(&conn, PASSWORD);

    let missing = connection(port, "no_password", &["public"]);
    store_password(&missing, "");
    let why = engine
        .connect(missing.clone())
        .await
        .expect_err("no password is stored for it");
    assert!(
        why.contains("No password is stored on this machine") && why.contains(&missing.url()),
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
        engine.db_listing(&conn).is_none(),
        "a refused connection registers nothing"
    );

    engine
        .connect(conn.clone())
        .await
        .expect("the connection registers its catalog");

    enumeration(&engine, port).await;
    qualified_offer(&engine, &conn).await;
    pushdown(&engine).await;
    let fixtures = env::temp_dir().join(format!("strata-pg-{}", process::id()));
    mixed_plan(&engine, &fixtures).await;
    exotic_types_and_refusals(&engine).await;
    statement_policy(&engine, &fixtures).await;
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
            vec!["public".to_string(), "orders".to_string()],
        ],
        "every schema the role can see, and every relation in them"
    );

    let (catalog, listing) = engine
        .db_listing(&connection(port, CATALOG, &["public", "warehouse"]))
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
                .map(|r| (r.name.as_str(), r.relkind.as_str()))
                .collect::<Vec<_>>()),
        Some(vec![
            ("big_orders", "v"),
            ("customers", "r"),
            ("orders", "r")
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

    assert!(
        engine
            .query(
                WsId(1),
                RunTag(10),
                format!(
                    "SELECT id FROM {CATALOG}.public.orders WHERE json_get_str(tags, 'channel') \
                     = 'web'"
                ),
                200,
            )
            .await
            .is_err(),
        "DB-08 flips this assertion; until then a pushed-down accessor must fail loudly rather \
         than quietly answering from a local fallback that does not exist"
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

/// **A reconnect replaces, and a disconnect stops resolving** — a phase of the test above.
async fn reconnect_and_disconnect(engine: &Engine, port: u16) {
    let renamed = connection(port, "warehouse", &["public"]);
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
            .is_err(),
        "the name it was registered under before must stop resolving"
    );

    engine.disconnect(&renamed.url());
    assert!(
        engine
            .query(
                WsId(1),
                RunTag(16),
                "SELECT id FROM warehouse.public.orders".to_string(),
                200,
            )
            .await
            .is_err(),
        "a forgotten connection's catalog must stop resolving"
    );
    assert!(
        engine.db_listing(&renamed).is_none(),
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
/// point: `CREATE TABLE AS` and `CREATE EXTERNAL TABLE` refuse a rootless engine *before* they look
/// at the target, so those two rows would otherwise assert nothing about the catalog. That ordering
/// is right and stays; it is unobservable in the app, where a window always has a project.
async fn statement_policy(engine: &Engine, dir: &Path) {
    engine.set_data_dir(dir);
    for sql in [
        format!("DROP TABLE {CATALOG}.public.orders"),
        format!("DROP VIEW {CATALOG}.public.big_orders"),
        format!("CREATE TABLE {CATALOG}.public.mine AS SELECT 1 AS id"),
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

    let engine = Engine::new(BTreeMap::new());
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
    let engine = Engine::new(BTreeMap::new());
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
