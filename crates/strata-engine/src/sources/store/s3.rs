//! **S3, and anything that speaks it.** The bucket's store, the four ways one is authorised, and
//! the credential bridge that keeps it signed.
//!
//! The one registrant with a credential chain of its own, which is the whole reason `aws-config`
//! is a dependency: `object_store` alone reads the `AWS_*` variables and stops, so "log in the way
//! you already do" — a profile, SSO, `credential_process`, an assumed role — is not something it
//! can express.
//!
//! **Authorisation is a declared setting, not four kinds.** Ambient, a named profile, static keys
//! and anonymous are four ways of *building* one store over one bucket, and the difference lives
//! in [`connect`](S3::connect)'s own body. Making them four registrants would give one bucket four
//! identities, which the upsert clash check would then permit — and all four compose the same
//! `s3://bucket` registration URL, so they would silently displace each other's store.
//!
//! Only the static-keys mode holds a secret, and it holds it the way every secret is held:
//! [`Field::Secret`] keys whose values live in this machine's keystore and reach `connect` through
//! a [`SecretRequest`], never through the def.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use aws_config::profile::ProfileFileCredentialsProvider;
use aws_config::provider_config::ProviderConfig;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use aws_types::os_shim_internal::{Env, Fs};
use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey, AwsCredential, AwsCredentialProvider};
use object_store::{CredentialProvider, Error};

use strata_core::secret::Secret;
use strata_model::SourceDef;

use super::{built, check_bucket, client_options, client_settings, probe as reachable};
use crate::secrets::SecretProvider;
use crate::sources::secret_slot;
use crate::sources::source::{
    DataSource, Field, SourceKind, SourceMode, SourceSetting, Sourced, When,
};

/// How this bucket is signed. libpq's `sslmode` shape: the source's own words, handed to its own
/// `connect` body, and the credential rows below hang off whichever is chosen.
pub const AUTH: &[&str] = &["ambient", "profile", "keys", "anonymous"];
/// The keys whose rows only mean something under `keys`.
const STATIC: &[&str] = &["keys"];
/// The key a static-credentials secret is filed under, in the keystore and the def's expectation
/// set.
pub const SECRET_KEY: &str = "secret_access_key";
pub const SESSION_TOKEN: &str = "session_token";
/// What a static-credentials data source reads from the environment when this machine's keystore
/// holds nothing — AWS's own conventions, stated here because they are this source's vocabulary.
const SECRET_ENV: &[&str] = &["AWS_SECRET_ACCESS_KEY"];
const TOKEN_ENV: &[&str] = &["AWS_SESSION_TOKEN"];

const BUCKET: Option<&str> = Some("BUCKET");
const AUTH_GROUP: Option<&str> = Some("AUTHENTICATION");

