//! Object stores: turning a [`ConnectionDef`] into a live `object_store` and registering it
//! on the session (W7, `docs/CONNECTIONS_SPEC.md`).
//!
//! **DataFusion core resolves nothing.** There is no built-in "read `s3://…`": the embedder
//! builds a store and calls `register_object_store` **per bucket**, or every scan of that
//! bucket fails with *"No suitable object store found"*. That call is the whole of what a
//! connection *does*, which is why the def's identity is exactly what the registry keys on —
//! scheme + authority, no path ([`ObjectStoreUrl::parse`] enforces it, so a bucket with a
//! path in it is refused here rather than registering under a key nothing looks up).
//!
//! **No arm of this module takes a secret value.** Every arm resolves through the host's own
//! provider chain, a named profile, a key **file** the OS already lets the user read, or not
//! at all (anonymous). The one place a credential value exists is inside
//! [`SdkCredentials::get_credential`], for the length of one signed request.
//!
//! That is the object-store half of a rule the DB workstream deliberately rewrote (see
//! [`strata_model::ConnectionDef`]): a secret Strata genuinely must hold lives in the OS keystore
//! and is read per use ([`strata_core::secret`], and [`sources`](super::sources) for the arm that
//! does it). Nothing changes here — object stores have host-side credential chains, so this
//! module still needs no secret at all.

use std::sync::Arc;

use aws_config::profile::ProfileFileCredentialsProvider;
use aws_config::provider_config::ProviderConfig;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use aws_types::os_shim_internal::{Env, Fs};
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::prelude::*;
use futures::stream::StreamExt;
use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey, AwsCredential, AwsCredentialProvider};
use object_store::gcp::{GoogleCloudStorageBuilder, GoogleConfigKey};
use object_store::http::HttpBuilder;
use object_store::{ClientConfigKey, CredentialProvider, Error, ObjectStore};

use strata_arrow::client::check_client_config;
use strata_model::{ConnectionDef, GcsAuth, Provider, S3Auth};

use super::connect::{self, Registration};

/// Build the object store `conn` describes and register it on `ctx`, so tables over its
/// bucket can be registered and scanned.
///
/// **All or nothing: on `Err`, nothing is registered for this bucket** — anything an earlier pass
/// registered is deregistered here rather than left behind, which is what makes the outcome
/// foldable onto a single `Reg` row. Leaving the old store would give a row reading `Failed` over a
/// bucket the engine still answers for.
///
/// So the credential chain is probed *before* the store goes in. Without that, a connection with no
/// usable credentials registers happily and the diagnosis lands on every table over the bucket, one
/// opaque signing error each. It resolves the chain **once** and throws the answer away; the
/// provider on the store resolves per request, so rotating credentials keep working.
///
/// **And then the bucket is asked, too** ([`reachable`]), which is a change of position: the case
/// the round-trip argument did not cover is a description that is well-formed and *wrong*, which
/// no local check can see.
///
/// Idempotent: registering over an existing key replaces it, which is what a re-scan wants. One
/// thing it cannot clean up, because it is never told about it: a connection whose bucket was
/// edited leaves the store registered under the old URL, which the edit gesture owns.
pub async fn connect(ctx: &SessionContext, conn: &ConnectionDef) -> Result<(), String> {
    let prepared = match prepare(conn).await {
        Ok((url, store)) => match reachable(conn, &store).await {
            Ok(()) => Ok((url, store)),
            Err(why) => Err(why),
        },
        Err(why) => Err(why),
    };
    settle(ctx, conn, prepared)
}

/// Apply a prepared store to the session, or take back whatever this connection last registered
/// — this arm's half of [`connect::settle`]'s contract.
///
/// Separate from `connect` so the tests' probe-free path settles through the *same* code rather
/// than a helper that restates it. That distinction has teeth: the first version of that helper
/// registered on `Ok` and simply returned on `Err`, which silently dropped the deregistration —
/// and the test whose whole subject is "a refused reconnect leaves nothing behind" went red
/// against a stand-in that could never have passed it. A test double for a contract has to share
/// the contract, which is now shared one level further out as well.
fn settle(
    ctx: &SessionContext,
    conn: &ConnectionDef,
    prepared: Result<(ObjectStoreUrl, Arc<dyn ObjectStore>), String>,
) -> Result<(), String> {
    connect::settle(
        ctx,
        prepared.map(|(url, store)| Registration::ObjectStore(url, store)),
        || disconnect(ctx, &conn.identity()),
    )
}

/// The URL a connection's store is **registered under** — the scheme DataFusion resolves a remote
/// path by, over the connection's address.
///
/// Composed here rather than carried on the def, because a scheme is this layer's rendering and
/// nothing outside it: what a connection *is* is its [`identity`](ConnectionDef::identity), and
/// two of the three kinds here do not even spell their scheme the way they are named (`gcs` is
/// registered as `gs`). `None` for an identity no object store answers to, which every source
/// connection is.
fn registration_url(identity: &str) -> Option<ObjectStoreUrl> {
    ObjectStoreUrl::parse(store_prefix(identity)?).ok()
}

