//! **A connection, against a real S3 server** (W7) — the half no unit test can reach.
//!
//! `engine::store`'s unit tests stop at the registry: they prove a `ConnectionDef` produces a store
//! under the right key, never that anything can be *read* through it. So this drives MinIO in a
//! container and asserts the whole chain — connection → registered object store → a table def
//! naming that connection → `register_external`'s listing and inference → rows. It is the only
//! thing that exercises **`SigV4` signing through the `aws-config` bridge**, where a server that
//! verifies signatures is the only witness that the credential triple is right.
//!
//! **A real server rather than a mock**, because an S3 mock is written by the same understanding of
//! the protocol it is meant to check. The fixture is seeded with `aws-sdk-s3` for the same reason:
//! a different client from the one under test, so the write and read sides cannot share a mistake.
//!
//! **Deliberately not `#[ignore]`d.** An ignored test is one nobody runs, and this is the only
//! thing that would notice a regression in the credential bridge. A container runtime is therefore
//! a development prerequisite; without one this fails rather than quietly passing, because "no
//! runtime" and "the code is fine" must not look the same. The runtime is found from
//! `~/.testcontainers.properties` or `DOCKER_HOST` — the former only because `testcontainers`
//! carries **`properties-config`**, without which a Testcontainers Cloud agent reads as no runtime
//! at all. That is how this first failed on CI.
//!
//! **Two providers, S3 and HTTP**, off one container: MinIO is an S3 API and an ordinary HTTP
//! origin once a bucket is world-readable. The HTTP arm reads a single object rather than a prefix,
//! and that is not a shortcut — `object_store`'s HTTP store lists through WebDAV PROPFIND, which
//! MinIO does not implement.
//!
//! **GCS is a known gap**, not an oversight. `object_store`'s GCS client speaks the XML API and
//! needs both list and get; the JSON-API emulators serve neither, localgcp has no XML list, and
//! MinIO 403s the GCS arm because a service-account file with `disable_oauth` sends an empty
//! `Authorization: Bearer` header. Real GCS authenticates with OAuth tokens, which no emulator
//! exercises, so GCS coverage needs a real bucket.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::Path;
use std::time::{Duration, Instant};
use std::{env, fs, process};

use strata_core::project::{save_defs, ProjectDefs};
use strata_engine::register::table_spec;
use strata_engine::{Connections, Engine, RunOutcome, RunTag, StoreEffect, TableSpec, WsId};
use strata_model::{
    ConnectionDef, CsvRead, Provider, S3Auth, S3Store, SourceFormat, TableDef, TableOrigin,
};
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::minio::MinIO;

/// The bucket the fixture is seeded into, and the connection's authority.
const BUCKET: &str = "strata-lake";

/// What the two connections are **called** — the name a table def carries, and what
/// `table_spec` resolves back to a store prefix. Written down rather than minted, so the
/// composition under test is driven by the same string a user would have typed.
const LAKE: &str = "lake";
const ORIGIN: &str = "origin";
/// MinIO's own root credentials (`testcontainers_modules::minio` starts it with the image
/// defaults). The test puts these in the **environment**, which is where an ambient connection
/// looks — see [`ambient`].
const KEY_ID: &str = "minioadmin";
const SECRET: &str = "minioadmin";
/// MinIO does not care which region it is told, but S3 connections must name one
/// (`engine::store` refuses a blank one), so the test names the conventional default.
const REGION: &str = "us-east-1";

/// Two rows the query at the end can count.
const REGIONS_CSV: &str = "id,region\n1,emea\n2,apac\n3,amer\n";

/// A **Hive-partitioned** lake under one prefix: `year=`/`month=` folders whose names carry the
/// values, and files that hold only the other columns. Two partitions with different row counts,
/// so a query filtered to one of them can be told from a query over both.
const HIVE_PREFIX: &str = "hive/";
const HIVE_2024: (&str, &str) = ("hive/year=2024/month=03/part.csv", "id,tally\n1,10\n2,20\n");
const HIVE_2025: (&str, &str) = ("hive/year=2025/month=01/part.csv", "id,tally\n3,30\n");

