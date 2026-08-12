//! **Connections** — the persisted description of one remote object store a project reads
//! from (W7, `docs/CONNECTIONS_SPEC.md`). Exactly what `.strata/project.json` stores, like
//! the catalog defs beside it.
//!
//! The rule the whole feature is built around: **Strata never stores, prompts for, or reads
//! a secret.** A connection carries only non-secret metadata — a bucket, a region, an
//! endpoint — plus a *reference* to where credentials live (a named `~/.aws` profile, a
//! service-account key **file path**). Credentials resolve at query time from the host's own
//! provider chains, so nothing here is a key, and nothing here has to be gitignored: the
//! whole def is shareable, which is why it rides the committed `project.json` rather than
//! the local `session.json`.
//!
//! There is no arm for a secret, anywhere in this module. That is the enforcement: an
//! access-key field cannot be added without adding a variant that says so out loud.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// One project-scoped connection: the bucket it names, and the provider that serves it.
///
/// **Identity is [`url`](Self::url), not the bucket** — scheme *and* authority, which is
/// exactly what DataFusion's object-store registry keys on (see `strata_core::engine::store`).
/// The distinction is not academic: `s3://lake` and `gs://lake` share a bucket and are two
/// different connections over two different stores, so anything addressing one of them —
/// a registration outcome, a store row, a Configure dropdown — has to say which.
///
/// **[`address`](Self::address) is not a bucket name**, which is why it is not called one: an
/// object store is addressed by a bucket, whose scheme its provider states (`acme-lake` under
/// S3 is `s3://acme-lake`), while an HTTP origin is addressed by the URL itself, scheme
/// included. One field either way, because a connection has exactly one address; what a
/// *provider* makes of it is the provider's business.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ConnectionDef {
    /// Where this connection points, in the terms its provider uses.
    ///
    /// - **S3 / GCS** — the bucket name alone (`acme-lake`). The scheme is the provider's, so
    ///   storing it here too would be two statements of one fact that can disagree: an `s3://`
    ///   bucket under a GCS provider is a def that reads one way and registers another.
    /// - **HTTP** — the whole origin (`http://aserver:8484`). `http` and `https` are two
    ///   different origins rather than two ways of reaching one, so the scheme is part of the
    ///   address and the person typing it is the only one who knows which it is.
    ///
    /// Never a path, on any provider: the object-store registry keys on scheme and authority
    /// ([`url`](Self::url)), so a path here would register under a key nothing looks up. What
    /// reads a path is the table's own source.
    ///
    /// `alias = "bucket"` is what it was called before HTTP carried its own scheme; a
    /// `project.json` written then still loads.
    #[serde(alias = "bucket")]
    pub address: String,
    /// Which object store this is, and the settings that store takes.
    pub provider: Provider,
    /// **Client options** — `object_store`'s own `ClientConfigKey` map, applied to whichever
    /// store this connection builds: timeouts, proxy, HTTP version, user agent.
    ///
    /// Here rather than inside a [`Provider`], and it is the one thing on a connection that
    /// genuinely is not the provider's: all three stores are built on the same HTTP client, and
    /// `with_config` takes the same keys for each. A per-provider copy would be the same table
    /// three times.
    ///
    /// A map rather than a list, because a key set twice has no meaning; the editor edits it as
    /// rows and commits it as this. Which names are legal, and what a blank value does, are
    /// `strata_core::engine::store`'s answer (`check_client_config`) — the keys are
    /// `object_store`'s vocabulary and this crate does not depend on it.
    ///
    /// Absent when empty, so a project file gains nothing until a connection sets one, and a def
    /// written before the field existed still loads.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub client_config: BTreeMap<String, String>,
}

impl ConnectionDef {
    /// The URL this connection registers under — `s3://acme-lake`, `gs://lake`,
    /// `http://aserver:8484`. Scheme + authority and nothing else, because that is the whole of
    /// what the object-store registry keys on.
    ///
    /// The two object stores compose it from their provider's scheme; HTTP's address **is** this
    /// URL, so there is nothing to compose. Which is also why there is no `Provider::scheme`: two
    /// of the three could answer and the third could not.
    pub fn url(&self) -> String {
        match self.provider {
            Provider::Http => self.address.clone(),
            Provider::S3(_) => format!("s3://{}", self.address),
            Provider::Gcs(_) => format!("gs://{}", self.address),
        }
    }