/// The `scheme://authority` a connection's remote paths hang off — [`registration_url`] as the
/// string a source path is composed onto.
///
/// `None` for an identity no object store answers to, which every source connection is: a table
/// reads files, and a connection that holds relations has none to read.
pub fn store_prefix(identity: &str) -> Option<String> {
    let (kind, address) = identity.split_once(':')?;
    match kind {
        "s3" => Some(format!("s3://{address}")),
        "gcs" => Some(format!("gs://{address}")),
        "http" => Some(address.to_string()),
        _ => None,
    }
}

/// [`store_prefix`] backwards: the connection a location's `scheme://authority` names.
///
/// For the one caller that arrives with a written URL rather than with a connection — a typed
/// `CREATE EXTERNAL TABLE … LOCATION 's3://acme-lake/events/'`, which has to be matched against
/// the project's own connections.
pub fn store_identity(url: &str) -> Option<String> {
    let (scheme, authority) = url.split_once("://")?;
    match scheme.to_ascii_lowercase().as_str() {
        "s3" => Some(format!("s3:{authority}")),
        "gs" => Some(format!("gcs:{authority}")),
        "http" | "https" => Some(format!("http:{url}")),
        _ => None,
    }
}

/// Everything a connection can be judged on **without asking its bucket**: the provider's naming
/// rules, the client options, the registry key, and a store built from all three.
///
/// Split out because its two callers want different things after it — [`connect`] probes the
/// bucket and registers, while the unit tests below assert what a def registers *under*, which is
/// a question about keying rather than about the network. Without the split, testing the keying
/// meant reaching a real bucket.
async fn prepare(conn: &ConnectionDef) -> Result<(ObjectStoreUrl, Arc<dyn ObjectStore>), String> {
    conn.provider.check_address(&conn.address)?;
    check_client_config(&conn.client_config)?;
    let url = registration_url(&conn.identity()).ok_or_else(|| {
        format!(
            "'{}' is not a bucket Strata can register.",
            conn.address.trim()
        )
    })?;
    let store = build(conn).await?;
    Ok((url, store))
}

/// **Does this bucket actually answer?** One request, on the connection's own store, and the
/// difference between a row that means something and a row that means a struct was built.
///
/// This was deliberately absent once, on the grounds that a probe is a round trip per connection on
/// every project open. That traded away too much: `AmazonS3Builder` constructs a store for a bucket
/// that does not exist in the region it was given, so a mistyped region registered **green** and
/// every table over it failed with `object_store`'s bare-redirect message, which names no bucket,
/// no region and no connection.
///
/// **The first page of a listing, not a HEAD and not a whole listing.** `ObjectStore` has no
/// head-bucket call. It has to be `list`'s *stream* taken once: `list_with_delimiter` reads like
/// the cheaper call and drains the paginated stream to assemble a complete `ListResult`, which
/// against the Hive lakes this exists for is a round trip per thousand folders. An empty bucket
/// ends the stream without yielding, which is a **pass**; only a `Some(Err(..))` is a refusal.
///
/// **It refuses exactly one thing: a bucket that is not in the region it was given.** That is the
/// fault no local check can see, and "may I list the root" is a far stronger demand than Strata
/// makes — `connect` is the registration pass's first phase, so there is no table prefix to probe
/// with. A
/// prefix-scoped `s3:ListBucket` and a `GetObject`-only public bucket both answer 403 at the root
/// while working perfectly, so refusing either would take a working project's every table down.
/// Rejected credentials therefore still fail at the first table, exactly as before this probe.
///
/// **Matched on the message, because `object_store` gives us nothing else.** The crate classifies
/// statuses into `PermissionDenied` / `NotFound` / … in `client/retry.rs`, but the S3 list path
/// never reaches it: `aws/client.rs` routes only two variants through that mapping and sends
/// `ListRequest` to `_ => Generic`. `RetryError` is `pub(crate)`, so `status()` cannot be
/// downcast to either. A first version matched on the variants and was dead code in every arm;
/// MinIO caught it. The sentence matched is one `object_store` defines as a literal.
///
/// **HTTP is exempt, and not out of laziness.** Its store lists over WebDAV `PROPFIND`, which most
/// file origins do not implement (MinIO included), so probing one would refuse working connections
/// for a verb their server was never going to answer. An HTTP connection names a whole origin and
/// its table names the object, so the table's own registration tests its reachability.
async fn reachable(conn: &ConnectionDef, store: &Arc<dyn ObjectStore>) -> Result<(), String> {
    if matches!(conn.provider, Provider::Http) {
        return Ok(());
    }
    match store.list(None).next().await {
        Some(Err(e)) if is_bare_redirect(&e) => Err(wrong_region(conn)),
        _ => Ok(()),
    }
}