/// The **mistake** the partition diagnosis exists for: a lake laid out under plain `2024/`
/// folders, read by a def that declares `year`. Hive partitions are `key=value` directories, so
/// an unkeyed level matches nothing and DataFusion calls the location empty — with the files
/// sitting right there.
const HIVE_UNKEYED_PREFIX: &str = "flat/";
const HIVE_UNKEYED: (&str, &str) = ("flat/2024/part.csv", "id,tally\n9,90\n");

/// How long to keep asking for a container while the *provider* says it is at capacity, and
/// how long to wait between asks. Sized for a hosted runtime handing out one worker at a time:
/// the previous holder's session is released at its job's end, so the wait is a handover, not a
/// queue — a minute and a half of asking is generous for that and still fails inside the run.
const CAPACITY_RETRY_BUDGET: Duration = Duration::from_secs(90);
const CAPACITY_RETRY_GAP: Duration = Duration::from_secs(10);

/// Is this failure the runtime saying **busy**, rather than saying no?
///
/// The distinction is the whole reason this test is not `#[ignore]`d: "no runtime" must keep
/// failing loudly, because it must never look like "the code is fine". But a provider that
/// hands out a fixed number of workers and is currently out of them has not told us anything
/// about the code at all, and retrying is the honest response to it rather than a way of
/// hiding a red run.
///
/// Matched on the message because that is where the provider puts it — the refusal arrives as
/// a generic `CreateContainer` either way. Two spellings, one fault: Testcontainers Cloud
/// answers `Failed to get a worker: ErrValidator: too many concurrent requests` when it can
/// answer at all, and drops the response mid-flight (`hyper` calls that `IncompleteMessage`)
/// when it cannot. Anything else — no endpoint, a bad image, a refused connection — is not a
/// capacity signal and falls straight through to the panic.
fn at_capacity(err: &impl Display) -> bool {
    let msg = err.to_string();
    msg.contains("too many concurrent requests") || msg.contains("IncompleteMessage")
}

/// A running MinIO, and the `http://` endpoint an S3 connection reaches it on.
///
/// The container is returned alongside the endpoint and must be held for the test's duration —
/// dropping it stops the server.
///
/// **Retries a capacity refusal, and only that** — see [`at_capacity`]. CI runs against a
/// hosted runtime with a single worker, and a worker is held by whoever has it until their
/// session is released, so an overlap is a wait rather than a fault. Serializing the CI job
/// covers two *live* jobs colliding and cannot cover the handover itself, which happens on the
/// provider's side where nothing here can watch it. Every other failure, and this one past its
/// budget, panics with the message it always did.
async fn minio() -> (ContainerAsync<MinIO>, String) {
    let deadline = Instant::now() + CAPACITY_RETRY_BUDGET;
    let container = loop {
        match MinIO::default().start().await {
            Ok(container) => break container,
            Err(err) if at_capacity(&err) && Instant::now() < deadline => {
                eprintln!("container runtime is at capacity, retrying: {err}");
                tokio::time::sleep(CAPACITY_RETRY_GAP).await;
            }
            Err(err) => panic!("MinIO starts (is a Docker runtime available?): {err}"),
        }
    };
    let port = container
        .get_host_port_ipv4(9000)
        .await
        .expect("MinIO's API port");
    (container, format!("http://127.0.0.1:{port}"))
}