    /// Upgrade a def written before an HTTP address carried its own scheme.
    ///
    /// `serde(alias = "bucket")` migrates the field *name*; this migrates the **value**, and
    /// without it an HTTP connection saved under the older shape breaks on the next open. It
    /// stored the authority alone (`example.com`) and the code derived `https`, so a bare
    /// authority now reads as a URL with no scheme — which [`Provider::check_address`] refuses,
    /// turning a connection that worked into an amber row asking for something the user never
    /// had to type. Prepending `https://` restores exactly the URL the old `url()` composed.
    ///
    /// A no-op for everything else: the two object stores never stored a scheme, and an HTTP
    /// address that already has one is left alone.
    pub fn migrated(mut self) -> Self {
        let bare = !self.address.contains("://");
        if matches!(self.provider, Provider::Http) && bare && !self.address.trim().is_empty() {
            self.address = format!("https://{}", self.address);
        }
        self
    }
}

/// **A connection's provider, and the settings that provider takes** — one field, not a
/// provider string beside a settings bag.
///
/// The same argument as [`SourceFormat`](crate::SourceFormat): the two are not independent.
/// A region means nothing to the HTTP store, and a def carrying both a provider and every
/// provider's fields has states where they disagree — an S3 region set on a GCS bucket,
/// silently ignored, and shown by whatever surface renders it. Here the provider *is* the
/// settings, so that state cannot be written down.
///
/// Three providers, and deliberately no fourth: Azure was dropped in the spec's v11.
/// S3-compatible stores (Cloudflare R2, MinIO, Alibaba OSS, Tencent COS) ride [`S3`](Self::S3)
/// via its [`endpoint`](S3Store::endpoint) rather than each becoming a provider of its own.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "provider", rename_all = "lowercase")]
pub enum Provider {
    S3(S3Store),
    Gcs(GcsStore),
    /// A public HTTP(S) origin. No settings and no auth: reads are anonymous, and the one thing
    /// there is to say — `http` or `https` — is part of the
    /// [address](ConnectionDef::address) itself.
    Http,
}

impl Provider {
    /// Which provider this is, without its settings.
    pub fn id(&self) -> ProviderId {
        match self {
            Self::S3(_) => ProviderId::S3,
            Self::Gcs(_) => ProviderId::Gcs,
            Self::Http => ProviderId::Http,
        }
    }

    /// Whether `address` is one this provider will actually accept — **checked here, so the
    /// engine and the connection editor cannot disagree about it**.
    ///
    /// Three different questions, because the three providers address different things: S3 has no
    /// underscores where GCS does, GCS reserves `goog` and `google` where S3 does not, and HTTP is
    /// not asking about a bucket name at all but about a URL. A form that kept its own copy of any
    /// of that would drift from the store's the first time either changed, so both call this.
    ///
    /// **Not exhaustive, on purpose.** Each provider reserves further names that no local check
    /// can settle — S3's `xn--` / `sthree-` prefixes and `-s3alias` / `--ol-s3` suffixes, GCS's
    /// "close misspellings" of `google` — and a bucket that exists is still a bucket you may not
    /// be able to read. This catches what is *statically* wrong, so the user is told at the field
    /// instead of by a signing error; the store remains the authority on the rest.
    pub fn check_address(&self, address: &str) -> Result<(), String> {
        match self {
            Self::S3(_) => check_s3_bucket(address),
            Self::Gcs(_) => check_gcs_bucket(address),
            Self::Http => check_http_url(address),
        }
    }
}

/// How a provider is **named to the user** — `S3` / `GCS` / `HTTP`.
///
/// Deliberately not the URL's own word for it (`s3`, `gs`, `https`), which belongs to the
/// registry rather than to a reader. The two say different things about the same
/// value and both are needed, which is why the product's name lives here and not at whichever
/// surface happened to want it first: the Connections pane's row badge and the connection
/// editor's provider picker (W7 · 03) have to agree, and a name typed twice is a name that can
/// disagree.
impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id().label())
    }
}

