//! Object stores: the three registrants that turn a [`SourceDef`] into a live `object_store`
//! (W7, `docs/CONNECTIONS_SPEC.md`), and everything true of all three.
//!
//! **The shell here, the registrant in its own file** ([`s3`], [`gcs`], [`http`]): the naming
//! rules each kind owns are its own `check_address`, and what every store shares — the client
//! options, the reachability probe, the one sentence a failed build reads as — is this module's.
//!
//! **DataFusion core resolves nothing.** There is no built-in "read `s3://…`": the embedder builds
//! a store and calls `register_object_store` **per bucket**, or every scan of that bucket fails
//! with *"No suitable object store found"*. That call is the whole of what a store data source
//! *does*, which is why the registration URL is exactly `SCHEME://address` and why a bucket with a
//! path in it is refused rather than registered under a key nothing looks up.
//!
//! **Authorisation is a declared setting, never a kind.** Ambient, a named profile, static keys
//! and anonymous are four ways of building one store over one bucket: four kinds would give one
//! bucket four identities, which the upsert clash check would permit — and all four compose the
//! same registration URL and would displace each other's store.
//!
//! **Only one key anywhere here holds a secret**, S3's `secret_access_key`, and it holds it the
//! way every secret is held: the value lives in this machine's keystore or arrives through AWS's
//! own environment convention, and the def records the expectation alone. Every other mode
//! resolves through the host's provider chain, a named profile, a key **file** the OS already lets
//! the user read, or not at all.

pub(crate) mod gcs;
pub(crate) mod http;
pub(crate) mod s3;

use std::sync::Arc;

use futures::stream::StreamExt;
use object_store::{ClientConfigKey, Error, ObjectStore};

use strata_arrow::client::CLIENT_KEYS;
use strata_model::SourceDef;

use crate::sources::source::{Field, SourceSetting};

/// The section every store's client options sit under.
pub(super) const CLIENT_GROUP: Option<&str> = Some("CLIENT OPTIONS");

/// `object_store`'s own `ClientConfigKey` map, as declared settings.
///
/// **Derived from [`CLIENT_KEYS`], never retyped**: that table is already the one place these
/// names and their descriptions are written, and `check_client_config` parses every one of them
/// through `ClientConfigKey::from_str` — so an option the form offers is one the store takes.
/// Every provider's store is built on the same HTTP client, so all three registrants fold in the
/// same list: a proxy, a timeout or a user agent applies to a signed S3 request exactly as it does
/// to a public HTTP one.
///
/// The label is the option's own name rather than an eyebrow of ours, because these are
/// `object_store`'s vocabulary and a user matching one against its documentation wants the
/// spelling that documentation uses.
pub(super) fn client_settings() -> Vec<SourceSetting> {
    CLIENT_KEYS
        .iter()
        .map(|option| SourceSetting {
            key: option.name,
            label: option.name,
            field: Field::Text,
            group: CLIENT_GROUP,
            required: false,
            default: None,
            when: None,
            hint: Some(option.what),
            placeholder: None,
        })
        .collect()
}

/// Resolve a data source's client options into `object_store`'s own keys, in a stable order.
///
/// A name the crate does not parse is **skipped rather than refused**: the declaration is built
/// from `CLIENT_KEYS`, so a value under any other key came from a hand-edited `project.json` and
/// is not a thing to fail a data source over.
pub(super) fn client_options(def: &SourceDef) -> Vec<(ClientConfigKey, String)> {
    CLIENT_KEYS
        .iter()
        .filter_map(|option| {
            let value = def.config.get(option.name)?.trim();
            match value.is_empty() {
                true => None,
                false => Some((option.name.parse().ok()?, value.to_string())),
            }
        })
        .collect()
}

/// One built store, or why the description it was built from is wrong.
///
/// The sentence is the shell's rather than each registrant's, because it says the same thing about
/// all three and names the data source the user gave rather than anything about the provider.
pub(super) fn built<S: ObjectStore + 'static>(
    def: &SourceDef,
    built: Result<S, Error>,
) -> Result<Arc<dyn ObjectStore>, String> {
    built
        .map(|store| Arc::new(store) as Arc<dyn ObjectStore>)
        .map_err(|e| format!("Cannot reach '{}': {e}", def.named()))
}