/// The one listing failure that says the *connection* is wrong rather than the caller's rights.
///
/// S3 answers a cross-region request with a 301 carrying no `Location` header;
/// `object_store` has a dedicated error for it whose `Display` is this literal
/// (`client/retry.rs`, `RequestError::BareRedirect`). Its own text goes on to guess at "an
/// incorrectly configured region" — a guess, because the crate has never heard of the field. We
/// have, so [`wrong_region`] says it outright.
///
/// Substring rather than equality: the sentence arrives wrapped in the layers that carried it,
/// which for a listing is `Error::Generic`'s `Generic {store} error: ` around `RetryError`'s
/// `Error performing {METHOD} {uri} in {elapsed:?}[, after {n} retries, …] - `. Quoted from the
/// two `Display` impls (`lib.rs`, `client/retry.rs`), because this comment used to give the shape
/// as `Generic S3 error: Error performing list request: …` — a plausible sentence the crate does
/// not write, which `catalog::readable` was later built from and matched nothing against.
fn is_bare_redirect(e: &Error) -> bool {
    e.to_string().contains("redirect without LOCATION")
}

/// What a wrong region reads as — naming the bucket and the region, which is the whole of the fix.
fn wrong_region(conn: &ConnectionDef) -> String {
    match &conn.provider {
        Provider::S3(s3) => format!(
            "The bucket '{}' does not answer in region '{}'. Check the region, or that the bucket \
             exists.",
            conn.address.trim(),
            s3.region.trim()
        ),
        _ => format!("'{}' did not answer.", conn.named()),
    }
}

/// Forget the object store registered under `url` — the Forget gesture's engine half (W7),
/// and the half an *edit* that moves a connection's bucket or provider also needs.
///
/// [`connect`] is additive by contract and only ever sees the def it is given, so nothing
/// else can take a store back out: without this, a forgotten bucket stays queryable until the
/// window is re-opened. `identity` is the connection's [`identity`](ConnectionDef::identity), from
/// which the key it went in under is composed the same way it was composed to register.
///
/// Silent about both ways it can do nothing, because neither is a fault: an identity no object
/// store answers to never registered anything, and a key with no store behind it is the ordinary
/// case for a connection that was refused.
pub fn disconnect(ctx: &SessionContext, identity: &str) {
    if let Some(url) = registration_url(identity) {
        let _ = ctx.deregister_object_store(url.as_ref());
    }
}

/// Every profile named in this machine's own AWS configuration, sorted — what the connection
/// editor's **Named profile** picker offers (W7 · 03, spec §6).
///
/// A *name*, and nothing else: this reads the section headers of `~/.aws/config` and
/// `~/.aws/credentials` and hands back the list. Nothing in a profile's body is read, kept or
/// shown, which is what keeps a discovery of the user's credentials free of their credentials.
///
/// Parsed by `aws-config` rather than by us, because the file's rules are not the ini rules they
/// look like: a profile is `[profile x]` in one file and `[x]` in the other, `AWS_CONFIG_FILE`
/// and `AWS_SHARED_CREDENTIALS_FILE` move both, and the two files merge. A hand-rolled reader
/// that got any of that wrong would offer a list the credential provider then disagrees with —
/// and this list exists precisely so the two agree.
///
/// **Empty means empty**, and a parse failure is empty too. There is nothing to report: the file
/// belongs to the user's own AWS setup rather than to Strata, and the editor says so where the
/// list would have been. What the *connection* does with a name it cannot resolve is
/// [`profile_credentials`]'s answer, which is the one that reaches a row.
pub async fn aws_profiles() -> Vec<String> {
    let profiles = aws_config::profile::load(&Fs::real(), &Env::real(), &Default::default(), None)
        .await
        .map(|set| set.profiles().map(str::to_string).collect::<Vec<_>>());
    let mut profiles = match profiles {
        Ok(profiles) => profiles,
        Err(e) => {
            tracing::debug!("no AWS profiles: {e}");
            return Vec::new();
        }
    };
    profiles.sort_unstable();
    profiles
}

/// Resolve a connection's client options into `object_store`'s own keys, in a stable order.
///
/// Unknown names are **skipped rather than refused** here, because [`connect`] has already run
/// [`check_client_config`] over the same map and this is the second half of one answer; a def that
/// somehow reached this point with one would have failed above.
fn client_options(conn: &ConnectionDef) -> Vec<(ClientConfigKey, String)> {
    conn.client_config
        .iter()
        .filter_map(|(name, value)| Some((name.parse().ok()?, value.trim().to_string())))
        .collect()
}