/// **Which provider, with no settings attached** — what a picker offers, and where the product's
/// name and the URL's scheme are each written down once.
///
/// [`Provider`] cannot be that picker's value: every arm but HTTP carries that provider's own
/// settings, so an option list built from it would have to invent a settings bag per option and
/// then throw it away on the one the user picks. This is the discriminant on its own, and
/// [`Provider::id`] is the projection — so the badge, the picker and the registry key all read
/// the same two tables rather than three copies of them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProviderId {
    S3,
    Gcs,
    Http,
}

impl ProviderId {
    /// The providers a picker offers, in the order it offers them (spec §1).
    pub const ALL: [ProviderId; 3] = [Self::S3, Self::Gcs, Self::Http];

    /// The product's name for this provider — see [`Display for Provider`](Provider).
    pub fn label(self) -> &'static str {
        match self {
            Self::S3 => "S3",
            Self::Gcs => "GCS",
            Self::Http => "HTTP",
        }
    }
}

// --- addresses --------------------------------------------------------------------------
//
// One home for each provider's published rules, called by `engine::store::connect` and by the
// connection editor alike (see [`Provider::check_address`]). Every message names the provider,
// because a def is edited in a form headed by that provider's picker and read on a row badged
// with it — and the three really do differ.

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
fn check_s3_bucket(bucket: &str) -> Result<(), String> {
    if bucket.is_empty() {
        return Err("This connection has no bucket.".into());
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
    // Counted after the charset check, so this is bytes and characters alike.
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

/// <https://cloud.google.com/storage/docs/buckets#naming>. Deliberately **not** the same rules as
/// S3's: GCS allows underscores, allows a dotted name up to [`GCS_DOTTED_MAX`], and reserves
/// Google's own name.
///
/// The two rules left to the store: "close misspellings" of `google` (`g00gle`), which has no
/// local definition, and the ownership verification a dotted name requires.
fn check_gcs_bucket(bucket: &str) -> Result<(), String> {
    if bucket.is_empty() {
        return Err("This connection has no bucket.".into());
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
    // Before the length rules, because a name ending in a dot fails both — and "it can't end
    // with a dot" is the fault, where "a part is 1 to 63 characters" is a description of the
    // empty part that leaves behind.
    if !starts_and_ends_alphanumeric(bucket) {
        return Err("A GCS bucket name has to start and end with a letter or number.".into());
    }
    match bucket.contains('.') {
        // A dotted name is a domain name, so the cap is the whole name's and each part's.
        true => {
            if !(BUCKET_MIN..=GCS_DOTTED_MAX).contains(&bucket.len()) {
                return Err(format!(
                    "A GCS bucket name containing dots is {BUCKET_MIN} to {GCS_DOTTED_MAX} \
                     characters long."
                ));
            }
            // Each part 1 to 63: the upper bound is Google's own, and a part of *no* length is
            // the `a..b` case, which is not a name any DNS label can carry.
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

/// An HTTP connection's address is a **whole origin URL** — `http://aserver:8484` — and it is
/// written in one box, scheme included, because `http` and `https` are two different origins and
/// only the person typing knows which their server speaks.
///
/// Everything after the authority is refused rather than trimmed away. The object-store registry
/// keys on scheme and authority, so a path here would register under a key nothing looks up while
/// the field went on showing it; and a path is not lost by being refused — it belongs to the
/// source of whatever table reads through this connection.
fn check_http_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("This connection has no URL.".into());
    }
    if url.chars().any(char::is_whitespace) {
        return Err("An HTTP URL can't contain spaces.".into());
    }
    let Some(authority) = ["http://", "https://"]
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme))
    else {
        return Err(
            "An HTTP connection needs a scheme: write 'https://aserver' or 'http://aserver'."
                .into(),
        );
    };
    if authority.is_empty() {
        return Err("An HTTP connection needs a host after its scheme.".into());
    }
    // **Userinfo is refused rather than carried.** `https://alice:hunter2@files.example.com` is a
    // well-formed origin and the common way a protected file drop is handed around, so it gets
    // pasted into this box — and every word of this def rides in `.strata/project.json`, which is
    // committed and shared. Refusing it here is what keeps the module's promise that nothing in a
    // def is a secret; a credential belongs in the keystore, not in a URL, and no provider Strata
    // supports authenticates this way.
    //
    // Asked of the **host part only**, and so before the path is trimmed off below: an `@` in a
    // path is not userinfo, and answering that one with this message would name the wrong half.
    let host = &authority[..authority.find(['/', '?', '#']).unwrap_or(authority.len())];
    if let Some(at) = host.find('@') {
        return Err(format!(
            "An HTTP connection can't carry a username or password. Drop '{}' from the URL.",
            &host[..=at],
        ));
    }
    // `/` is the path, and `?` / `#` are what a URL puts after one — all three are the table's
    // to carry, not the connection's.
    if let Some(at) = authority.find(['/', '?', '#']) {
        return Err(format!(
            "An HTTP connection is an origin, not a path. Drop '{}' and give it to the table \
             that reads through this connection.",
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

/// How an S3 (or S3-compatible) bucket is reached.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct S3Store {
    /// **Required**, and load-bearing: `object_store` does not derive a bucket's region
    /// reliably (arrow-rs#2795) and silently defaults to `us-east-1`, which reads a
    /// different bucket's worth of nothing. `strata_core::engine::store` refuses a blank one
    /// rather than letting that default stand.
    pub region: String,
    pub auth: S3Auth,
    /// An S3-**compatible** endpoint (R2 / MinIO / OSS / COS). Empty means AWS itself.
    pub endpoint: String,
    /// Allow a plain-`http` endpoint — a MinIO on a workstation. Only meaningful with
    /// [`endpoint`](Self::endpoint) set; AWS is HTTPS.
    pub allow_http: bool,
}

/// Where S3 credentials come from. Every mode is secret-free by construction — there is no
/// variant carrying a key, a secret or a token, and that absence is the feature.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum S3Auth {
    /// The host's own chain, whatever it happens to be: environment variables, then the
    /// `~/.aws` profiles, SSO, `credential_process`, web identity, ECS, IMDS.
    #[default]
    Ambient,
    /// One **named** profile from `~/.aws/config` — a reference to the user's own AWS
    /// configuration, never its contents.
    Profile { name: String },
    /// Unsigned requests: a public bucket.
    Anonymous,
}

/// How a GCS bucket is reached. Native to `object_store`; no extra SDK.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct GcsStore {
    pub auth: GcsAuth,
}

/// Where GCS credentials come from — secret-free like [`S3Auth`].
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum GcsAuth {
    /// Application Default Credentials: `GOOGLE_APPLICATION_CREDENTIALS`, then the gcloud
    /// ADC file, then the GCE/GKE metadata server.
    #[default]
    Ambient,
    /// A service-account JSON key **file path**. The path, never the key: inline SA JSON is
    /// exactly the secret this feature refuses to hold.
    ServiceAccount { path: String },
    /// Unsigned requests: a public bucket.
    Anonymous,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ConnectionDef {
        serde_json::from_str(json).expect("a connection def")
    }

    /// **An object store's scheme is its provider's; an HTTP connection's address is already the
    /// URL.** Both halves matter: the first is why an `s3://` bucket under a GCS provider cannot
    /// be written down, and the second is why the editor's HTTP box takes `http://aserver:8484`
    /// whole rather than a host beside a scheme picker.
    #[test]
    fn a_connections_url_is_its_address_in_its_providers_terms() {
        assert_eq!(
            ConnectionDef {
                address: "acme-lake".into(),
                provider: Provider::S3(S3Store::default()),
                client_config: Default::default(),
            }
            .url(),
            "s3://acme-lake"
        );
        assert_eq!(
            ConnectionDef {
                address: "lake".into(),
                provider: Provider::Gcs(GcsStore::default()),
                client_config: Default::default(),
            }
            .url(),
            "gs://lake"
        );
        for written in ["https://example.com:8080", "http://aserver:8484"] {
            assert_eq!(
                ConnectionDef {
                    address: written.into(),
                    provider: Provider::Http,
                    client_config: Default::default(),
                }
                .url(),
                written,
                "an HTTP address is registered exactly as it was written"
            );
        }
    }

    /// The product's name and the URL's word are different strings for the same provider, and
    /// both are load-bearing: the badge says `GCS` where the registry key says `gs`. Pinned so a
    /// later edit cannot quietly collapse one into the other.
    ///
    /// Asserted through **both** vocabularies at once — the settings-carrying [`Provider`] the
    /// catalog stores and the settings-free [`ProviderId`] a picker offers — because the whole
    /// point of the second is that it is not a second copy of the table.
    ///
    /// There is no scheme here at all: two of the three providers state one and HTTP's is inside
    /// its address, so an answer on `ProviderId` could only be a guess for the third.
    #[test]
    fn a_provider_is_named_for_the_reader() {
        for (provider, id, name) in [
            (Provider::S3(S3Store::default()), ProviderId::S3, "S3"),
            (Provider::Gcs(GcsStore::default()), ProviderId::Gcs, "GCS"),
            (Provider::Http, ProviderId::Http, "HTTP"),
        ] {
            assert_eq!(provider.to_string(), name);
            assert_eq!(provider.id(), id);
            assert_eq!(id.label(), name);
        }
        // Every provider is offered, so a fourth one cannot be added without the picker gaining
        // it too.
        assert_eq!(
            ProviderId::ALL.len(),
            3,
            "the picker offers every provider there is"
        );
    }

    fn s3() -> Provider {
        Provider::S3(S3Store::default())
    }

    fn gcs() -> Provider {
        Provider::Gcs(GcsStore::default())
    }

    fn http() -> Provider {
        Provider::Http
    }

    /// Every S3 rule, from AWS's own list — refused **here** rather than by a signing error on
    /// whatever table reads the bucket, which is the whole reason this is not left to the store.
    #[test]
    fn an_s3_bucket_name_follows_amazons_rules() {
        // The shape the rules describe, and the things they permit that look odd: dots, hyphens,
        // digits at either end, and the shortest and longest names there are.
        for good in [
            "acme-lake",
            "acme.lake.eu",
            "3lake9",
            "abc",
            &"a".repeat(63),
        ] {
            assert_eq!(s3().check_address(good), Ok(()), "{good}");
        }
        for (bad, why) in [
            ("", "no bucket"),
            ("ab", "3 to 63"),
            (&"a".repeat(64), "3 to 63"),
            ("Acme-Lake", "lowercase"),
            ("acme_lake", "lowercase"),
            ("acme lake", "lowercase"),
            ("acme/lake", "lowercase"),
            ("-acme-lake", "start and end"),
            ("acme-lake-", "start and end"),
            (".acme", "start and end"),
            ("acme..lake", "two dots"),
            ("192.168.5.4", "ip address"),
        ] {
            let message = s3().check_address(bad).expect_err(bad);
            assert!(message.to_lowercase().contains(why), "{bad}: {message}");
        }
        // The rule names the *format*, not every dotted name: `999.1.1.1` is no address, and both
        // providers accept it. Asserted so the two checkers cannot drift apart on it either.
        assert_eq!(s3().check_address("999.1.1.1"), Ok(()));
        assert_eq!(gcs().check_address("999.1.1.1"), Ok(()));
    }

    /// GCS's rules are **not** S3's, and the differences are the point: an underscore is legal
    /// here and not there, a dotted name may run to 222, and Google reserves its own name.
    #[test]
    fn a_gcs_bucket_name_follows_googles_rules() {
        for good in [
            "acme_lake",
            "acme-lake",
            "3lake9",
            // A dotted name well past S3's 63, with every part inside 63.
            &format!("{}.{}.{}", "a".repeat(63), "b".repeat(63), "c".repeat(63)),
            // Four parts, but not four octets — so not an address, and not refused as one.
            "999.1.1.1",
        ] {
            assert_eq!(gcs().check_address(good), Ok(()), "{good}");
        }
        for (bad, why) in [
            ("", "no bucket"),
            ("ab", "3 to 63"),
            (&"a".repeat(64), "3 to 63"),
            ("Acme-Lake", "lowercase"),
            ("acme lake", "lowercase"),
            ("-acme", "start and end"),
            ("acme.", "start and end"),
            ("192.168.5.4", "ip address"),
            ("googly-data", "'goog'"),
            ("not-google-really", "'google'"),
            // Dotted, so 222 is the cap it breaks: 4 x 63 plus its dots is 255.
            (&[&"a".repeat(63)[..]; 4].join("."), "3 to 222"),
            (&format!("{}.b", "a".repeat(64)), "1 to 63"),
            ("acme..lake", "1 to 63"),
        ] {
            let message = gcs().check_address(bad).expect_err(bad);
            assert!(message.to_lowercase().contains(why), "{bad}: {message}");
        }
    }

    /// **An HTTP address is a whole URL, written in one box** — neither of the two rule sets
    /// above applies, because there is no bucket here to name.
    ///
    /// A path is the case worth pinning: it is refused rather than trimmed off, and the message
    /// quotes the part to drop, because a URL silently shortened to its origin is a field showing
    /// one thing while the connection means another.
    #[test]
    fn an_http_address_is_a_whole_url() {
        for good in [
            "http://aserver:8484",
            "https://example.com",
            "https://example.com:8080",
            "http://localhost:9000",
        ] {
            assert_eq!(http().check_address(good), Ok(()), "{good}");
        }
        for (bad, why) in [
            ("", "no url"),
            ("aserver:8484", "needs a scheme"),
            ("ftp://aserver", "needs a scheme"),
            ("https://", "needs a host"),
            ("https://aserver:8484/fake", "not a path"),
            ("https://aserver:8484/", "not a path"),
            ("https://aserver?x=1", "not a path"),
            ("https://a server", "spaces"),
            // Userinfo: a well-formed origin, and the one shape that would put a password in a
            // committed file.
            ("https://alice:hunter2@files.example.com", "password"),
            ("https://alice@files.example.com", "password"),
        ] {
            let message = http().check_address(bad).expect_err(bad);
            assert!(message.to_lowercase().contains(why), "{bad}: {message}");
        }
        // The offending part is named, so the fix is obvious from the message alone.
        let message = http()
            .check_address("https://aserver:8484/fake")
            .expect_err("a path");
        assert!(message.contains("'/fake'"), "{message}");
        let message = http()
            .check_address("https://alice:hunter2@files.example.com")
            .expect_err("userinfo");
        assert!(message.contains("'alice:hunter2@'"), "{message}");
        // An `@` in a *path* is not userinfo, and is answered by the path's own message rather
        // than by one naming a credential that isn't there.
        let message = http()
            .check_address("https://aserver/mail@home")
            .expect_err("a path");
        assert!(message.contains("'/mail@home'"), "{message}");
    }

    #[test]
    fn each_provider_round_trips_with_its_own_settings() {
        for def in [
            ConnectionDef {
                address: "acme-lake".into(),
                provider: Provider::S3(S3Store {
                    region: "eu-west-2".into(),
                    auth: S3Auth::Profile {
                        name: "analytics".into(),
                    },
                    endpoint: "https://s3.example.net".into(),
                    allow_http: true,
                }),
                client_config: Default::default(),
            },
            ConnectionDef {
                address: "lake".into(),
                provider: Provider::Gcs(GcsStore {
                    auth: GcsAuth::ServiceAccount {
                        path: "/keys/reader.json".into(),
                    },
                }),
                client_config: Default::default(),
            },
            ConnectionDef {
                address: "http://aserver:8484".into(),
                provider: Provider::Http,
                client_config: Default::default(),
            },
        ] {
            let json = serde_json::to_string(&def).expect("serialize");
            assert_eq!(parse(&json), def, "{json}");
        }
    }

    /// The persisted shape is the one `docs/CONNECTIONS_SPEC.md` §5 describes: a `provider`
    /// tag beside that provider's own non-secret fields. Pinned as literal JSON because the
    /// file is committed and shared — a round-trip through today's structs could not catch a
    /// tag or a field name changing under it.
    #[test]
    fn the_persisted_shape_is_the_tagged_provider() {
        let json = serde_json::to_string(&ConnectionDef {
            address: "acme-lake".into(),
            provider: Provider::S3(S3Store {
                region: "eu-west-2".into(),
                auth: S3Auth::Profile {
                    name: "analytics".into(),
                },
                ..Default::default()
            }),
            client_config: Default::default(),
        })
        .expect("serialize");
        assert_eq!(
            json,
            r#"{"address":"acme-lake","provider":{"provider":"s3","region":"eu-west-2","auth":{"mode":"profile","name":"analytics"},"endpoint":"","allow_http":false}}"#
        );
        assert_eq!(
            serde_json::to_string(&Provider::Http).expect("serialize"),
            r#"{"provider":"http"}"#
        );
        assert_eq!(
            serde_json::to_string(&GcsAuth::ServiceAccount {
                path: "/keys/r.json".into()
            })
            .expect("serialize"),
            r#"{"mode":"service-account","path":"/keys/r.json"}"#
        );
    }

    /// **A stored HTTP connection keeps working across the rename.** `serde(alias)` carries the
    /// field name; this carries the value, and it is the half that would otherwise break a
    /// project silently: the old shape stored the authority alone and derived `https`, so without
    /// the migration the def reads as a URL with no scheme and the connection is refused, asking
    /// for something the user never had to type.
    #[test]
    fn a_stored_http_connection_keeps_the_scheme_it_was_registered_under() {
        let old =
            parse(r#"{"bucket":"example.com:8080","provider":{"provider":"http"}}"#).migrated();
        assert_eq!(old.address, "https://example.com:8080");
        assert_eq!(
            old.url(),
            "https://example.com:8080",
            "the URL the old code composed"
        );
        assert_eq!(old.provider.check_address(&old.address), Ok(()));

        // An address that already carries a scheme is left exactly as it is — including a plain
        // `http` one, which is a different origin and must not be promoted.
        for written in ["http://aserver:8484", "https://aserver:8484"] {
            let def = ConnectionDef {
                address: written.into(),
                provider: Provider::Http,
                client_config: Default::default(),
            };
            assert_eq!(def.clone().migrated(), def, "{written}");
        }
        // And it is a no-op for the two object stores, whose address never held a scheme.
        let bucket = ConnectionDef {
            address: "acme-lake".into(),
            provider: Provider::S3(S3Store::default()),
            client_config: Default::default(),
        };
        assert_eq!(bucket.clone().migrated(), bucket);
    }

    /// A provider's settings are all `#[serde(default)]`, so a def written before a setting
    /// existed still loads — the same rule the session snapshot's per-tab facets follow, and
    /// for the same reason: the file on disk is older than the code reading it after every
    /// release.
    ///
    /// The **address** carries that rule too, one step further: it was called `bucket` until HTTP
    /// started holding a whole URL, so every def below is written the old way and has to load
    /// exactly as it did (`serde(alias)`). A project file is committed and shared; a rename that
    /// silently emptied a field would take the connection with it.
    #[test]
    fn a_def_predating_a_setting_loads_with_its_default() {
        let def =
            parse(r#"{"bucket":"acme-lake","provider":{"provider":"s3","region":"us-east-1"}}"#);
        assert_eq!(def.address, "acme-lake", "the old field name still loads");
        assert_eq!(
            def.provider,
            Provider::S3(S3Store {
                region: "us-east-1".into(),
                auth: S3Auth::Ambient,
                endpoint: String::new(),
                allow_http: false,
            })
        );
        assert_eq!(
            parse(r#"{"bucket":"lake","provider":{"provider":"gcs"}}"#).provider,
            Provider::Gcs(GcsStore {
                auth: GcsAuth::Ambient
            })
        );
    }
}