/// **Does this bucket actually answer?** One request, on the data source's own store, and the
/// difference between a row that means something and a row that means a struct was built.
///
/// This was deliberately absent once, on the grounds that a probe is a round trip per data source on
/// every project open. That traded away too much: `AmazonS3Builder` constructs a store for a bucket
/// that does not exist in the region it was given, so a mistyped region registered **green** and
/// every table over it failed with `object_store`'s bare-redirect message, which names no bucket,
/// no region and no data source.
///
/// **The first page of a listing, not a HEAD and not a whole listing.** `ObjectStore` has no
/// head-bucket call. It has to be `list`'s *stream* taken once: `list_with_delimiter` reads like
/// the cheaper call and drains the paginated stream to assemble a complete `ListResult`, which
/// against the Hive lakes this exists for is a round trip per thousand folders. An empty bucket
/// ends the stream without yielding, which is a **pass**; only a `Some(Err(..))` is a refusal.
///
/// **It refuses exactly one thing: a bucket that is not in the region it was given.** That is the
/// fault no local check can see, and "may I list the root" is a far stronger demand than Strata
/// makes — connecting is the registration pass's first phase, so there is no table prefix to probe
/// with. A prefix-scoped `s3:ListBucket` and a `GetObject`-only public bucket both answer 403 at
/// the root while working perfectly, so refusing either would take a working project's every table
/// down. Rejected credentials therefore still fail at the first table, exactly as before this
/// probe.
///
/// `refused` is the caller's own sentence, because only the registrant knows which of its settings
/// is wrong — S3 names the region, which is the whole of the fix.
///
/// **Matched on the message, because `object_store` gives us nothing else.** The crate classifies
/// statuses into `PermissionDenied` / `NotFound` / … in `client/retry.rs`, but the S3 list path
/// never reaches it: `aws/client.rs` routes only two variants through that mapping and sends
/// `ListRequest` to `_ => Generic`. `RetryError` is `pub(crate)`, so `status()` cannot be
/// downcast to either. A first version matched on the variants and was dead code in every arm;
/// MinIO caught it. The sentence matched is one `object_store` defines as a literal.
pub(super) async fn probe(
    store: &Arc<dyn ObjectStore>,
    refused: impl FnOnce() -> String,
) -> Result<(), String> {
    match store.list(None).next().await {
        Some(Err(e)) if is_bare_redirect(&e) => Err(refused()),
        _ => Ok(()),
    }
}

/// The one listing failure that says the *data source* is wrong rather than the caller's rights.
///
/// S3 answers a cross-region request with a 301 carrying no `Location` header; `object_store` has
/// a dedicated error for it whose `Display` is this literal (`client/retry.rs`,
/// `RequestError::BareRedirect`). Its own text goes on to guess at "an incorrectly configured
/// region" — a guess, because the crate has never heard of the field. We have, so the caller says
/// it outright.
///
/// Substring rather than equality: the sentence arrives wrapped in the layers that carried it,
/// which for a listing is `Error::Generic`'s `Generic {store} error: ` around `RetryError`'s
/// `Error performing {METHOD} {uri} in {elapsed:?}[, after {n} retries, …] - `.
fn is_bare_redirect(e: &Error) -> bool {
    e.to_string().contains("redirect without LOCATION")
}

/// The longest any single dot-separated part of a bucket name may be. Both providers say 63; for
/// S3 that is also the whole name's limit, while GCS lets a dotted name run to
/// [`GCS_DOTTED_MAX`].
const LABEL_MAX: usize = 63;
/// The shortest a bucket name may be, on both providers.
const BUCKET_MIN: usize = 3;
/// GCS only: a name **containing dots** may run this long in total, each part still capped at
/// [`LABEL_MAX`].
const GCS_DOTTED_MAX: usize = 222;

/// <https://docs.aws.amazon.com/AmazonS3/latest/userguide/bucketnamingrules.html>, general-purpose
/// buckets. The S3-compatible stores that ride this provider (R2, MinIO, OSS, COS) are all at
/// least this strict, so applying AWS's rules to them refuses nothing they would have accepted.
pub(super) fn check_bucket(bucket: &str) -> Result<(), String> {
    if bucket.is_empty() {
        return Err("This data source has no bucket.".into());
    }
    if !bucket
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
    {
        return Err(
            "An S3 bucket name can only contain lowercase letters, numbers, dots and hyphens."
                .into(),
        );
    }
    if !starts_and_ends_alphanumeric(bucket) {
        return Err("An S3 bucket name has to start and end with a letter or number.".into());
    }
    if !(BUCKET_MIN..=LABEL_MAX).contains(&bucket.len()) {
        return Err(format!(
            "An S3 bucket name is {BUCKET_MIN} to {LABEL_MAX} characters long."
        ));
    }
    if bucket.contains("..") {
        return Err("An S3 bucket name can't contain two dots in a row.".into());
    }
    if is_dotted_decimal_ip(bucket) {
        return Err("An S3 bucket name can't be formatted as an IP address.".into());
    }
    Ok(())
}