/// The store itself, per provider. Split from [`connect`] so the registration is one line
/// with one meaning: every way this can fail is a way of describing the connection wrong.
async fn build(conn: &ConnectionDef) -> Result<Arc<dyn ObjectStore>, String> {
    let bucket = conn.address.trim();
    match &conn.provider {
        Provider::S3(s3) => {
            let region = s3.region.trim();
            if region.is_empty() {
                return Err("This S3 connection needs a region.".into());
            }
            let mut builder = AmazonS3Builder::new()
                .with_bucket_name(bucket)
                .with_region(region);
            let endpoint = s3.endpoint.trim();
            if !endpoint.is_empty() {
                if endpoint.starts_with("http://") && !s3.allow_http {
                    return Err(format!(
                        "The endpoint '{endpoint}' is plain HTTP. Turn on 'Allow plain HTTP' for \
                         this connection, or give it an https endpoint."
                    ));
                }
                builder = builder
                    .with_endpoint(endpoint)
                    .with_allow_http(s3.allow_http);
            }
            builder = match &s3.auth {
                S3Auth::Anonymous => builder.with_skip_signature(true),
                S3Auth::Ambient => builder.with_credentials(ambient_credentials(region).await?),
                S3Auth::Profile { name } => {
                    let profile = name.trim();
                    if profile.is_empty() {
                        return Err("This S3 connection needs a profile name.".into());
                    }
                    builder.with_credentials(profile_credentials(region, profile).await?)
                }
            };
            for (key, value) in client_options(conn) {
                builder = builder.with_config(AmazonS3ConfigKey::Client(key), value);
            }
            builder
                .build()
                .map(|s| Arc::new(s) as Arc<dyn ObjectStore>)
                .map_err(|e| format!("Cannot reach '{}': {e}", conn.named()))
        }
        Provider::Gcs(gcs) => {
            let mut builder = GoogleCloudStorageBuilder::new().with_bucket_name(bucket);
            builder = match &gcs.auth {
                GcsAuth::Ambient => builder,
                GcsAuth::ServiceAccount { path } => {
                    let path = path.trim();
                    if path.is_empty() {
                        return Err("This GCS connection needs a service-account file.".into());
                    }
                    builder.with_service_account_path(path)
                }
                GcsAuth::Anonymous => builder.with_skip_signature(true),
            };
            for (key, value) in client_options(conn) {
                builder = builder.with_config(GoogleConfigKey::Client(key), value);
            }
            builder
                .build()
                .map(|s| Arc::new(s) as Arc<dyn ObjectStore>)
                .map_err(|e| format!("Cannot reach '{}': {e}", conn.named()))
        }
        Provider::Http => {
            let insecure = conn.address.trim().starts_with("http://");
            let mut builder = HttpBuilder::new()
                .with_url(bucket)
                .with_config(ClientConfigKey::AllowHttp, insecure.to_string());
            for (key, value) in client_options(conn) {
                builder = builder.with_config(key, value);
            }
            builder
                .build()
                .map(|s| Arc::new(s) as Arc<dyn ObjectStore>)
                .map_err(|e| format!("Cannot reach '{}': {e}", conn.named()))
        }
        Provider::Source(_) => Err(format!(
            "'{}' is a data source, not an object store.",
            conn.named()
        )),
    }
}

/// **Ambient**: the host's whole chain, whatever answers first — environment, then the
/// `~/.aws` profiles, SSO, `credential_process`, web identity, ECS, IMDS.
///
/// This is `aws-config`'s own `DefaultCredentialsChain`, and that is the point of the
/// dependency. `object_store` alone is **env-only**: `AmazonS3Builder::from_env` reads the
/// `AWS_*` variables plus IMDS / ECS / web-identity, and stops. It does not parse
/// `~/.aws/config`, does not do SSO, and ignores `AWS_PROFILE` — so "log in the way you
/// already do" is not something it can express.
///
/// The region is handed to the SDK as well as to the store builder: a profile may name a
/// different one, and the connection's own region is the answer the user gave.
async fn ambient_credentials(region: &str) -> Result<AwsCredentialProvider, String> {
    let config = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(region.to_string()))
        .load()
        .await;
    let provider = config
        .credentials_provider()
        .ok_or_else(|| named(None, "resolved no credentials.".into()))?;
    probe(provider, None).await
}

/// **A named profile, and only that profile.**
///
/// Built from [`ProfileFileCredentialsProvider`] directly rather than by naming a profile on
/// the default chain, and the difference is the whole behaviour. `ConfigLoader::profile_name`
/// configures the chain's *Profile arm*; it does not move that arm to the front, and
/// `DefaultCredentialsChain::build` is unconditionally `Environment → Profile → WebIdentity →
/// ECS → IMDS`. So a Strata launched from a shell exporting `AWS_ACCESS_KEY_ID` signed as the
/// *environment* identity while the row showed the profile the user had chosen — Ambient and
/// Profile were the same connection wherever ambient credentials existed, and a misspelled
/// profile name still registered green, which is precisely the state the probe below exists to
/// catch. Selecting a profile has to mean that profile is the only thing consulted.
///
/// This provider still resolves the profile's *own* mechanism — `source_profile` chains,
/// `role_arn`, `sso_session`, `credential_process` — because that is what the profile says to
/// do. What it will not do is fall back to somebody else's identity.
async fn profile_credentials(region: &str, profile: &str) -> Result<AwsCredentialProvider, String> {
    let config =
        ProviderConfig::without_region().with_region(Some(Region::new(region.to_string())));
    let provider = ProfileFileCredentialsProvider::builder()
        .profile_name(profile)
        .configure(&config)
        .build();
    probe(SharedCredentialsProvider::new(provider), Some(profile)).await
}

/// Resolve `provider` once, throw the answer away, and hand back the bridge that will resolve
/// it again per request.
///
/// The probe is what [`connect`] turns into the pane's status: a chain that answers nothing is
/// the amber "needs credentials" state, and saying so here is the difference between one
/// accurate row and a broken table per def. The resolved credential is **not** kept —
/// short-lived credentials (SSO, assumed roles, IMDS) expire in minutes, and
/// [`SdkCredentials`] is what keeps asking.
async fn probe(
    provider: SharedCredentialsProvider,
    profile: Option<&str>,
) -> Result<AwsCredentialProvider, String> {
    provider
        .provide_credentials()
        .await
        .map_err(|e| named(profile, format!("resolved no credentials: {e}")))?;
    Ok(Arc::new(SdkCredentials { provider }))
}