/// What an S3 data source is described by, beyond the client options every store shares.
const OWN: &[SourceSetting] = &[
    SourceSetting {
        key: "address",
        label: "BUCKET",
        field: Field::Text,
        group: BUCKET,
        required: true,
        default: None,
        when: None,
        hint: Some("The bucket name alone. A path belongs to the table that reads it"),
        placeholder: Some("my-bucket"),
    },
    SourceSetting {
        key: "region",
        label: "REGION",
        field: Field::Text,
        group: BUCKET,
        required: true,
        default: None,
        when: None,
        hint: Some("S3 can't detect a bucket's region, and guessing it reads the wrong bucket"),
        placeholder: Some("us-east-1"),
    },
    SourceSetting {
        key: "endpoint",
        label: "ENDPOINT",
        field: Field::Text,
        group: BUCKET,
        required: false,
        default: None,
        when: None,
        hint: Some(
            "An S3-compatible endpoint: MinIO, Cloudflare R2, Alibaba OSS, Tencent COS. Blank \
             means AWS itself",
        ),
        placeholder: Some("https://s3.example.com"),
    },
    SourceSetting {
        key: "auth",
        label: "AUTHENTICATION",
        field: Field::Choice(AUTH),
        group: AUTH_GROUP,
        required: false,
        default: Some("ambient"),
        when: None,
        hint: Some(
            "'ambient' resolves whatever this machine already has: environment, ~/.aws, SSO, \
             instance roles",
        ),
        placeholder: None,
    },
    SourceSetting {
        key: "profile",
        label: "AWS PROFILE",
        field: Field::Text,
        group: AUTH_GROUP,
        required: true,
        default: None,
        when: Some(When {
            key: "auth",
            values: &["profile"],
        }),
        hint: Some("A profile named in this machine's own ~/.aws configuration"),
        placeholder: None,
    },
    SourceSetting {
        key: "access_key_id",
        label: "ACCESS KEY ID",
        field: Field::Text,
        group: AUTH_GROUP,
        required: true,
        default: None,
        when: Some(When {
            key: "auth",
            values: STATIC,
        }),
        hint: None,
        placeholder: Some("AKIA…"),
    },
    SourceSetting {
        key: SECRET_KEY,
        label: "SECRET ACCESS KEY",
        field: Field::Secret,
        group: AUTH_GROUP,
        required: true,
        default: None,
        when: Some(When {
            key: "auth",
            values: STATIC,
        }),
        hint: None,
        placeholder: None,
    },
    SourceSetting {
        key: SESSION_TOKEN,
        label: "SESSION TOKEN",
        field: Field::Secret,
        group: AUTH_GROUP,
        required: false,
        default: None,
        when: Some(When {
            key: "auth",
            values: STATIC,
        }),
        hint: Some("For temporary credentials. Blank for long-lived keys"),
        placeholder: None,
    },
];

/// Its own settings, then every client option — assembled once, because `CLIENT_KEYS` is a runtime
/// table and a declaration is a `&'static [SourceSetting]`.
static SETTINGS: LazyLock<Vec<SourceSetting>> =
    LazyLock::new(|| [OWN, &client_settings()].concat());

/// S3 and every store that speaks it.
#[derive(Debug)]
pub struct S3;

impl SourceKind for S3 {
    const NAME: &'static str = "s3";
    const LABEL: &'static str = "S3";
    const BADGE: &'static str = "S3";
    const MODE: SourceMode = SourceMode::Store;
    /// Two of these on one address would register a single URL between them, so tables under
    /// either would resolve through whichever went in last.
    const UNIQUE: &'static [&'static str] = &["address"];
    const SCHEME: Option<&'static str> = Some("s3");
}

