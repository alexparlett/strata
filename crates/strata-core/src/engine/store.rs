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
//! **Nothing in this module reads, writes or holds a secret.** Every arm resolves through
//! the host's own provider chain, a named profile, a key **file** the OS already lets the
//! user read, or not at all (anonymous). The one place a credential value exists is inside
//! [`SdkCredentials::get_credential`], for the length of one signed request.

use std::sync::Arc;

use aws_config::profile::ProfileFileCredentialsProvider;
use aws_config::provider_config::ProviderConfig;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::prelude::*;
use object_store::aws::{AmazonS3Builder, AwsCredential, AwsCredentialProvider};
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::http::HttpBuilder;
use object_store::{CredentialProvider, ObjectStore};

use strata_model::{ConnectionDef, GcsAuth, Provider, S3Auth};

/// Build the object store `conn` describes and register it on `ctx`, so tables over its
/// bucket can be registered and scanned.
///
/// **All or nothing: on `Err`, nothing is registered for this bucket** — including anything an
/// earlier pass registered, which is deregistered here rather than left behind. That is the
/// same contract [`register_external`](super::catalog::register_external) keeps for a table
/// (it deregisters before it re-infers, so a failed re-scan leaves the table absent), and it
/// is what makes the outcome foldable onto a single `Reg` row: a connection cannot be both
/// refused and live. Leaving the old store would produce exactly that — a row reading `Failed`
/// over a bucket the engine still answers for.
///
/// So the credential chain is probed *before* the store goes in, not after. That probe is
/// deliberate, and it is what the pane's status dot is made of. Without it a connection with
/// no usable credentials registers perfectly happily and the diagnosis lands somewhere else
/// entirely — on every table over the bucket, one opaque signing error each, rather than once
/// on the thing that is actually wrong. It resolves the chain **once** and throws the answer
/// away; the provider installed on the store resolves per request, so credentials that rotate
/// (or an `aws sso login` in another terminal) keep working without anything here being
/// re-run.
///
/// Idempotent: registering over an existing key replaces it, which is what a re-scan wants.
///
/// One thing it cannot clean up, because it is not told about it: a connection whose **bucket**
/// was edited leaves the store registered under the *old* URL. That is an edit, and the edit
/// gesture owns it (Connections 03) — nothing here ever sees the def it replaced.
pub async fn connect(ctx: &SessionContext, conn: &ConnectionDef) -> Result<(), String> {
    if conn.bucket.trim().is_empty() {
        return Err("This connection has no bucket.".into());
    }
    let url = ObjectStoreUrl::parse(conn.url()).map_err(|e| {
        format!(
            "'{}' is not a bucket Strata can register: {e}",
            conn.bucket.trim()
        )
    })?;
    match build(conn).await {
        Ok(store) => {
            ctx.register_object_store(url.as_ref(), store);
            Ok(())
        }
        Err(why) => {
            // Errs when nothing was registered under this key, which is the ordinary case
            // (a first pass, or a def that has never worked) and not a failure of its own.
            let _ = ctx.deregister_object_store(url.as_ref());
            Err(why)
        }
    }
}

/// Forget the object store registered under `url` — the Forget gesture's engine half (W7),
/// and the half an *edit* that moves a connection's bucket or provider also needs.
///
/// [`connect`] is additive by contract and only ever sees the def it is given, so nothing
/// else can take a store back out: without this, a forgotten bucket stays queryable until the
/// window is re-opened. `url` is the connection's [`ConnectionDef::url`] — the key it went in
/// under, and the only key the registry answers to.
///
/// Silent about both ways it can do nothing, because neither is a fault: a URL that does not
/// parse never registered anything, and a key with no store behind it is the ordinary case
/// for a connection that was refused.
pub fn disconnect(ctx: &SessionContext, url: &str) {
    if let Ok(url) = ObjectStoreUrl::parse(url) {
        let _ = ctx.deregister_object_store(url.as_ref());
    }
}