/// Create `BUCKET` and put every `(key, body)` in `objects`, through `aws-sdk-s3`.
///
/// Deliberately not through `object_store`: it has no create-bucket call at all, and using a
/// second implementation for the write is what makes the read a real check rather than a
/// round trip of our own assumptions.
///
/// Every object in one call, because the bucket is created **once**: MinIO answers a second
/// `CreateBucket` with `BucketAlreadyOwnedByYou`, so a per-object seed would have to either
/// swallow that error (and with it a real one) or ask whether it is the first caller.
async fn seed(endpoint: &str, objects: &[(&str, &str)]) {
    use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
    use aws_sdk_s3::primitives::ByteStream;
    use aws_sdk_s3::{Client, Config};

    let config = Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(REGION))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .credentials_provider(Credentials::new(KEY_ID, SECRET, None, None, "test"))
        .build();
    let client = Client::from_conf(config);

    client
        .create_bucket()
        .bucket(BUCKET)
        .send()
        .await
        .expect("create the bucket");
    for (key, body) in objects {
        client
            .put_object()
            .bucket(BUCKET)
            .key(*key)
            .body(ByteStream::from(body.as_bytes().to_vec()))
            .send()
            .await
            .unwrap_or_else(|e| panic!("put the fixture object '{key}': {e}"));
    }

    client
        .put_bucket_policy()
        .bucket(BUCKET)
        .policy(format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":["*"]}},"Action":["s3:GetObject"],"Resource":["arn:aws:s3:::{BUCKET}/*"]}}]}}"#
        ))
        .send()
        .await
        .expect("make the bucket world-readable");
}

/// Put MinIO's credentials where an **Ambient** connection will find them: the environment is
/// the first arm of `aws-config`'s default chain, and it is the arm a developer's shell
/// actually carries.
///
/// `AWS_SESSION_TOKEN` is set blank (which the SDK reads as absent) rather than left alone, so
/// a stray token in the runner's environment cannot be sent to MinIO and rejected.
fn ambient() {
    env::set_var("AWS_ACCESS_KEY_ID", KEY_ID);
    env::set_var("AWS_SECRET_ACCESS_KEY", SECRET);
    env::set_var("AWS_SESSION_TOKEN", "");
}

/// The connection under test: S3-compatible, reached over plain HTTP at `endpoint`, signing
/// with whatever the host's chain resolves.
fn connection(endpoint: &str, auth: S3Auth) -> ConnectionDef {
    ConnectionDef {
        address: BUCKET.into(),
        name: LAKE.into(),
        provider: Provider::S3(S3Store {
            region: REGION.into(),
            auth,
            endpoint: endpoint.into(),
            allow_http: true,
        }),
        client_config: client_options(),
    }
}

/// The two options both connections carry: one `object_store` must parse into a duration, and one
/// it must accept as a header value.
fn client_options() -> BTreeMap<String, String> {
    [("timeout", "30s"), ("user_agent", "strata-integration")]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The **same server, reached as a plain HTTP origin**: no bucket, no signing, the address is the
/// whole URL. What an `http(s)://` connection is for — a public data drop rather than a store you
/// hold credentials to.
///
/// It carries the same **client options** the S3 connection does, so `with_config` is proved
/// through both routes into `ClientOptions`: `HttpBuilder`'s direct one here, and
/// `AmazonS3ConfigKey::Client(..)` there.
fn http_connection(endpoint: &str) -> ConnectionDef {
    ConnectionDef {
        address: endpoint.into(),
        name: ORIGIN.into(),
        provider: Provider::Http,
        client_config: client_options(),
    }
}

/// A table def **as Table Config writes one over a connection** (W7 · 04): the connection's URL,
/// and a source relative to its bucket. Composed into the spec by `register::table_spec`, which
/// is the app's own mapping — so what registers below is the string a def really produces rather
/// than one this test wrote out by hand.
///
/// The trailing `/` is load-bearing: without it `ListingTableUrl` reads the path as a single file
/// (`engine::catalog::listing_url` only adds one for a local directory, which a bucket prefix is
/// not).
fn known(endpoint: &str) -> Connections {
    Connections::of(&[
        connection(endpoint, S3Auth::Ambient),
        http_connection(endpoint),
    ])
}

fn table(endpoint: &str) -> TableSpec {
    let def = TableDef {
        name: "regions".into(),
        format: SourceFormat::Csv(CsvRead::default()),
        connection: Some(LAKE.into()),
        sources: vec!["data/".into()],
        partition_cols: Vec::new(),
        origin: TableOrigin::External,
    };
    table_spec(Path::new("/nowhere"), &def, &known(endpoint))
}

/// The **Hive-partitioned** table over that same bucket: one bucket-relative prefix, and the two
/// folder levels declared as typed columns exactly as the Configure window's Hive section writes
/// them.
///
/// `Int32`, not the `Utf8` DataFusion infers on its own: the types are the def's, and a value
/// that came out of a folder name has to arrive as the column the user asked for or the cast
/// warning that surface shows would be about nothing.
fn hive_table(endpoint: &str) -> TableSpec {
    let def = TableDef {
        name: "tallies".into(),
        format: SourceFormat::Csv(CsvRead::default()),
        connection: Some(LAKE.into()),
        sources: vec![HIVE_PREFIX.into()],
        partition_cols: vec![
            ("year".into(), "Int32".into()),
            ("month".into(), "Int32".into()),
        ],
        origin: TableOrigin::External,
    };
    table_spec(Path::new("/nowhere"), &def, &known(endpoint))
}

/// The same object over the **HTTP** connection: one file, and deliberately no trailing slash.
/// `object_store`'s HTTP store lists through WebDAV PROPFIND, which MinIO does not implement,
/// so a prefix-shaped source could not be read here — a single object is what this provider is
/// for, and it is read through `head` + `get` like any other.
///
/// An HTTP connection's URL is a whole origin, so the bucket is part of the *table's* source —
/// the same composition, over an address the provider does not supply a scheme for.
fn http_table(endpoint: &str) -> TableSpec {
    let def = TableDef {
        name: "regions_http".into(),
        format: SourceFormat::Csv(CsvRead::default()),
        connection: Some(ORIGIN.into()),
        sources: vec![format!("{BUCKET}/data/regions.csv")],
        partition_cols: Vec::new(),
        origin: TableOrigin::External,
    };
    table_spec(Path::new("/nowhere"), &def, &known(endpoint))
}

/// **A typed `CREATE EXTERNAL TABLE` over the connected bucket** (ED-10) — a *phase* of the test
/// below, called in sequence, not a test of its own: it reads through the store that test
/// registered, and the ambient credentials it signs with are process-wide (see that function's
/// doc comment for why a second `#[tokio::test]` would race them).
///
/// What only a live store can show. A typed `LOCATION` arrives as one composed string, so the arm
/// has to take it apart into the pair every other path holds — the connection's URL and a
/// bucket-relative source — land it on a connection this project actually has, compose it back
/// through `resolve_source`, and read the objects. Everything up to that last step is asserted in
/// `ddl::external`'s own tests; this is the step that needs a bucket.
///
/// The project folder is real but empty: it is never consulted for a source over a connection, and
/// it is here because a def is durable and the arm refuses to write one with nowhere to put it. It
/// is **not** cleaned up here — the engine goes on pointing at it for the phases that follow, and
/// removing a live data root would leave a later phase spooling into a directory that is gone.
/// The caller sweeps it once the test is over.
async fn typed_registration(engine: &Engine, project: &Path) {
    fs::create_dir_all(project).expect("a project folder");
    save_defs(project, &ProjectDefs::default()).expect("scaffold");
    engine.set_data_dir(project);

    let RunOutcome::Statement(report) = engine
        .run(
            WsId(1),
            RunTag(10),
            format!(
                "CREATE EXTERNAL TABLE typed STORED AS CSV LOCATION 's3://{BUCKET}/data/' \
                 OPTIONS ('format.has_header' 'true')"
            ),
            50,
        )
        .await
        .expect("the typed registration runs")
    else {
        panic!("a registration is a statement, not rows");
    };
    let Some(StoreEffect::TableUpserted { def, meta }) = report.effect else {
        panic!("{report:?}");
    };
    assert_eq!(
        (def.connection.as_deref(), def.sources.as_slice()),
        (
            Some(format!("s3://{BUCKET}").as_str()),
            &["data/".to_string()][..]
        ),
        "the LOCATION split into the connection it names and a source relative to its bucket"
    );
    let columns: Vec<&str> = meta.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(columns, ["id", "region"], "the schema came off the objects");

    let (output, _) = engine
        .query(WsId(1), RunTag(11), "SELECT * FROM typed".into(), 50)
        .await
        .expect("query the typed table");
    assert_eq!(output.total, 3, "and it reads through the same store");
}

/// **The whole chain, against a server that verifies signatures.**
///
/// Connection → registered store → schema inference over a real listing → a query that returns
/// the rows. The signing is the part worth the container: `S3Auth::Ambient` resolves through
/// `aws-config` and is wrapped by `engine::store::SdkCredentials`, so a bridge that handed over
/// the wrong fields would produce a store that registers perfectly and then gets a 403 from
/// every request — which is exactly the failure mode the unit tests cannot see.
///
/// **One test, deliberately, rather than one per assertion.** An ambient connection reads its
/// credentials from the process environment, and cargo runs tests in parallel threads of one
/// process — so a second test setting a different `AWS_ACCESS_KEY_ID` races this one's and
/// either may win. That is not hypothetical: it was two tests first, and the good path failed
/// with a 403 because the rejection case's wrong key arrived mid-run. Sequential phases in a
/// single test are the fix that does not depend on `--test-threads=1` being remembered, and
/// they cost one container instead of two.
#[tokio::test]
async fn a_table_over_a_connection_reads_through_the_object_store() {
    let (_minio, endpoint) = minio().await;
    seed(
        &endpoint,
        &[
            ("data/regions.csv", REGIONS_CSV),
            HIVE_2024,
            HIVE_2025,
            HIVE_UNKEYED,
        ],
    )
    .await;
    ambient();

    let engine = Engine::builder().build();
    engine
        .connect(connection(&endpoint, S3Auth::Ambient))
        .await
        .expect("the connection registers its object store");

    let meta = engine
        .register(table(&endpoint))
        .await
        .expect("the table registers over the bucket");
    let columns: Vec<&str> = meta.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(columns, ["id", "region"], "the schema came off the object");

    let (output, _) = engine
        .query(WsId(1), RunTag(1), "SELECT * FROM regions".into(), 50)
        .await
        .expect("query the remote table");
    assert_eq!(output.total, 3, "every seeded row came back");

    let found = engine
        .detect_partitions(None, None, vec![format!("s3://{BUCKET}/{HIVE_PREFIX}")])
        .await;
    assert_eq!(
        found,
        ["year", "month"],
        "the levels were listed through the object store, outermost first"
    );

    let meta = engine
        .register(hive_table(&endpoint))
        .await
        .expect("the partitioned table registers over the bucket");
    let columns: Vec<(&str, &str)> = meta
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.dtype.as_str()))
        .collect();
    assert_eq!(
        columns,
        [
            ("id", "Int64"),
            ("tally", "Int64"),
            ("year", "Int32"),
            ("month", "Int32")
        ],
        "the file's columns, then the folder tree's — as the types the def asked for"
    );

    let (output, _) = engine
        .query(
            WsId(1),
            RunTag(2),
            "SELECT year, month, tally FROM tallies ORDER BY year, tally".into(),
            50,
        )
        .await
        .expect("query the partitioned table");
    let cells: Vec<Vec<&str>> = output
        .rows
        .iter()
        .map(|row| row.iter().map(|c| c.text.as_str()).collect())
        .collect();
    assert_eq!(
        cells,
        [
            ["2024", "3", "10"],
            ["2024", "3", "20"],
            ["2025", "1", "30"]
        ],
        "every partition's rows came back carrying its folder's values"
    );

    let (pruned, _) = engine
        .query(
            WsId(1),
            RunTag(3),
            "SELECT tally FROM tallies WHERE year = 2025".into(),
            50,
        )
        .await
        .expect("query one partition");
    assert_eq!(pruned.total, 1, "only the 2025 partition was read");
    assert_eq!(pruned.rows[0][0].text, "30");

    let unkeyed = TableSpec {
        name: "flat".into(),
        paths: vec![format!("s3://{BUCKET}/{HIVE_UNKEYED_PREFIX}")],
        ..hive_table(&endpoint)
    };
    let refused = engine
        .register(unkeyed)
        .await
        .expect_err("an unkeyed level matches no partition column");
    assert_eq!(
        refused,
        format!(
            "No .csv files under 's3://{BUCKET}/{HIVE_UNKEYED_PREFIX}' match the partition \
             columns 'year', 'month'."
        ),
        "the store was listed, found the files, and the columns are what missed them"
    );

    let empty = TableSpec {
        name: "empty".into(),
        paths: vec![format!("s3://{BUCKET}/nothing/")],
        ..hive_table(&endpoint)
    };
    let refused = engine
        .register(empty)
        .await
        .expect_err("a prefix with nothing under it");
    assert_eq!(
        refused,
        format!("No files matched 's3://{BUCKET}/nothing/'."),
        "the store settled that there is nothing there, so the partition columns are not blamed"
    );

    let project = env::temp_dir().join(format!("strata_typed_external_{}", process::id()));
    let _ = fs::remove_dir_all(&project);
    typed_registration(&engine, &project).await;

    let orphan = TableSpec {
        name: "orphan".into(),
        paths: vec!["s3://not-connected/data/".into()],
        ..table(&endpoint)
    };
    let refused = engine.register(orphan).await.expect_err("no object store");
    assert!(
        refused.to_lowercase().contains("object store"),
        "the failure names the missing store: {refused}"
    );

    engine
        .connect(http_connection(&endpoint))
        .await
        .expect("the HTTP connection registers its object store");
    let meta = engine
        .register(http_table(&endpoint))
        .await
        .expect("the table registers over the HTTP origin");
    let columns: Vec<&str> = meta.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(columns, ["id", "region"], "the schema came off the object");

    let (output, _) = engine
        .query(WsId(1), RunTag(4), "SELECT * FROM regions_http".into(), 50)
        .await
        .expect("query the remote table");
    assert_eq!(output.total, 3, "every seeded row came back over HTTP");

    let orphan = TableSpec {
        name: "orphan_http".into(),
        paths: vec!["http://127.0.0.1:1/lake/x.csv".into()],
        ..http_table(&endpoint)
    };
    let refused = engine.register(orphan).await.expect_err("no object store");
    assert!(
        refused.to_lowercase().contains("object store"),
        "the failure names the missing store: {refused}"
    );

    engine.disconnect(&connection(&endpoint, S3Auth::Ambient).named());
    let forgotten = TableSpec {
        name: "forgotten".into(),
        ..table(&endpoint)
    };
    let refused = engine
        .register(forgotten)
        .await
        .expect_err("the store is gone");
    assert!(
        refused.to_lowercase().contains("object store"),
        "a forgotten bucket is unreachable, exactly as one that was never connected: {refused}"
    );

    env::set_var("AWS_ACCESS_KEY_ID", "AKIAWRONGKEY");
    env::set_var("AWS_SECRET_ACCESS_KEY", "wrong-secret");

    let engine = Engine::builder().build();
    engine
        .connect(connection(&endpoint, S3Auth::Ambient))
        .await
        .expect("a 403 is an authorization answer, not a description fault");
    let refused = engine
        .register(table(&endpoint))
        .await
        .expect_err("MinIO rejects the signature");
    let lower = refused.to_lowercase();
    assert!(
        lower.contains("403")
            || lower.contains("forbidden")
            || lower.contains("access denied")
            || lower.contains("signature"),
        "the row should carry MinIO's rejection of the signature, got: {refused}"
    );

    let _ = fs::remove_dir_all(&project);
}
