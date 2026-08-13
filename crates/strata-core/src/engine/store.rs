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

use std::collections::BTreeMap;
use std::sync::Arc;

use aws_config::profile::ProfileFileCredentialsProvider;
use aws_config::provider_config::ProviderConfig;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use aws_types::os_shim_internal::{Env, Fs};
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::prelude::*;
// `list` hands back a stream, and [`reachable`] wants exactly its first item.
use futures::stream::StreamExt;
use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey, AwsCredential, AwsCredentialProvider};
use object_store::gcp::{GoogleCloudStorageBuilder, GoogleConfigKey};
use object_store::http::HttpBuilder;
use object_store::{ClientConfigKey, CredentialProvider, Error, ObjectStore};

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
/// **And then the bucket is asked, too** ([`reachable`]). That is a change of position: this
/// function used to stop at the credential chain on the argument that asking the bucket costs a
/// round trip per connection on every project open. It does, and it is worth it — the case that
/// argument did not cover is a description that is *well-formed and wrong*, which no local check
/// can see. A bucket named in the wrong region builds a perfectly good store, registers green,
/// and fails every table under it. One request per connection at open buys a status that means
/// what it says.
///
/// Idempotent: registering over an existing key replaces it, which is what a re-scan wants.
///
/// One thing it cannot clean up, because it is not told about it: a connection whose **bucket**
/// was edited leaves the store registered under the *old* URL. That is an edit, and the edit
/// gesture owns it (Connections 03) — nothing here ever sees the def it replaced.
pub async fn connect(ctx: &SessionContext, conn: &ConnectionDef) -> Result<(), String> {
    let prepared = match prepare(conn).await {
        // **And then ask the bucket.** See [`reachable`] — the step that makes a connection's row
        // mean "this bucket answers" rather than "a store was constructed".
        Ok((url, store)) => match reachable(conn, &store).await {
            Ok(()) => Ok((url, store)),
            Err(why) => Err(why),
        },
        Err(why) => Err(why),
    };
    settle(ctx, conn, prepared)
}