/// <https://cloud.google.com/storage/docs/buckets#naming>. Deliberately **not** S3's rules: GCS
/// allows underscores and a dotted name up to [`GCS_DOTTED_MAX`], and reserves Google's own name.
/// Left to the store: "close misspellings" of `google`, and a dotted name's ownership verification.
pub(super) fn check_gcs_bucket(bucket: &str) -> Result<(), String> {
    if bucket.is_empty() {
        return Err("This data source has no bucket.".into());
    }
    if !bucket
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-' || c == '_')
    {
        return Err(
            "A GCS bucket name can only contain lowercase letters, numbers, dots, \
                    hyphens and underscores."
                .into(),
        );
    }
    if !starts_and_ends_alphanumeric(bucket) {
        return Err("A GCS bucket name has to start and end with a letter or number.".into());
    }
    match bucket.contains('.') {
        true => {
            if !(BUCKET_MIN..=GCS_DOTTED_MAX).contains(&bucket.len()) {
                return Err(format!(
                    "A GCS bucket name containing dots is {BUCKET_MIN} to {GCS_DOTTED_MAX} \
                     characters long."
                ));
            }
            if !bucket
                .split('.')
                .all(|part| (1..=LABEL_MAX).contains(&part.len()))
            {
                return Err(format!(
                    "Each dot-separated part of a GCS bucket name is 1 to {LABEL_MAX} characters \
                     long."
                ));
            }
        }
        false => {
            if !(BUCKET_MIN..=LABEL_MAX).contains(&bucket.len()) {
                return Err(format!(
                    "A GCS bucket name is {BUCKET_MIN} to {LABEL_MAX} characters long."
                ));
            }
        }
    }
    if is_dotted_decimal_ip(bucket) {
        return Err("A GCS bucket name can't be an IP address.".into());
    }
    if bucket.starts_with("goog") {
        return Err("A GCS bucket name can't start with 'goog'.".into());
    }
    if bucket.contains("google") {
        return Err("A GCS bucket name can't contain 'google'.".into());
    }
    Ok(())
}

/// An HTTP data source's address is a **whole origin URL** — `http://aserver:8484` — and it is
/// written in one box, scheme included, because `http` and `https` are two different origins and
/// only the person typing knows which their server speaks.
///
/// Everything after the authority is refused rather than trimmed away. The object-store registry
/// keys on scheme and authority, so a path here would register under a key nothing looks up while
/// the field went on showing it; and a path is not lost by being refused — it belongs to the
/// source of whatever table reads through this data source.
pub(super) fn check_http_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("This data source has no URL.".into());
    }
    if url.chars().any(char::is_whitespace) {
        return Err("An HTTP URL can't contain spaces.".into());
    }
    let Some(authority) = ["http://", "https://"]
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme))
    else {
        return Err(
            "An HTTP data source needs a scheme: write 'https://aserver' or 'http://aserver'."
                .into(),
        );
    };
    if authority.is_empty() {
        return Err("An HTTP data source needs a host after its scheme.".into());
    }
    let host = &authority[..authority.find(['/', '?', '#']).unwrap_or(authority.len())];
    if let Some(at) = host.find('@') {
        return Err(format!(
            "An HTTP data source can't carry a username or password. Drop '{}' from the URL.",
            &host[..=at],
        ));
    }
    if let Some(at) = authority.find(['/', '?', '#']) {
        return Err(format!(
            "An HTTP data source is an origin, not a path. Drop '{}' and give it to the table \
             that reads through this source.",
            &authority[at..]
        ));
    }
    Ok(())
}

fn starts_and_ends_alphanumeric(bucket: &str) -> bool {
    let alphanumeric = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    bucket.starts_with(alphanumeric) && bucket.ends_with(alphanumeric)
}

/// Whether `bucket` reads as `192.168.5.4` — four decimal octets, which is the only form the GCS
/// and S3 rules name. A name like `999.1.1.1` is not representable as an address and so is not
/// refused by either.
fn is_dotted_decimal_ip(bucket: &str) -> bool {
    let parts: Vec<&str> = bucket.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok())
}