/// A credential-chain failure, saying **which** chain — the named profile is the user's own
/// word and is most of the diagnosis when it is the thing that is wrong (a profile that isn't
/// in `~/.aws/config`, or an SSO session that has expired).
fn named(profile: Option<&str>, why: String) -> String {
    match profile {
        Some(p) => format!("The AWS profile '{p}' {why}"),
        None => format!("The AWS credential chain {why}"),
    }
}

/// The bridge itself: an `object_store` credential provider backed by the AWS SDK's.
///
/// Resolution happens **per request**, not once at build — that is the whole point of wrapping
/// the provider rather than copying a key out of it. Short-lived credentials (SSO, assumed
/// roles, IMDS) expire in minutes, and the SDK's provider is the thing that knows how to
/// refresh them.
#[derive(Debug)]
struct SdkCredentials {
    provider: SharedCredentialsProvider,
}

#[async_trait::async_trait]
impl CredentialProvider for SdkCredentials {
    type Credential = AwsCredential;

    async fn get_credential(&self) -> object_store::Result<Arc<AwsCredential>> {
        let creds = self
            .provider
            .provide_credentials()
            .await
            .map_err(|e| Error::Generic {
                store: "S3",
                source: Box::new(e),
            })?;
        Ok(Arc::new(AwsCredential {
            key_id: creds.access_key_id().to_string(),
            secret_key: creds.secret_access_key().to_string(),
            token: creds.session_token().map(str::to_string),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::process;
    use std::thread;

    use datafusion::datasource::listing::ListingTableUrl;
    use strata_model::{GcsStore, S3Store, SourceDef};

    /// **What a connection's paths hang off, per provider** — and the one arm where the identity
    /// is not a bucket with a scheme bolted on.
    ///
    /// An S3 or GCS address is a bare bucket, so its identity reads as `kind:bucket` and the
    /// prefix puts the provider's own scheme in front. An HTTP address is a **whole origin**, so
    /// its identity is a scheme in front of a URL that already has one — and the prefix has to
    /// take the address back out rather than compose anything. Handing the identity itself to a
    /// store builder produced `http:https://files.example.com`, which is the shape this pins.
    ///
    /// A source holds relations rather than files, so it has no prefix at all: `None` here is
    /// what makes `table_spec` compose nothing remote for one.
    #[test]
    fn a_connections_store_prefix_is_what_its_paths_hang_off() {
        let prefix = |conn: &ConnectionDef| store_prefix(&conn.identity());
        assert_eq!(
            prefix(&s3("acme-lake", S3Store::default())).as_deref(),
            Some("s3://acme-lake")
        );
        assert_eq!(
            prefix(&ConnectionDef {
                address: "acme-lake".into(),
                name: String::new(),
                provider: Provider::Gcs(GcsStore::default()),
                client_config: Default::default(),
            })
            .as_deref(),
            Some("gs://acme-lake")
        );
        for origin in ["https://files.example.com", "http://127.0.0.1:9000"] {
            assert_eq!(
                prefix(&ConnectionDef {
                    address: origin.into(),
                    name: String::new(),
                    provider: Provider::Http,
                    client_config: Default::default(),
                })
                .as_deref(),
                Some(origin),
                "an HTTP address is already a URL, and the prefix is that URL"
            );
        }
        assert_eq!(
            prefix(&ConnectionDef {
                address: "db:5432/analytics".into(),
                name: String::new(),
                provider: Provider::Source(SourceDef {
                    kind: "postgres".into(),
                    ..Default::default()
                }),
                client_config: Default::default(),
            }),
            None,
            "a source holds relations, so no path composes onto it"
        );
    }

    fn s3(bucket: &str, store: S3Store) -> ConnectionDef {
        ConnectionDef {
            address: bucket.into(),
            name: String::new(),
            provider: Provider::S3(store),
            client_config: Default::default(),
        }
    }

    /// Whether a source path under `url` resolves to a registered store — asked exactly the
    /// way a table's registration asks it, through the `ListingTableUrl` a source path becomes.
    fn reaches(ctx: &SessionContext, source: &str) -> bool {
        let url = ListingTableUrl::parse(source).expect("a source url");
        ctx.runtime_env().object_store(&url).is_ok()
    }

    /// [`connect`] **minus the bucket probe** — every local judgement, and the registration.
    ///
    /// The tests below ask what a def registers *under*: that `s3://acme-lake` lands where a
    /// source path beneath it will look, that a second connect replaces rather than stacks, that
    /// a refused def leaves nothing behind. Those are questions about naming and keying, and none
    /// of them is improved by the bucket existing — while `connect` itself now asks a real server
    /// whether it does. Routing them through the full call would make this suite dial out to AWS
    /// on every run, invent buckets it does not own, and fail on a plane.
    ///
    /// So the network half is **MinIO's to test**, in `tests/object_store_minio.rs`, against a
    /// bucket that is really there — which is where the rest of `connect`'s remote behaviour has
    /// always been checked. What stays here is everything that can be settled without a server.
    /// It settles through [`settle`], the same call `connect` makes, so the all-or-nothing
    /// contract this suite asserts is the real one and not a restatement of it.
    async fn connect_unprobed(ctx: &SessionContext, conn: &ConnectionDef) -> Result<(), String> {
        settle(ctx, conn, prepare(conn).await)
    }

    /// The three arms that need no credential chain register without one — and registering is
    /// per bucket, under the key the registry looks a source path up by.
    #[tokio::test]
    async fn a_secret_free_connection_registers_under_its_own_bucket() {
        let ctx = SessionContext::new();
        for conn in [
            s3(
                "acme-lake",
                S3Store {
                    region: "eu-west-2".into(),
                    auth: S3Auth::Anonymous,
                    ..Default::default()
                },
            ),
            ConnectionDef {
                address: "public-lake".into(),
                name: String::new(),
                provider: Provider::Gcs(GcsStore {
                    auth: GcsAuth::Anonymous,
                }),
                client_config: Default::default(),
            },
            ConnectionDef {
                address: "http://aserver:8484".into(),
                name: String::new(),
                provider: Provider::Http,
                client_config: Default::default(),
            },
        ] {
            connect_unprobed(&ctx, &conn).await.expect("registers");
            let source = format!(
                "{}/data/x.parquet",
                store_prefix(&conn.identity()).expect("an object store's own prefix")
            );
            assert!(reaches(&ctx, &source), "{source}");
        }
        assert!(!reaches(&ctx, "s3://other-lake/x.parquet"));
    }

    /// **A named profile signs as that profile, and Ambient signs as the host's chain** — the
    /// one bespoke piece here, the reason the `aws-config` dependency exists, and the rule that
    /// was wrong first time round: naming a profile on the default chain only configures its
    /// *Profile arm*, which sits behind `Environment`, so an exported `AWS_ACCESS_KEY_ID` used
    /// to win and the chosen profile was never read.
    ///
    /// Hermetic in the only way this can be: the AWS chain is configured by process
    /// environment, so the test sets all of it — the profile file it may read, and ambient
    /// credentials that are *deliberately different*, so "the profile won" and "the environment
    /// won" cannot be confused. `AWS_SESSION_TOKEN` is set blank (which the SDK reads as
    /// absent) rather than left alone, or a developer running this from a shell with session
    /// credentials exported would fail the token assertion — and `assert_eq!` would print their
    /// real token.
    ///
    /// **One test owns the AWS environment for this binary.** These are process-global and
    /// unsynchronised; nothing else in the crate reads an `AWS_*` variable (the other store
    /// tests are anonymous, GCS or HTTP, none of which consult a chain), and they are set
    /// rather than removed, so there is no window in which a concurrent test could see them
    /// disappear.
    ///
    /// It asserts the credential, not just that `connect` returned `Ok`: the bridge's whole job
    /// is carrying the SDK's answer across, and a provider that resolved and then handed over
    /// the wrong fields would sign every request with nothing and fail at the bucket.
    #[tokio::test]
    async fn a_named_profile_signs_as_that_profile_and_not_as_the_environment() {
        let dir = std::env::temp_dir().join(format!("strata-store-profile-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let credentials = dir.join("credentials");
        fs::write(
            &credentials,
            "[readonly]\naws_access_key_id = AKIAPROFILE\naws_secret_access_key = profile-secret\n",
        )
        .unwrap();
        std::env::set_var("AWS_SHARED_CREDENTIALS_FILE", &credentials);
        std::env::set_var("AWS_CONFIG_FILE", dir.join("config"));
        std::env::set_var("AWS_ACCESS_KEY_ID", "AKIAAMBIENT");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "ambient-secret");
        std::env::set_var("AWS_SESSION_TOKEN", "");

        let ambient = ambient_credentials("eu-west-2")
            .await
            .expect("the ambient chain resolves");
        let credential = ambient.get_credential().await.expect("a credential");
        assert_eq!(credential.key_id, "AKIAAMBIENT");
        assert_eq!(credential.secret_key, "ambient-secret");
        assert_eq!(credential.token, None);

        let named = profile_credentials("eu-west-2", "readonly")
            .await
            .expect("the named profile resolves");
        let credential = named.get_credential().await.expect("a credential");
        assert_eq!(credential.key_id, "AKIAPROFILE");
        assert_eq!(credential.secret_key, "profile-secret");

        let missing = profile_credentials("eu-west-2", "no-such-profile")
            .await
            .expect_err("refused");
        assert!(missing.contains("no-such-profile"), "{missing}");

        let ctx = SessionContext::new();
        connect_unprobed(
            &ctx,
            &s3(
                "signed-lake",
                S3Store {
                    region: "eu-west-2".into(),
                    auth: S3Auth::Profile {
                        name: "readonly".into(),
                    },
                    ..Default::default()
                },
            ),
        )
        .await
        .expect("registers");
        assert!(reaches(&ctx, "s3://signed-lake/x.parquet"));

        let _ = fs::remove_dir_all(&dir);
    }

    /// **A refused connection leaves no store behind, including one an earlier pass
    /// registered.** Otherwise a `Reg::Failed` row would sit over a bucket the engine still
    /// answers for — the "both refused and live" state `connect`'s contract exists to prevent,
    /// and the one a re-scan after an edit would produce.
    #[tokio::test]
    async fn a_failed_reconnect_deregisters_what_the_last_one_registered() {
        let ctx = SessionContext::new();
        let good = s3(
            "acme-lake",
            S3Store {
                region: "eu-west-2".into(),
                auth: S3Auth::Anonymous,
                ..Default::default()
            },
        );
        connect_unprobed(&ctx, &good).await.expect("registers");
        assert!(reaches(&ctx, "s3://acme-lake/x.parquet"));

        let broken = s3(
            "acme-lake",
            S3Store {
                region: String::new(),
                auth: S3Auth::Anonymous,
                ..Default::default()
            },
        );
        connect_unprobed(&ctx, &broken).await.expect_err("refused");
        assert!(
            !reaches(&ctx, "s3://acme-lake/x.parquet"),
            "the previous store must not outlive the def that registered it"
        );
    }

    /// An S3-compatible store (R2 / MinIO) is the same provider with an endpoint, not a
    /// provider of its own — including the plain-http case a workstation MinIO needs.
    #[tokio::test]
    async fn an_s3_compatible_endpoint_is_the_s3_provider() {
        let ctx = SessionContext::new();
        let conn = s3(
            "local-lake",
            S3Store {
                region: "us-east-1".into(),
                auth: S3Auth::Anonymous,
                endpoint: "http://127.0.0.1:9000".into(),
                allow_http: true,
            },
        );
        connect_unprobed(&ctx, &conn).await.expect("registers");
        assert!(reaches(&ctx, "s3://local-lake/x.parquet"));
    }

    /// **A def that cannot say where to read from is refused, and says which field is
    /// missing** — never defaulted. The region case is the sharp one: `AmazonS3Builder` would
    /// happily assume `us-east-1` and then read the wrong endpoint (arrow-rs#2795).
    #[tokio::test]
    async fn a_def_missing_what_it_needs_is_refused_by_name() {
        let ctx = SessionContext::new();
        let cases = [
            (
                s3(
                    "acme-lake",
                    S3Store {
                        region: "  ".into(),
                        auth: S3Auth::Anonymous,
                        ..Default::default()
                    },
                ),
                "region",
            ),
            (
                s3(
                    "acme-lake",
                    S3Store {
                        region: "eu-west-2".into(),
                        auth: S3Auth::Profile { name: " ".into() },
                        ..Default::default()
                    },
                ),
                "profile name",
            ),
            (
                ConnectionDef {
                    address: "lake".into(),
                    name: String::new(),
                    provider: Provider::Gcs(GcsStore {
                        auth: GcsAuth::ServiceAccount {
                            path: String::new(),
                        },
                    }),
                    client_config: Default::default(),
                },
                "service-account file",
            ),
            (
                ConnectionDef {
                    address: "   ".into(),
                    name: String::new(),
                    provider: Provider::Http,
                    client_config: Default::default(),
                },
                "spaces",
            ),
            (
                ConnectionDef {
                    address: "aserver:8484".into(),
                    name: String::new(),
                    provider: Provider::Http,
                    client_config: Default::default(),
                },
                "scheme",
            ),
        ];
        for (conn, wanted) in cases {
            let e = connect_unprobed(&ctx, &conn).await.expect_err("refused");
            assert!(e.contains(wanted), "{e}");
        }
    }

    /// A client option is refused for the two things `object_store` would not report: a name it
    /// has never heard of (dropped on the floor at build time) and a blank value (handed to a
    /// parser expecting a duration). Both name the key.
    #[tokio::test]
    async fn a_client_option_it_cannot_use_is_refused_by_name() {
        let ctx = SessionContext::new();
        for (config, wanted) in [
            ([("nonsense", "1")], "'nonsense' is not a client option"),
            ([("timeout", "  ")], "'timeout' has no value"),
        ] {
            let conn = ConnectionDef {
                client_config: config
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                ..s3(
                    "acme-lake",
                    S3Store {
                        region: "eu-west-2".into(),
                        auth: S3Auth::Anonymous,
                        ..Default::default()
                    },
                )
            };
            let e = connect_unprobed(&ctx, &conn).await.expect_err("refused");
            assert!(e.contains(wanted), "{e}");
        }
    }

    /// …and one it *can* use reaches the store. Asserted through a successful registration rather
    /// than by reading the client back (`ClientOptions` exposes nothing): the value is parsed by
    /// `object_store` at build, so a duration it could not read would fail the build here.
    #[tokio::test]
    async fn a_client_option_is_applied_to_the_store_it_builds() {
        let ctx = SessionContext::new();
        let conn = ConnectionDef {
            client_config: [("timeout", "45s"), ("user_agent", "strata-test")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..s3(
                "acme-lake",
                S3Store {
                    region: "eu-west-2".into(),
                    auth: S3Auth::Anonymous,
                    ..Default::default()
                },
            )
        };
        connect_unprobed(&ctx, &conn).await.expect("registers");
        assert!(reaches(&ctx, "s3://acme-lake/data/x.parquet"));
    }

    /// **A plain-`http` S3 endpoint without the toggle is refused by name.** Left to
    /// `object_store` it is not refused at all until the first request, and then only as
    /// reqwest's "builder error" — no host, no scheme, nothing the user can act on — because the
    /// client is built `https_only(!allow_http)`. The message has to name the control.
    #[tokio::test]
    async fn a_plain_http_endpoint_without_the_toggle_is_refused_by_name() {
        let ctx = SessionContext::new();
        let conn = |allow_http| {
            s3(
                "acme-lake",
                S3Store {
                    region: "eu-west-2".into(),
                    auth: S3Auth::Anonymous,
                    endpoint: "http://localhost:9000".into(),
                    allow_http,
                },
            )
        };
        let e = connect_unprobed(&ctx, &conn(false))
            .await
            .expect_err("refused");
        assert!(
            e.contains("plain HTTP") && e.contains("Allow plain HTTP"),
            "{e}"
        );
        connect_unprobed(&ctx, &conn(true))
            .await
            .expect("registers");
        let mut secure = conn(false);
        if let Provider::S3(s3) = &mut secure.provider {
            s3.endpoint = "https://s3.example.net".into();
        }
        connect_unprobed(&ctx, &secure).await.expect("registers");
    }

    /// An address is scheme + authority, so a path in one is a def that would register under a
    /// key nothing ever looks up. Refused at the connection, where the user can fix it — rather
    /// than silently, leaving every table over it reporting no object store.
    ///
    /// Both providers that can carry one, because they carry it differently: an HTTP address is
    /// the whole URL, so its path comes after the origin, while a bucket name simply may not
    /// contain a slash at all.
    #[tokio::test]
    async fn an_address_carrying_a_path_is_refused() {
        let ctx = SessionContext::new();
        for (conn, quoted) in [
            (
                ConnectionDef {
                    address: "https://aserver:8484/fake".into(),
                    name: String::new(),
                    provider: Provider::Http,
                    client_config: Default::default(),
                },
                "'/fake'",
            ),
            (
                ConnectionDef {
                    address: "acme-lake/data".into(),
                    name: String::new(),
                    provider: Provider::S3(S3Store {
                        region: "eu-west-2".into(),
                        ..Default::default()
                    }),
                    client_config: Default::default(),
                },
                "lowercase letters",
            ),
        ] {
            let e = connect_unprobed(&ctx, &conn).await.expect_err("refused");
            assert!(e.contains(quoted), "{e}");
        }
    }

    /// Re-connecting replaces, rather than stacking or failing — a re-scan re-runs the whole
    /// pass, and an edited connection has to be able to take over its own bucket.
    #[tokio::test]
    async fn connecting_twice_replaces_the_registered_store() {
        let ctx = SessionContext::new();
        let conn = s3(
            "acme-lake",
            S3Store {
                region: "eu-west-2".into(),
                auth: S3Auth::Anonymous,
                ..Default::default()
            },
        );
        connect_unprobed(&ctx, &conn).await.expect("registers");
        connect_unprobed(&ctx, &conn)
            .await
            .expect("registers again");
        assert!(reaches(&ctx, "s3://acme-lake/x.parquet"));
    }

    /// A server answering every request with a redirect that carries no `Location` header —
    /// which is exactly what S3 does to a cross-region request, and the only way to reach
    /// [`is_bare_redirect`] without owning two buckets in two regions.
    ///
    /// Loopback, on a port the OS picks, serving from a thread that lives as long as the
    /// process. This is not the suite dialling out: nothing leaves the machine, and the
    /// listener is the test's own.
    fn bare_redirect_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("an address").port();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.read(&mut [0; 1024]);
                let _ = stream
                    .write_all(b"HTTP/1.1 301 Moved Permanently\r\nContent-Length: 0\r\n\r\n");
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// **The wrong-region refusal, pinned to behaviour rather than to a sentence.**
    ///
    /// [`is_bare_redirect`] matches a literal out of `object_store`'s error prose, because the
    /// crate routes S3 list failures into `Generic` and offers nothing structured to ask. That
    /// makes the refusal one dependency bump away from silently reverting — a mistyped region
    /// would register green again and every table under it would fail on `object_store`'s own
    /// bare-redirect message, with the rest of this suite still passing.
    ///
    /// So this drives the real path: `connect`, against a server that answers the way a
    /// cross-region S3 does, asserting the refusal is **ours** and names the region. A reworded
    /// upstream message fails here instead of in a user's project.
    #[tokio::test]
    async fn a_bare_redirect_is_refused_as_a_wrong_region() {
        let ctx = SessionContext::new();
        let conn = s3(
            "acme-lake",
            S3Store {
                region: "eu-west-2".into(),
                auth: S3Auth::Anonymous,
                endpoint: bare_redirect_server(),
                allow_http: true,
            },
        );
        let e = connect(&ctx, &conn).await.expect_err("refused");
        assert!(e.contains("'eu-west-2'"), "{e}");
        assert!(e.contains("'acme-lake'"), "{e}");
        assert!(!reaches(&ctx, "s3://acme-lake/x.parquet"));
    }
}