#[async_trait]
impl DataSource for S3 {
    fn settings(&self) -> &'static [SourceSetting] {
        &SETTINGS
    }

    /// <https://docs.aws.amazon.com/AmazonS3/latest/userguide/bucketnamingrules.html>,
    /// general-purpose buckets. The S3-compatible stores that ride this kind (R2, MinIO, OSS, COS)
    /// are all at least this strict, so applying AWS's rules to them refuses nothing they would
    /// have accepted.
    ///
    /// **Not exhaustive, on purpose:** S3 reserves further names no local check can settle, and a
    /// bucket that exists is still one you may not be able to read. This catches what is
    /// *statically* wrong, so the user is told at the field instead of by a signing error.
    fn check_address(&self, address: &str) -> Result<(), String> {
        check_bucket(address)
    }

    /// Build the bucket's store, resolving credentials by whichever mode the def chose.
    ///
    /// The chain is resolved **once** and the answer thrown away; the provider on the store
    /// resolves per request, so rotating credentials keep working.
    async fn connect(
        &self,
        def: &SourceDef,
        secrets: Arc<dyn SecretProvider>,
    ) -> Result<Sourced, String> {
        let value = |key: &str| def.config.get(key).map(|v| v.trim()).unwrap_or_default();
        let region = value("region");
        if region.is_empty() {
            return Err("This S3 data source needs a region.".into());
        }
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(def.setting("address"))
            .with_region(region);

        let endpoint = value("endpoint");
        if !endpoint.is_empty() {
            // Plain HTTP is derived from the endpoint the user typed rather than offered beside
            // it: `http://` is already the decision, and a switch for it is a second answer to a
            // question that has one.
            let allow_http = endpoint.starts_with("http://");
            builder = builder.with_endpoint(endpoint).with_allow_http(allow_http);
        }

        builder = match value("auth") {
            "anonymous" => builder.with_skip_signature(true),
            "profile" => {
                let profile = value("profile");
                if profile.is_empty() {
                    return Err("This S3 data source needs a profile name.".into());
                }
                builder.with_credentials(profile_credentials(region, profile).await?)
            }
            "keys" => {
                let id = value("access_key_id");
                if id.is_empty() {
                    return Err("This S3 data source needs an access key id.".into());
                }
                let secret = read(&secrets, def, SECRET_KEY, SECRET_ENV)
                    .await?
                    .ok_or_else(|| {
                        let fixes = secret_slot(def, SECRET_KEY, SECRET_ENV)
                            .map(|request| request.fixes())
                            .unwrap_or_default();
                        format!(
                            "This S3 data source has no secret access key on this machine. {fixes}"
                        )
                    })?;
                let token = read(&secrets, def, SESSION_TOKEN, TOKEN_ENV).await?;
                builder = builder
                    .with_access_key_id(id)
                    .with_secret_access_key(secret.expose());
                match token {
                    Some(token) => builder.with_token(token.expose()),
                    None => builder,
                }
            }
            _ => builder.with_credentials(ambient_credentials(region).await?),
        };

        for (key, value) in client_options(def) {
            builder = builder.with_config(AmazonS3ConfigKey::Client(key), value);
        }
        let store = built(def, builder.build())?;
        reachable(&store, || {
            format!(
                "The bucket '{}' does not answer in region '{region}'. Check the region, or that \
                 the bucket exists.",
                def.setting("address")
            )
        })
        .await?;
        Ok(Sourced::Store { store })
    }
}

/// Read one of this data source's secrets **off the render-free worker**, the way every keystore
/// read is: the store is a blocking platform call, and `connect` is on the engine's runtime.
///
/// `Ok(None)` for a key nothing is stored for, which is the ordinary case for an optional one.
async fn read(
    secrets: &Arc<dyn SecretProvider>,
    def: &SourceDef,
    key: &str,
    env: &'static [&'static str],
) -> Result<Option<Secret>, String> {
    // A def that expects no secret for `key` has none stored anywhere: there is no slot to ask
    // about, which is the same answer as an empty one.
    let Some(request) = secret_slot(def, key, env) else {
        return Ok(None);
    };
    let secrets = Arc::clone(secrets);
    tokio::task::spawn_blocking(move || secrets.secret(&request))
        .await
        .map_err(|e| format!("Reading a secret failed: {e}"))?
}

/// Every profile named in this machine's own AWS configuration, sorted — what the data source
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
/// list would have been. What the *data source* does with a name it cannot resolve is
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
/// different one, and the data source's own region is the answer the user gave.
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
/// Profile were the same data source wherever ambient credentials existed, and a misspelled
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
    use std::fs;
    use std::process;

    use super::*;

    /// One S3 data source over `name`, signed by the named profile `readonly`.
    fn bucket(name: &str) -> SourceDef {
        SourceDef {
            kind: S3::NAME.into(),
            name: name.into(),
            config: [
                ("region", "eu-west-2"),
                ("auth", "profile"),
                ("profile", "readonly"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
            ..Default::default()
        }
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
    /// It asserts the credential, not just that a store was built: the bridge's whole job
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

        S3.connect(
            &bucket("signed-lake"),
            Arc::new(crate::secrets::MemSecrets::new()),
        )
        .await
        .expect("a store the profile signs for");

        let _ = fs::remove_dir_all(&dir);
    }
}