/// The store itself, per provider. Split from [`connect`] so the registration is one line
/// with one meaning: every way this can fail is a way of describing the connection wrong.
async fn build(conn: &ConnectionDef) -> Result<Arc<dyn ObjectStore>, String> {
    let bucket = conn.bucket.trim();
    match &conn.provider {
        Provider::S3(s3) => {
            // Region is not optional and cannot be inferred: `AmazonS3Builder` silently
            // defaults to `us-east-1` (arrow-rs#2795), which resolves to a real endpoint
            // serving a different bucket's worth of nothing. Refused rather than defaulted.
            let region = s3.region.trim();
            if region.is_empty() {
                return Err("This S3 connection needs a region.".into());
            }
            let mut builder = AmazonS3Builder::new()
                .with_bucket_name(bucket)
                .with_region(region);
            let endpoint = s3.endpoint.trim();
            if !endpoint.is_empty() {
                builder = builder
                    .with_endpoint(endpoint)
                    .with_allow_http(s3.allow_http);
            }
            builder = match &s3.auth {
                // No chain at all — unsigned requests against a public bucket.
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
            builder
                .build()
                .map(|s| Arc::new(s) as Arc<dyn ObjectStore>)
                .map_err(|e| format!("Cannot reach '{}': {e}", conn.url()))
        }
        Provider::Gcs(gcs) => {
            let mut builder = GoogleCloudStorageBuilder::new().with_bucket_name(bucket);
            builder = match &gcs.auth {
                // Application Default Credentials, which the builder resolves itself: the
                // `GOOGLE_APPLICATION_CREDENTIALS` file, then the gcloud ADC file, then the
                // GCE/GKE metadata server. The last of those is installed without a request,
                // so an ambient GCS connection on a machine with no credentials at all
                // registers cleanly and fails at read time — the one arm here whose status
                // cannot be known without asking the bucket, and it is not worth a request.
                GcsAuth::Ambient => builder,
                GcsAuth::ServiceAccount { path } => {
                    let path = path.trim();
                    if path.is_empty() {
                        return Err("This GCS connection needs a service-account file.".into());
                    }
                    // The **path**. `with_service_account_key` takes inline JSON and is the
                    // one call in this module's vocabulary that would put a private key in
                    // our hands; it is not used anywhere and must not be.
                    builder.with_service_account_path(path)
                }
                GcsAuth::Anonymous => builder.with_skip_signature(true),
            };
            builder
                .build()
                .map(|s| Arc::new(s) as Arc<dyn ObjectStore>)
                .map_err(|e| format!("Cannot reach '{}': {e}", conn.url()))
        }
        // A public origin: no credentials to resolve, and nothing to get wrong but the URL.
        Provider::Http => HttpBuilder::new()
            .with_url(conn.url())
            .build()
            .map(|s| Arc::new(s) as Arc<dyn ObjectStore>)
            .map_err(|e| format!("Cannot reach '{}': {e}", conn.url())),
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
        // `Region` needs an owned, 'static string; the borrow is the caller's field.
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
    // The documented way to give a standalone provider what it needs for the profiles that
    // make network calls (SSO, assume-role): a `ProviderConfig` carrying the region, with the
    // default HTTPS client and sleep implementation left to resolve themselves.
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
        let creds = self.provider.provide_credentials().await.map_err(|e| {
            object_store::Error::Generic {
                store: "S3",
                source: Box::new(e),
            }
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
    use std::process;

    use datafusion::datasource::listing::ListingTableUrl;
    use strata_model::{GcsStore, S3Store};

    fn s3(bucket: &str, store: S3Store) -> ConnectionDef {
        ConnectionDef {
            bucket: bucket.into(),
            provider: Provider::S3(store),
        }
    }

    /// Whether a source path under `url` resolves to a registered store — asked exactly the
    /// way a table's registration asks it, through the `ListingTableUrl` a source path becomes.
    fn reaches(ctx: &SessionContext, source: &str) -> bool {
        let url = ListingTableUrl::parse(source).expect("a source url");
        ctx.runtime_env().object_store(&url).is_ok()
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
                bucket: "public-lake".into(),
                provider: Provider::Gcs(GcsStore {
                    auth: GcsAuth::Anonymous,
                }),
            },
            ConnectionDef {
                bucket: "example.com".into(),
                provider: Provider::Http,
            },
        ] {
            connect(&ctx, &conn).await.expect("registers");
            // The whole contract: a source path under the bucket now resolves to a store.
            let source = format!("{}/data/x.parquet", conn.url());
            assert!(reaches(&ctx, &source), "{source}");
        }
        // …and only that bucket. A neighbouring one is still unreachable, which is what makes
        // the def's identity meaningful.
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

        // Ambient is the host's whole chain, and the environment is its first arm.
        let ambient = ambient_credentials("eu-west-2")
            .await
            .expect("the ambient chain resolves");
        let credential = ambient.get_credential().await.expect("a credential");
        assert_eq!(credential.key_id, "AKIAAMBIENT");
        assert_eq!(credential.secret_key, "ambient-secret");
        assert_eq!(credential.token, None);

        // The profile is that profile — *even though* the environment would have answered, and
        // answered with something else. This is the assertion the original code failed.
        let named = profile_credentials("eu-west-2", "readonly")
            .await
            .expect("the named profile resolves");
        let credential = named.get_credential().await.expect("a credential");
        assert_eq!(credential.key_id, "AKIAPROFILE");
        assert_eq!(credential.secret_key, "profile-secret");

        // A profile that is not in the file is refused, rather than falling back to the
        // environment and registering green — the whole point of the probe.
        let missing = profile_credentials("eu-west-2", "no-such-profile")
            .await
            .expect_err("refused");
        assert!(missing.contains("no-such-profile"), "{missing}");

        // …and a connection over a resolving chain registers like any other.
        let ctx = SessionContext::new();
        connect(
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
        connect(&ctx, &good).await.expect("registers");
        assert!(reaches(&ctx, "s3://acme-lake/x.parquet"));

        // The same bucket, now described by a def that cannot build a store.
        let broken = s3(
            "acme-lake",
            S3Store {
                region: String::new(),
                auth: S3Auth::Anonymous,
                ..Default::default()
            },
        );
        connect(&ctx, &broken).await.expect_err("refused");
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
        connect(&ctx, &conn).await.expect("registers");
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
                    bucket: "lake".into(),
                    provider: Provider::Gcs(GcsStore {
                        auth: GcsAuth::ServiceAccount { path: "".into() },
                    }),
                },
                "service-account file",
            ),
            (
                ConnectionDef {
                    bucket: "   ".into(),
                    provider: Provider::Http,
                },
                "bucket",
            ),
        ];
        for (conn, wanted) in cases {
            let e = connect(&ctx, &conn).await.expect_err("refused");
            assert!(e.contains(wanted), "{e}");
        }
    }

    /// A bucket is scheme + authority, so a path in it is a def that would register under a
    /// key nothing ever looks up. Refused at the connection, where the user can fix it —
    /// rather than silently, leaving every table over it reporting no object store.
    #[tokio::test]
    async fn a_bucket_carrying_a_path_is_refused() {
        let ctx = SessionContext::new();
        let e = connect(
            &ctx,
            &ConnectionDef {
                bucket: "example.com/data".into(),
                provider: Provider::Http,
            },
        )
        .await
        .expect_err("refused");
        assert!(e.contains("example.com/data"), "{e}");
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
        connect(&ctx, &conn).await.expect("registers");
        connect(&ctx, &conn).await.expect("registers again");
        assert!(reaches(&ctx, "s3://acme-lake/x.parquet"));
    }
}
