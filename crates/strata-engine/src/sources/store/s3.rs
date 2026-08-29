//! **S3, and anything that speaks it.** The bucket's store, and the credential bridge that keeps
//! it signed.
//!
//! The one arm with a credential chain of its own, which is the whole reason `aws-config` is a
//! dependency: `object_store` alone reads the `AWS_*` variables and stops, so "log in the way you
//! already do" — a profile, SSO, `credential_process`, an assumed role — is not something it can
//! express. Nothing here holds a key: a credential value exists for the length of one signed
//! request, inside [`SdkCredentials::get_credential`].

use std::sync::Arc;

use aws_config::profile::ProfileFileCredentialsProvider;
use aws_config::provider_config::ProviderConfig;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use aws_types::os_shim_internal::{Env, Fs};
use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey, AwsCredential, AwsCredentialProvider};
use object_store::{ClientConfigKey, CredentialProvider, Error, ObjectStore};

use strata_model::{ConnectionDef, S3Auth, S3Store};

use super::built;

/// The bucket's store: the region, the endpoint, the credentials and the client options, in the
/// one place S3's rules are written.
///
/// Every way it can fail is a way of describing the connection wrong, which is what lets
/// [`connect`](super::connect) treat the registration itself as one line with one meaning.
pub(super) async fn build(
    conn: &ConnectionDef,
    s3: &S3Store,
    options: &[(ClientConfigKey, String)],
) -> Result<Arc<dyn ObjectStore>, String> {
    let region = s3.region.trim();
    if region.is_empty() {
        return Err("This S3 connection needs a region.".into());
    }
    let mut builder = AmazonS3Builder::new()
        .with_bucket_name(conn.address.trim())
        .with_region(region);
    let endpoint = s3.endpoint.trim();
    if !endpoint.is_empty() {
        if endpoint.starts_with("http://") && !s3.allow_http {
            return Err(format!(
                "The endpoint '{endpoint}' is plain HTTP. Turn on 'Allow plain HTTP' for this \
                 connection, or give it an https endpoint."
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
    for (key, value) in options {
        builder = builder.with_config(AmazonS3ConfigKey::Client(*key), value);
    }
    built(conn, builder.build())
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
    use std::fs;
    use std::process;

    use strata_model::Provider;

    use super::*;

    /// One S3 connection over `name`, for the arm to build a store from.
    fn bucket(name: &str) -> ConnectionDef {
        ConnectionDef {
            address: name.into(),
            name: String::new(),
            provider: Provider::S3(S3Store::default()),
            client_config: Default::default(),
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

        build(
            &bucket("signed-lake"),
            &S3Store {
                region: "eu-west-2".into(),
                auth: S3Auth::Profile {
                    name: "readonly".into(),
                },
                ..Default::default()
            },
            &[],
        )
        .await
        .expect("a store the profile signs for");

        let _ = fs::remove_dir_all(&dir);
    }
}