/// Apply a prepared store to the session, or take back whatever this connection last registered
/// — the all-or-nothing half of [`connect`]'s contract, and the only place it is written.
///
/// Separate from `connect` so the tests' probe-free path settles through the *same* code rather
/// than a helper that restates it. That distinction has teeth: the first version of that helper
/// registered on `Ok` and simply returned on `Err`, which silently dropped the deregistration —
/// and the test whose whole subject is "a refused reconnect leaves nothing behind" went red
/// against a stand-in that could never have passed it. A test double for a contract has to share
/// the contract.
fn settle(
    ctx: &SessionContext,
    conn: &ConnectionDef,
    prepared: Result<(ObjectStoreUrl, Arc<dyn ObjectStore>), String>,
) -> Result<(), String> {
    match prepared {
        Ok((url, store)) => {
            ctx.register_object_store(url.as_ref(), store);
            Ok(())
        }
        Err(why) => {
            // Re-parsed rather than threaded through the error: a def refused *before* the URL
            // was parsed never registered anything under it either, so the lookup simply misses.
            // Errs when nothing was registered under this key, which is the ordinary case (a
            // first pass, or a def that has never worked) and not a failure of its own.
            if let Ok(url) = ObjectStoreUrl::parse(conn.url()) {
                let _ = ctx.deregister_object_store(url.as_ref());
            }
            Err(why)
        }
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
    // **The provider's own naming rules, checked from the def's own module** — the same call the
    // connection editor makes, so a name refused at the field is refused here in the same words
    // and a name this accepts is one the editor would have let through. A blank bucket, a name
    // carrying a path and every rule below them are all this one answer.
    conn.provider.check_address(&conn.address)?;
    // The client options are the def's other half, and refused on the same terms: a name
    // `object_store` has never heard of would otherwise be dropped silently at build time.
    check_client_config(&conn.client_config)?;
    let url = ObjectStoreUrl::parse(conn.url()).map_err(|e| {
        format!(
            "'{}' is not a bucket Strata can register: {e}",
            conn.address.trim()
        )
    })?;
    let store = build(conn).await?;
    Ok((url, store))
}

/// **Does this bucket actually answer?** One request, on the connection's own store, and the
/// difference between a row that means something and a row that means a struct was built.
///
/// This used to be deliberately absent, and the reasoning was explicit: [`connect`] asks the
/// *host's chain* for a credential and never asks the bucket whether it is any good, because
/// that is a round trip per connection on every project open. What that traded away turned out
/// to be too much. `AmazonS3Builder` will happily construct a store for a bucket that does not
/// exist in the region it was given — nothing in the description is checked against anything —
/// so a mistyped region registered **green**, the pane showed a healthy connection, and every
/// table over it then failed with `object_store`'s own bare-redirect message, which names no
/// bucket, no region and no connection. The diagnosis was one clause long and landed on the
/// wrong surface. A connection's status is worth a request.
///
/// **The first page of a listing, not a HEAD and not a whole listing.** `ObjectStore` has no
/// head-bucket call, so a list is the cheapest thing the trait can express that the server has to
/// resolve for real. It has to be `list`'s *stream*, taken once: `list_with_delimiter` reads
/// like the cheaper call and is the more expensive one, because it drains the paginated stream to
/// assemble a complete `ListResult` (`object_store` `client/list.rs`). Against the Hive lakes
/// this exists for — tens of thousands of top-level `key=` folders — that is a round trip per
/// thousand, per connection, on every project open and every whole-catalog re-scan. Pulling one
/// item off `list` fetches one page and drops the rest unpolled.
///
/// An empty bucket ends the stream without yielding, which is a **pass**: nothing was there to
/// find, and the request that established it succeeded. Only a `Some(Err(..))` is a refusal.
///
/// **It refuses exactly one thing: a bucket that is not in the region it was given.** Everything
/// else the listing can answer — including every flavour of "no" — registers, and is left to the
/// table that actually reads a path.
///
/// That is narrower than it first looks like it should be, and the narrowness is the point twice
/// over.
///
/// **It is the fault this exists for.** A wrong region is the case no local check can see: the
/// bucket name is valid, the credentials resolve, the store builds, and S3 answers a 301 carrying
/// no `Location` header, which reaches the user as a sentence naming no bucket, no region and no
/// connection. Nothing else that a root listing can tell us is worth a refusal.
///
/// **And "may I list the root" is a far stronger demand than Strata makes.** `connect` is
/// `register_pass`'s **first** phase, so no table has registered and there is no prefix to probe
/// with — a root listing is simply the only question available, not the right one. An
/// `s3:ListBucket` conditioned on `s3:prefix: ["team/*"]` is AWS's own documented way to hand
/// somebody a folder and answers **403 at the root** while `s3://lake/team/events/` reads
/// perfectly; a published dataset granting `GetObject` and not `ListBucket` does the same, and a
/// single-file source over it never lists at all. Refusing either would take a working project's
/// every table down with the connection — a worse fault than the one being caught.
///
/// So rejected credentials still fail at the first table, which is exactly where they failed
/// before this probe existed: this declines a *new* win rather than losing an old one.
///
/// **Matched on the message, because `object_store` gives us nothing else to match on.** This is
/// the part worth checking before trusting: the crate *does* classify statuses into
/// `PermissionDenied` / `NotFound` / … (`client/retry.rs`, `RetryError::error`), but the S3 list
/// path never reaches it — `aws/client.rs`'s `From<Error> for crate::Error` routes only
/// `CompleteMultipartRequest` and `DeleteObjectsRequest` through that mapping and sends every
/// other variant, `ListRequest` included, to `_ => Generic`. So a 403, a 404 and a bare redirect
/// arrive here as the same variant, and `RetryError` is `pub(crate)` so its `status()` cannot be
/// reached by downcast either. A first version of this function matched on the variants and was
/// dead code in every arm; MinIO caught it. Matching one distinctive sentence is the honest
/// remaining option, and it is a sentence `object_store` defines as a literal
/// (`RequestError::BareRedirect`).
///
/// **HTTP is exempt, and not out of laziness.** `object_store`'s HTTP store lists over WebDAV
/// `PROPFIND`, which most origins serving files do not implement (MinIO included — see
/// `tests/object_store_minio.rs`, where it is why the HTTP arm reads a single object). Probing
/// one by listing would refuse working connections for a verb their server was never going to
/// answer, which is a worse lie than the one this function exists to remove. An HTTP connection
/// names a whole origin and its table names the object, so the table's own registration is where
/// its reachability is genuinely tested.
async fn reachable(conn: &ConnectionDef, store: &Arc<dyn ObjectStore>) -> Result<(), String> {
    if matches!(conn.provider, Provider::Http) {
        return Ok(());
    }
    // One page, then dropped: `next()` polls the paginated stream once, and the stream is not
    // polled again. An exhausted stream (`None`) is an empty bucket, which answered fine.
    match store.list(None).next().await {
        Some(Err(e)) if is_bare_redirect(&e) => Err(wrong_region(conn)),
        // Everything else — an object, an empty bucket, a 403, a 404 — registers. See above.
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
        // No other provider takes a region, so none can be redirected for naming the wrong one;
        // the generic wording is here so the function is total rather than because it is reachable.
        _ => format!("'{}' did not answer.", conn.url()),
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
    // `Default::default()` is `EnvConfigFiles` — the standard pair of profile files, resolved
    // through `env` so the two override variables are honoured. Named by inference rather than
    // by path: `aws-config`'s own alias for the type is deprecated in favour of one that would
    // cost a second direct dependency on `aws-runtime` for a word.
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

/// One tunable of the HTTP client every object store is built on — `object_store`'s
/// [`ClientConfigKey`], with the sentence the editor's picker shows beside it.
///
/// The **name** is `ClientConfigKey`'s own (`AsRef<str>`), so what a connection stores is what
/// `from_str` reads back; the description is ours, because the crate's is a doc comment and not a
/// value.
#[derive(PartialEq, Eq, Debug)]
pub struct ClientKey {
    pub name: &'static str,
    pub what: &'static str,
}

/// Every client option a connection may set, in the order the picker offers them.
///
/// A written-down table, because `ClientConfigKey` has no enumeration of itself — the same shape
/// and the same reason as `ENGINE_KEYS` for DataFusion's settings. It is kept honest from the
/// other side: [`check_client_config`] parses every name through `ClientConfigKey::from_str`, so
/// a typo here is a test failure rather than an option that silently never applies
/// (`tests::every_offered_client_key_is_one_object_store_knows`).
///
/// Two of `object_store`'s keys are deliberately **absent**, and for the same reason in both
/// halves: they are already said elsewhere, and a second control for one setting is two controls
/// that can disagree. `allow_http` is the S3 provider's own
/// [`S3Store::allow_http`](strata_model::S3Store::allow_http) toggle, and on an HTTP connection it
/// is the **scheme the user typed** ([`build`] derives it); `default_content_type` describes an
/// upload, and nothing here writes.
pub const CLIENT_KEYS: &[ClientKey] = &[
    ClientKey {
        name: "timeout",
        what: "Whole-request timeout, from connect to the last byte of the body (30s, 500ms)",
    },
    ClientKey {
        name: "connect_timeout",
        what: "Timeout for the connect phase alone",
    },
    ClientKey {
        name: "pool_idle_timeout",
        what: "How long an idle connection is kept alive",
    },
    ClientKey {
        name: "pool_max_idle_per_host",
        what: "Maximum idle connections kept per host",
    },
    ClientKey {
        name: "allow_invalid_certificates",
        what: "Trust any TLS certificate. Every site, including expired ones - a last resort",
    },
    ClientKey {
        name: "proxy_url",
        what: "HTTP proxy to send requests through",
    },
    ClientKey {
        name: "proxy_ca_certificate",
        what: "PEM certificate authority for the proxy connection",
    },
    ClientKey {
        name: "proxy_excludes",
        what: "Hosts that bypass the proxy",
    },
    ClientKey {
        name: "user_agent",
        what: "User-Agent header this connection sends",
    },
    ClientKey {
        name: "http1_only",
        what: "Use HTTP/1 only",
    },
    ClientKey {
        name: "http2_only",
        what: "Use HTTP/2 only",
    },
    ClientKey {
        name: "http2_keep_alive_interval",
        what: "How often to send an HTTP/2 keep-alive ping",
    },
    ClientKey {
        name: "http2_keep_alive_timeout",
        what: "How long to wait for a keep-alive ping to be acknowledged",
    },
    ClientKey {
        name: "http2_keep_alive_while_idle",
        what: "Keep sending HTTP/2 pings while the connection is idle",
    },
    ClientKey {
        name: "http2_max_frame_size",
        what: "Maximum HTTP/2 frame size",
    },
    ClientKey {
        name: "randomize_addresses",
        what: "Shuffle resolved addresses, spreading connections over more servers",
    },
];

/// The catalogue entry for `name`, if this is a client option a connection may set.
pub fn client_key(name: &str) -> Option<&'static ClientKey> {
    CLIENT_KEYS.iter().find(|k| k.name == name)
}

/// Whether a connection's client options are ones the store will accept — **the same call the
/// connection editor makes**, so an option refused at the field is refused here in the same words.
///
/// Two failures, and neither is `object_store`'s to report: a name it has never heard of would be
/// dropped on the floor by `from_str` at build time with nothing said, and a blank value would be
/// handed to a parser that expects a duration or a boolean. Both are told here, naming the key.
pub fn check_client_config(config: &BTreeMap<String, String>) -> Result<(), String> {
    for (name, value) in config {
        if client_key(name).is_none() {
            return Err(format!(
                "'{name}' is not a client option Strata can set on a connection."
            ));
        }
        if value.trim().is_empty() {
            return Err(format!("The client option '{name}' has no value."));
        }
    }
    Ok(())
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
                // **Refused by name, because reqwest refuses it namelessly.** `ClientOptions`
                // builds its client `https_only(!allow_http)`, so a plain-`http` endpoint without
                // the toggle fails every request with "HTTP error: builder error" — no host, no
                // scheme, nothing to act on. The HTTP provider derives its answer from the scheme
                // the user typed; this one has a control of its own, so the honest thing is to say
                // which control. (Cost an afternoon once, on the connection editor's own tests.)
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
            // **Last, so an explicit option wins.** `allow_http` is the one key both halves can
            // reach (the endpoint toggle above sets it), and the user's own table is the more
            // specific statement of the two.
            for (key, value) in client_options(conn) {
                builder = builder.with_config(AmazonS3ConfigKey::Client(key), value);
            }
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
            for (key, value) in client_options(conn) {
                builder = builder.with_config(GoogleConfigKey::Client(key), value);
            }
            builder
                .build()
                .map(|s| Arc::new(s) as Arc<dyn ObjectStore>)
                .map_err(|e| format!("Cannot reach '{}': {e}", conn.url()))
        }
        // A public origin: no credentials to resolve, and nothing to get wrong but the URL —
        // whose scheme is the def's own, so a plain-`http` origin is reached as one.
        // The one builder that takes `ClientConfigKey` directly; the two above wrap it in their
        // own config enum, and all three land on the same `ClientOptions`.
        Provider::Http => {
            // **Plain `http` is allowed exactly when the address asks for it.** `ClientOptions`
            // builds a reqwest client with `https_only(!allow_http)`, so without this every
            // request to an `http://` origin fails before it leaves the process, with a
            // "builder error" that names nothing. It is derived rather than offered as a
            // control because the user has already said which they meant, in the scheme: a
            // toggle beside it could only disagree with the URL above it.
            let insecure = conn.address.trim().starts_with("http://");
            let mut builder = HttpBuilder::new()
                .with_url(conn.url())
                .with_config(ClientConfigKey::AllowHttp, insecure.to_string());
            for (key, value) in client_options(conn) {
                builder = builder.with_config(key, value);
            }
            builder
                .build()
                .map(|s| Arc::new(s) as Arc<dyn ObjectStore>)
                .map_err(|e| format!("Cannot reach '{}': {e}", conn.url()))
        }
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
    use strata_model::{GcsStore, S3Store};

    fn s3(bucket: &str, store: S3Store) -> ConnectionDef {
        ConnectionDef {
            address: bucket.into(),
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
                provider: Provider::Gcs(GcsStore {
                    auth: GcsAuth::Anonymous,
                }),
                client_config: Default::default(),
            },
            // HTTP's address is the whole origin, scheme and all — it is what the user typed
            // and what the registry keys on, with nothing composed in between.
            ConnectionDef {
                address: "http://aserver:8484".into(),
                provider: Provider::Http,
                client_config: Default::default(),
            },
        ] {
            connect_unprobed(&ctx, &conn).await.expect("registers");
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

        // The same bucket, now described by a def that cannot build a store.
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
                    provider: Provider::Http,
                    client_config: Default::default(),
                },
                "spaces",
            ),
            // A scheme is half of an HTTP address, so an origin without one is not one.
            (
                ConnectionDef {
                    address: "aserver:8484".into(),
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

    /// **Every name we offer is one `object_store` answers to.** The catalogue is written down
    /// (`ClientConfigKey` cannot enumerate itself), so this is what keeps it from drifting: a
    /// typo would otherwise be a picker entry that parses to nothing and silently never applies.
    #[test]
    fn every_offered_client_key_is_one_object_store_knows() {
        for key in CLIENT_KEYS {
            let parsed: ClientConfigKey = key
                .name
                .parse()
                .unwrap_or_else(|_| panic!("object_store does not know '{}'", key.name));
            // Round-trips, so the name we store is the one it reads back — not a synonym.
            assert_eq!(parsed.as_ref(), key.name);
            assert!(!key.what.is_empty(), "{} has no description", key.name);
        }
        // The two deliberate omissions, so removing them stays a decision rather than an
        // oversight: `allow_http` is the S3 endpoint toggle's, and nothing here uploads.
        for absent in ["allow_http", "default_content_type"] {
            assert!(
                absent.parse::<ClientConfigKey>().is_ok(),
                "still a real key, just not offered"
            );
            assert!(client_key(absent).is_none(), "{absent} is not offered");
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
        // …and with the toggle on it is exactly the connection the MinIO test drives.
        connect_unprobed(&ctx, &conn(true))
            .await
            .expect("registers");
        // An https endpoint needs no toggle.
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
                    provider: Provider::Http,
                    client_config: Default::default(),
                },
                "'/fake'",
            ),
            (
                ConnectionDef {
                    address: "acme-lake/data".into(),
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
                // Read whatever the client sends before answering, or it sees the write as a
                // reset connection rather than as a response.
                let _ = stream.read(&mut [0; 1024]);
                // No `Location`, which is the whole point.
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
        // And a refused connection leaves nothing registered behind it.
        assert!(!reaches(&ctx, "s3://acme-lake/x.parquet"));
    }
}
