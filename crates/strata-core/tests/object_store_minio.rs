//! **A connection, against a real S3 server** (W7) — the half no unit test can reach.
//!
//! `engine::store`'s own tests prove a `ConnectionDef` produces a store and that the store
//! lands under the right registry key. What they cannot prove is that anything can be *read*
//! through it: every one of them stops at the registry, and the two that involve credentials
//! either skip signing entirely (`Anonymous`) or resolve a credential and throw it away
//! without ever signing a request with it.
//!
//! So this drives MinIO in a container and asserts the whole chain end to end — connection →
//! registered object store → `register_external`'s listing and schema inference → a query that
//! returns rows. In particular it is the only thing that exercises **SigV4 signing through the
//! `aws-config` bridge**: `SdkCredentials` hands `object_store` a key/secret/token triple, and
//! a server that actually verifies signatures is the only witness that the triple is right.
//!
//! **Why a real server rather than a mock.** An S3 mock is written by the same understanding of
//! the protocol it is meant to check, so a misreading produces a mock that agrees with the bug
//! and a test that passes. MinIO cannot be talked round. The fixture is seeded with
//! `aws-sdk-s3` for the same reason — a different client from the one under test, so the write
//! and read sides cannot share a mistake.
//!
//! **An ordinary integration test, deliberately not `#[ignore]`d.** An ignored test is one
//! nobody runs, and this is exactly the test worth running: it is the only thing that would
//! notice a regression in the credential bridge or the registration order. So a **container
//! runtime is a development prerequisite** for this repo — Testcontainers Cloud, Docker,
//! colima, whichever. Without one this fails rather than quietly passing, which is the point:
//! "no runtime" and "the code is fine" must not look the same. The runtime is discovered from
//! `~/.testcontainers.properties` or `DOCKER_HOST` — the former only because `testcontainers`
//! is built with **`properties-config`**, without which that file is `#[cfg]`'d out and a
//! Testcontainers Cloud agent (which advertises itself only through that file) reads as no
//! runtime at all. That is not hypothetical: it is how this first failed on CI.
//!
//! **S3 only, and GCS is not an oversight** — it was tried, and the gap is *listing*.
//! `object_store`'s GCS client speaks the **XML** API and needs two halves of it:
//! `{base}/{bucket}?list-type=2` to list and `{base}/{bucket}/{key}` to get (its
//! `gcp/client.rs:156,670`). DataFusion lists a prefix before reading, so neither is optional.
//! MiniSky and fake-gcs-server are JSON-API only; localgcp does serve XML *downloads* but has
//! no XML list, so a listing request 404s. MinIO has the whole XML API and was measured: the
//! GCS arm gets a **403 even on a world-readable bucket**, because a service-account file with
//! `disable_oauth` makes `object_store` send an empty `Authorization: Bearer` header, which
//! MinIO refuses rather than reading as anonymous. None of this is evidence against the GCS
//! provider — real GCS authenticates with OAuth bearer tokens, the path no emulator exercises
//! — it means GCS coverage needs a real bucket rather than a container.

use std::collections::BTreeMap;

use strata_core::engine::{Engine, RunTag, TableSpec, WsId};
use strata_model::{ConnectionDef, CsvRead, Provider, S3Auth, S3Store, SourceFormat};
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::minio::MinIO;

/// The bucket the fixture is seeded into, and the connection's authority.
const BUCKET: &str = "strata-lake";
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

/// A running MinIO, and the `http://` endpoint an S3 connection reaches it on.
///
/// The container is returned alongside the endpoint and must be held for the test's duration —
/// dropping it stops the server.
async fn minio() -> (ContainerAsync<MinIO>, String) {
    let container = MinIO::default()
        .start()
        .await
        .expect("MinIO starts (is a Docker runtime available?)");
    let port = container
        .get_host_port_ipv4(9000)
        .await
        .expect("MinIO's API port");
    (container, format!("http://127.0.0.1:{port}"))
}

/// Create `BUCKET` and put one CSV object at `key`, through `aws-sdk-s3`.
///
/// Deliberately not through `object_store`: it has no create-bucket call at all, and using a
/// second implementation for the write is what makes the read a real check rather than a
/// round trip of our own assumptions.
async fn seed(endpoint: &str, key: &str, body: &str) {
    use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
    use aws_sdk_s3::primitives::ByteStream;
    use aws_sdk_s3::{Client, Config};

    let config = Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(REGION))
        .endpoint_url(endpoint)
        // MinIO serves path-style; virtual-hosted would resolve a hostname that isn't there.
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
    client
        .put_object()
        .bucket(BUCKET)
        .key(key)
        .body(ByteStream::from(body.as_bytes().to_vec()))
        .send()
        .await
        .expect("put the fixture object");
}

/// Put MinIO's credentials where an **Ambient** connection will find them: the environment is
/// the first arm of `aws-config`'s default chain, and it is the arm a developer's shell
/// actually carries.
///
/// `AWS_SESSION_TOKEN` is set blank (which the SDK reads as absent) rather than left alone, so
/// a stray token in the runner's environment cannot be sent to MinIO and rejected.
fn ambient() {
    std::env::set_var("AWS_ACCESS_KEY_ID", KEY_ID);
    std::env::set_var("AWS_SECRET_ACCESS_KEY", SECRET);
    std::env::set_var("AWS_SESSION_TOKEN", "");
}

/// The connection under test: S3-compatible, reached over plain HTTP at `endpoint`, signing
/// with whatever the host's chain resolves.
fn connection(endpoint: &str, auth: S3Auth) -> ConnectionDef {
    ConnectionDef {
        bucket: BUCKET.into(),
        provider: Provider::S3(S3Store {
            region: REGION.into(),
            auth,
            endpoint: endpoint.into(),
            allow_http: true,
        }),
    }
}

/// One CSV table over the seeded prefix. The trailing `/` is load-bearing: without it
/// `ListingTableUrl` reads the path as a single file (`engine::catalog::listing_url` only adds
/// one for a local directory, which a bucket prefix is not).
fn table() -> TableSpec {
    TableSpec {
        name: "regions".into(),
        paths: vec![format!("s3://{BUCKET}/data/")],
        format: SourceFormat::Csv(CsvRead::default()),
        partitions: Vec::new(),
    }
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
    seed(&endpoint, "data/regions.csv", REGIONS_CSV).await;
    ambient();

    let engine = Engine::new(BTreeMap::new());
    engine
        .connect(connection(&endpoint, S3Auth::Ambient))
        .await
        .expect("the connection registers its object store");

    // Registration reads the object through that store — this is the first real network
    // traffic our own code causes, and the first signed request.
    let meta = engine
        .register(table())
        .await
        .expect("the table registers over the bucket");
    let columns: Vec<&str> = meta.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(columns, ["id", "region"], "the schema came off the object");

    let (output, _) = engine
        .query(WsId(1), RunTag(1), "SELECT * FROM regions".into(), 50)
        .await
        .expect("query the remote table");
    assert_eq!(output.total, 3, "every seeded row came back");

    // …and the engine reads through the connection, not around it: a bucket nothing connected
    // has no store, which is the failure a table over an unregistered bucket must give.
    let orphan = TableSpec {
        name: "orphan".into(),
        paths: vec!["s3://not-connected/data/".into()],
        ..table()
    };
    let refused = engine.register(orphan).await.expect_err("no object store");
    assert!(
        refused.to_lowercase().contains("object store"),
        "the failure names the missing store: {refused}"
    );

    // --- and Forget takes it back out ----------------------------------------------------
    //
    // **`disconnect` is the only thing that can un-register a bucket**, which is why it is
    // pinned against a real store rather than asserted on a return value it does not have:
    // `connect` is additive by contract and never sees the def it replaced, so without this
    // call a forgotten connection stays queryable for the life of the window and the pane says
    // it is gone. The failure it must produce is the orphan's above, on the bucket that worked
    // two lines ago.
    engine.disconnect(&connection(&endpoint, S3Auth::Ambient).url());
    let forgotten = TableSpec {
        name: "forgotten".into(),
        ..table()
    };
    let refused = engine
        .register(forgotten)
        .await
        .expect_err("the store is gone");
    assert!(
        refused.to_lowercase().contains("object store"),
        "a forgotten bucket is unreachable, exactly as one that was never connected: {refused}"
    );

    // --- and now the same connection with credentials the server refuses -----------------
    //
    // **A connection whose credentials the server rejects fails at the table, not at the
    // connection** — the honest limit of `connect`'s probe, pinned here rather than left as
    // folklore. The probe asks the *host's chain* whether it can produce a credential; it
    // never asks the bucket whether that credential is any good, because that would be a
    // network round trip per connection on every project open. So a wrong-but-well-formed key
    // resolves, the connection goes green, and the refusal surfaces on the first read.
    //
    // Last, and in the same test, for the reason the doc comment gives: this rewrites the
    // environment the phases above depend on.
    std::env::set_var("AWS_ACCESS_KEY_ID", "AKIAWRONGKEY");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "wrong-secret");

    // A fresh engine, so this phase starts from nothing: the one above has had its store
    // disconnected by the block before this, and even before that its registration came from
    // the good credentials. Either way a pass here cannot be an artefact of the earlier one.
    let engine = Engine::new(BTreeMap::new());
    engine
        .connect(connection(&endpoint, S3Auth::Ambient))
        .await
        .expect("the chain resolves, so the connection registers");
    let refused = engine
        .register(table())
        .await
        .expect_err("MinIO rejects the signature");
    assert!(
        !refused.is_empty(),
        "the table's row carries the server's refusal"
    );
}
