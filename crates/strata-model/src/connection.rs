//! **Connections** — the persisted description of one remote source a project reads from: an
//! object store, or a source served by a registered kind. Exactly what `.strata/project.json`
//! stores, like the catalog defs beside it. Spec: `docs/CONNECTIONS_SPEC.md`.
//!
//! The rule the whole feature is built around: **no arm of this module holds a secret value.** A
//! connection carries non-secret metadata plus, where credentials are needed, a *reference* to
//! where they live — a named `~/.aws` profile, a key **file path**, or the bare expectation that
//! this machine's keystore holds one ([`SourceDef::secrets`]). Nothing here has to be gitignored,
//! which is why the def rides the committed `project.json`.
//!
//! The keystore slot is **derived** from the connection's own identity and the key it is for, so
//! the committed def carries no machine-local id and two colleagues' keystores never fight over
//! it through git.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

/// One project-scoped connection: the address it names, and the provider that serves it.
///
/// **Identity is [`identity`](Self::identity), not the address** — the kind *and* the address,
/// so `s3:lake` and `gcs:lake` share a bucket name and are two different connections over two
/// different stores.
///
/// **[`address`](Self::address) is not a bucket name**, which is why it is not called one: an
/// object store is addressed by a bucket, an HTTP origin by the URL itself, and a database by
/// `host:port/database`. What a *provider* makes of it is the provider's business.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ConnectionDef {
    /// Where this connection points, in the terms its provider uses.
    ///
    /// - **S3 / GCS** — the bucket name alone (`acme-lake`); the scheme is the provider's.
    /// - **HTTP** — the whole origin (`http://aserver:8484`), because `http` and `https` are two
    ///   different origins rather than two ways of reaching one.
    /// - **A registered source** — whatever its kind says an address is (`host:port/database` for
    ///   `PostgreSQL`), judged by that kind's own rule rather than by anything here.
    ///
    /// Never a path on an object store: the registry keys on scheme and authority, so a path there
    /// would register under a key nothing looks up. `alias = "bucket"` is what this was called
    /// before HTTP carried its own scheme.
    #[serde(alias = "bucket")]
    pub address: String,
    /// **What this connection is called** — the name the catalog tree shows, the editor titles, a
    /// table def points at, and, for a source, the catalog half of `catalog.schema.table`.
    ///
    /// One field for all of it, so what a user renames is what queries say. Minted from the
    /// address for a def that predates it ([`mint_name`]) and edited freely afterwards, within
    /// [`check_catalog`]'s rules.
    #[serde(default, alias = "catalog", skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Which object store this is, and the settings that store takes.
    pub provider: Provider,
    /// **Client options** — `object_store`'s own `ClientConfigKey` map: timeouts, proxy, HTTP
    /// version, user agent.
    ///
    /// Outside [`Provider`] because it is the one thing on a connection that is not the provider's:
    /// all three stores are built on the same HTTP client, so a per-provider copy would be the same
    /// table three times. A map rather than a list, because a key set twice has no meaning.
    ///
    /// Which names are legal is `strata_engine::store`'s answer (`check_client_config`) —
    /// the keys are `object_store`'s vocabulary and this crate does not depend on it. **Empty on a
    /// database connection**, which speaks no HTTP.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub client_config: BTreeMap<String, String>,
}

impl ConnectionDef {
    /// What this connection **is**, as one string: `s3:acme-lake`, `http:http://aserver:8484`,
    /// `postgres:db.internal:5432/analytics`.
    ///
    /// The pair `(kind, address)` — the two fields that make two connections different things —
    /// spelled for the places that need one value: the store's rows, the registry keys, the
    /// keystore slot, a table def's reference to the connection it reads through.
    ///
    /// **Not a URL.** The scheme a remote path is registered under is the engine's own rendering
    /// and belongs to whatever serves the connection, so it is composed there and never here.
    /// Split this in one place if it must be split at all: a kind holds no colon, so the first one
    /// separates the halves.
    pub fn identity(&self) -> String {
        self.provider.identity(&self.address)
    }

    /// What this connection is **called** — [`name`](Self::name) trimmed, which is the key the
    /// project's rows, a table def's reference and every surface address it by.
    ///
    /// A def written before names had a field is called what its address mints, so an older
    /// project opens with every connection named rather than with a row called nothing.
    pub fn named(&self) -> String {
        match self.name.trim() {
            "" => mint_name(&self.address),
            named => named.to_string(),
        }
    }

    /// The catalog this connection registers, or `None` for one that registers an object store.
    ///
    /// A source's relations are addressed through a catalog and that catalog **is** the
    /// connection's name ([`named`](Self::named)) — one field, so the tree's mark, the Forget
    /// confirm, the remote scans a window keeps and the names completion offers cannot spell it
    /// differently. Asked of the def rather than of a live engine on purpose: a connection that
    /// has never answered still has to say what a query would have to write.
    pub fn catalog(&self) -> Option<String> {
        self.provider.source().map(|_| self.named())
    }

    /// Upgrade a def written before an HTTP address carried its own scheme.
    ///
    /// `serde(alias = "bucket")` migrates the field *name*; this migrates the **value**. The older
    /// shape stored the authority alone and derived `https`, so a bare authority now reads as a URL
    /// with no scheme, which [`Provider::check_address`] refuses. Prepending `https://` restores
    /// exactly the URL the old `url()` composed.
    ///
    /// A no-op for everything else.
    pub fn migrated(mut self) -> Self {
        let bare = !self.address.contains("://");
        if matches!(self.provider, Provider::Http) && bare && !self.address.trim().is_empty() {
            self.address = format!("https://{}", self.address);
        }
        self
    }
}

/// **A connection's provider, and the settings that provider takes** — one field, not a provider
/// string beside a settings bag, so an S3 region set on a GCS bucket is a state that cannot be
/// written down. The same argument as [`SourceFormat`](crate::SourceFormat).
///
/// Three object stores, and deliberately no fourth: S3-compatible stores (R2, MinIO, OSS, COS) ride
/// [`S3`](Self::S3) via its [`endpoint`](S3Store::endpoint).
///
/// [`Source`](Self::Source) is one arm for every source a registered kind serves, shipped and
/// embedder-written alike: the engine's registry is keyed by that `kind` string, and the arm is
/// the string plus the settings the kind declares. A source registers a catalog of relations
/// where the object stores register a store, and that difference lives entirely in
/// `strata_engine`: everything here is the same def, `Reg` row, editor window, registration pass
/// and Forget confirm.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "provider", rename_all = "lowercase")]
pub enum Provider {
    S3(S3Store),
    Gcs(GcsStore),
    /// A public HTTP(S) origin. No settings and no auth: reads are anonymous, and the one thing
    /// there is to say — `http` or `https` — is part of the
    /// [address](ConnectionDef::address) itself.
    Http,
    Source(SourceDef),
}

impl Provider {
    /// Which provider this is, without its settings.
    pub fn id(&self) -> ProviderId {
        match self {
            Self::S3(_) => ProviderId::S3,
            Self::Gcs(_) => ProviderId::Gcs,
            Self::Http => ProviderId::Http,
            Self::Source(_) => ProviderId::Source,
        }
    }

    /// What this connection is, given the address it names — [`ConnectionDef::identity`]'s one
    /// mint site.
    ///
    /// The typed arms know their own kind; a [`Source`](Self::Source) carries it.
    pub fn identity(&self, address: &str) -> String {
        let address = address.trim();
        match self {
            Self::S3(_) => format!("s3:{address}"),
            Self::Gcs(_) => format!("gcs:{address}"),
            Self::Http => format!("http:{address}"),
            Self::Source(source) => source.identity(address),
        }
    }

    /// What a registered kind serves, or `None` for a provider that registers an object store.
    pub fn source(&self) -> Option<&SourceDef> {
        match self {
            Self::S3(_) | Self::Gcs(_) | Self::Http => None,
            Self::Source(source) => Some(source),
        }
    }

    /// Whether `address` is one this provider will actually accept — **checked here, so the engine
    /// and the connection editor cannot disagree about it**.
    ///
    /// Three different questions, because the object stores address different things. **Not
    /// exhaustive, on purpose:** each reserves further names no local check can settle, and a
    /// bucket that exists is still one you may not be able to read. This catches what is
    /// *statically* wrong, so the user is told at the field instead of by a signing error.
    ///
    /// A [`Source`](Self::Source) is not asked here at all: what an address means is the kind's
    /// own rule, and the engine routes to it through the registry.
    pub fn check_address(&self, address: &str) -> Result<(), String> {
        match self {
            Self::S3(_) => check_s3_bucket(address),
            Self::Gcs(_) => check_gcs_bucket(address),
            Self::Http => check_http_url(address),
            Self::Source(_) => Ok(()),
        }
    }
}

/// One data source, in the terms the kind that serves it declares.
///
/// Flat and kind-keyed for every source alike: what a typed arm per source would state in fields
/// is [`config`](Self::config), whose keys are the kind's own declaration, so a source the engine
/// gains needs no change here and none is more first-class than another. The values are non-secret
/// — a credential is named by [`secrets`](Self::secrets) and stored elsewhere.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct SourceDef {
    /// **Required.** Which kind serves this connection — the registry key, and the prefix of the
    /// keystore family each of its secrets is filed under. A def whose kind nothing answers to
    /// settles failed, naming the fix.
    pub kind: String,
    /// The settings the kind declares, by the keys it documents. Outside this crate's vocabulary
    /// on purpose: what a source is configured by is the source's own business.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, String>,
    /// Which of the kind's secret-typed keys this connection has a value for — the
    /// **expectation**, never a reference and never a value. The values live in this machine's
    /// keystore, or arrive through the kind's own environment convention, so a colleague pulling
    /// the project gets "no entry on this machine, here is the fix" rather than silence.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub secrets: BTreeSet<String>,
    /// The namespaces this connection **shows**: `DataGrip`'s "N of M schemas" choice.
    ///
    /// Display only, never a filter the engine applies — registration exposes every namespace the
    /// connection can see (the providers are lazy, so that costs nothing), and a query naming one
    /// that is not enabled still resolves and runs. This scopes the data-sources tree and
    /// completion: "what am I working with", not "what may I read".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub schemas: Vec<String>,
    /// Whether Strata refuses to **change** what this connection holds: `INSERT` into one of its
    /// relations, and `CREATE TABLE … AS SELECT` making one.
    ///
    /// **Default `true`.** The gate is the def rather than a machine-local preference because a
    /// connection is committed and shared — a colleague pulling the project gets the same answer
    /// about the same server.
    pub read_only: bool,
}

/// Hand-written for `read_only` alone: a derived `false` would make a def that omits the field
/// writable.
impl SourceDef {
    /// This source's half of [`ConnectionDef::identity`]: its kind and the address it was given.
    pub fn identity(&self, address: &str) -> String {
        format!("{}:{}", self.kind.trim(), address.trim())
    }
}

impl Default for SourceDef {
    fn default() -> Self {
        Self {
            kind: String::new(),
            config: BTreeMap::new(),
            secrets: BTreeSet::new(),
            schemas: Vec::new(),
            read_only: true,
        }
    }
}

/// **The catalog name a database connection registers under** — checked against the project's other
/// connections, so the engine's registration and the editor's blocker cannot disagree.
///
/// `existing` is the connections to fold `candidate` against, `candidate` excluded: the project's
/// stored defs for the editor, and the sources already registered on the session for
/// `strata_engine::sources::connect`. Different sets on purpose — a connection that failed to
/// connect reserves nothing, which is why the engine's set is the live one.
///
/// A no-op for every provider that registers an object store.
pub fn check_catalog_name(
    existing: &[ConnectionDef],
    candidate: &ConnectionDef,
) -> Result<(), String> {
    let Some(source) = candidate.provider.source() else {
        return Ok(());
    };
    let _ = source;
    check_catalog(&candidate.named())?;
    let name = candidate.named();
    for other in existing {
        if other.identity() == candidate.identity() {
            continue;
        }
        if other.named().eq_ignore_ascii_case(&name) {
            return Err(format!(
                "'{name}' is already the catalog name of the connection '{}'. Give this one a \
                 different name.",
                other.identity()
            ));
        }
    }
    Ok(())
}

/// How a provider is **named to the user** — `S3` / `GCS` / `HTTP`. Deliberately not the URL's own
/// word for it, which belongs to the registry: the row badge and the editor's picker have to agree,
/// and a name typed twice is a name that can disagree.
///
/// A [`Source`](Provider::Source) names itself: its badge is the kind's own, which the engine's
/// registry answers for, and a shared label would badge every source the same.
impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(source) => f.write_str(source.kind.trim()),
            _ => f.write_str(self.id().label()),
        }
    }
}

/// **Which provider, with no settings attached** — what a picker offers.
///
/// [`Provider`] cannot be that picker's value: every arm but HTTP carries settings, so an option
/// list built from it would invent a settings bag per option and throw it away on the one the user
/// picks. This is the discriminant on its own, and [`Provider::id`] is the projection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProviderId {
    S3,
    Gcs,
    Http,
    /// A source served by a registered kind — see [`Provider::Source`].
    Source,
}

impl ProviderId {
    /// The providers a picker offers, in the order it offers them (spec §1). A new arm ships into
    /// every picker silently, which is right for a picker of *providers* and wrong wherever the
    /// question is narrower — that narrower question is [`OBJECT_STORES`](Self::OBJECT_STORES).
    ///
    /// **[`Source`](Self::Source) is deliberately not among them**: which kinds are registered is
    /// the engine's answer, not this crate's, and a picker offering "a source" without saying
    /// which would be offering nothing.
    pub const ALL: [ProviderId; 3] = [Self::S3, Self::Gcs, Self::Http];

    /// The providers that register an **object store** — what a surface offers when the question is
    /// "which connection do these *files* read through".
    ///
    /// A separate list rather than a filter written at each site, because getting it wrong is
    /// silent: Configure's LOCATION TYPE pill would offer a Postgres connection to read parquet
    /// through, and the CONNECTION picker under it would be empty with nothing saying why.
    pub const OBJECT_STORES: [ProviderId; 3] = [Self::S3, Self::Gcs, Self::Http];

    /// Whether this provider registers an object store (rather than a source's catalog) —
    /// [`OBJECT_STORES`](Self::OBJECT_STORES) asked of one value.
    pub fn is_object_store(self) -> bool {
        matches!(self, Self::S3 | Self::Gcs | Self::Http)
    }

    /// The product's name for this provider — see [`Display for Provider`](Provider). A
    /// [`Source`](Self::Source) is named by its own kind, so what is here is the word for one
    /// whose kind is not to hand.
    pub fn label(self) -> &'static str {
        match self {
            Self::S3 => "S3",
            Self::Gcs => "GCS",
            Self::Http => "HTTP",
            Self::Source => "SRC",
        }
    }
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
    let host = &authority[..authority.find(['/', '?', '#']).unwrap_or(authority.len())];
    if let Some(at) = host.find('@') {
        return Err(format!(
            "An HTTP connection can't carry a username or password. Drop '{}' from the URL.",
            &host[..=at],
        ));
    }
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
    /// **Required**: `object_store` does not derive a bucket's region reliably (arrow-rs#2795) and
    /// silently defaults to `us-east-1`, so `strata_engine::store` refuses a blank one.
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

/// A name for a connection at `address`, before anyone has typed one.
///
/// The address's most name-like part, folded to what [`check_catalog`] accepts: the bucket for an
/// object store, the database for a server, the host for an origin. A blank or unusable address
/// mints `source`, which is a name the editor shows rather than a failure it hides.
pub fn mint_name(address: &str) -> String {
    let address = address.trim().trim_end_matches('/');
    let after_scheme = address.split_once("://").map_or(address, |(_, rest)| rest);
    let last = after_scheme
        .rsplit(['/', '@'])
        .next()
        .unwrap_or(after_scheme);
    let bare: String = last
        .split(':')
        .next()
        .unwrap_or(last)
        .chars()
        .map(|c| match c.is_ascii_alphanumeric() {
            true => c.to_ascii_lowercase(),
            false => '_',
        })
        .collect();
    let trimmed = bare.trim_matches('_');
    match trimmed.chars().next() {
        Some(head) if head.is_ascii_alphabetic() || head == '_' => trimmed.to_string(),
        Some(_) => format!("_{trimmed}"),
        None => "source".to_string(),
    }
}

/// [`mint_name`] with a number appended until nothing in `taken` holds it — what a *second*
/// connection to one address is called.
pub fn mint_free_name(address: &str, taken: &[String]) -> String {
    let base = mint_name(address);
    let held = |name: &str| taken.iter().any(|t| t.trim().eq_ignore_ascii_case(name));
    if !held(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|name| !held(name))
        .unwrap_or(base)
}

/// Whether `name` is one a data source may register under, on its own terms — the half of
/// [`check_catalog_name`] that needs no other connection.
///
/// A **bare** SQL identifier, narrower than what DataFusion could resolve, because every surface
/// that renders `pg.public.orders` would otherwise have to quote it. Case-folded against the
/// reserved name, because unquoted identifiers are.
pub fn check_catalog(catalog: &str) -> Result<(), String> {
    let name = catalog.trim();
    if name.is_empty() {
        return Err("This connection has no catalog name.".into());
    }
    let mut chars = name.chars();
    let head = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_');
    if !head || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!(
            "'{name}' is not a name queries can use. A catalog name starts with a letter or \
             '_' and holds only letters, numbers and '_'."
        ));
    }
    if name.eq_ignore_ascii_case(WORKSPACE_CATALOG) {
        return Err(format!(
            "'{WORKSPACE_CATALOG}' is this project's own catalog. Give this connection a \
             different name."
        ));
    }
    Ok(())
}

/// The catalog the project's own tables, views and results live in — what a data source's catalog
/// name may not be ([`check_catalog`]). Here rather than in the engine that registers it, because
/// both crates need it: `strata_engine::CATALOG` reads it.
pub const WORKSPACE_CATALOG: &str = "strata";

#[cfg(test)]
mod tests {
    use std::slice;

    use super::*;

    fn parse(json: &str) -> ConnectionDef {
        serde_json::from_str(json).expect("a connection def")
    }

    /// **A connection is its kind and its address**, and nothing composes a scheme: two providers
    /// over one bucket name are two connections because their kinds differ, not because they
    /// render different URLs.
    #[test]
    fn a_connections_identity_is_its_kind_and_its_address() {
        let def = |address: &str, provider: Provider| ConnectionDef {
            address: address.into(),
            name: String::new(),
            provider,
            client_config: Default::default(),
        };
        assert_eq!(
            def("acme-lake", Provider::S3(S3Store::default())).identity(),
            "s3:acme-lake"
        );
        assert_eq!(
            def("lake", Provider::Gcs(GcsStore::default())).identity(),
            "gcs:lake"
        );
        assert_ne!(
            def("lake", Provider::S3(S3Store::default())).identity(),
            def("lake", Provider::Gcs(GcsStore::default())).identity(),
            "one bucket name, two kinds, two connections"
        );
        for written in ["https://example.com:8080", "http://aserver:8484"] {
            assert_eq!(
                def(written, Provider::Http).identity(),
                format!("http:{written}"),
                "an HTTP address is its own origin, kept exactly as it was written"
            );
        }
    }

    /// The product's name and the URL's word are different strings for the same provider, and both
    /// are load-bearing: the badge says `GCS` where the registry key says `gs`. Asserted through
    /// **both** vocabularies at once, because the whole point of [`ProviderId`] is that it is not a
    /// second copy of the table.
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
        assert_eq!(
            ProviderId::ALL.to_vec(),
            ProviderId::OBJECT_STORES.to_vec(),
            "every provider a picker offers is an object store; a source is named by its kind"
        );
        assert!(!ProviderId::Source.is_object_store());
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
    /// whatever table reads the bucket.
    #[test]
    fn an_s3_bucket_name_follows_amazons_rules() {
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
            &format!("{}.{}.{}", "a".repeat(63), "b".repeat(63), "c".repeat(63)),
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
            (&[&"a".repeat(63)[..]; 4].join("."), "3 to 222"),
            (&format!("{}.b", "a".repeat(64)), "1 to 63"),
            ("acme..lake", "1 to 63"),
        ] {
            let message = gcs().check_address(bad).expect_err(bad);
            assert!(message.to_lowercase().contains(why), "{bad}: {message}");
        }
    }

    /// **An HTTP address is a whole URL, written in one box.** A path is refused rather than
    /// trimmed off, because a URL silently shortened to its origin is a field showing one thing
    /// while the connection means another.
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
            ("https://alice:hunter2@files.example.com", "password"),
            ("https://alice@files.example.com", "password"),
        ] {
            let message = http().check_address(bad).expect_err(bad);
            assert!(message.to_lowercase().contains(why), "{bad}: {message}");
        }
        let message = http()
            .check_address("https://aserver:8484/fake")
            .expect_err("a path");
        assert!(message.contains("'/fake'"), "{message}");
        let message = http()
            .check_address("https://alice:hunter2@files.example.com")
            .expect_err("userinfo");
        assert!(message.contains("'alice:hunter2@'"), "{message}");
        let message = http()
            .check_address("https://aserver/mail@home")
            .expect_err("a path");
        assert!(message.contains("'/mail@home'"), "{message}");
    }

    /// **A catalog name is what queries type**, so it is a bare SQL identifier, it is not the
    /// workspace's own, and it is not another connection's — one answer for the engine's
    /// registration and the editor's blocker alike.
    #[test]
    fn a_catalog_name_is_a_free_sql_identifier() {
        let with = |catalog: &str, address: &str| ConnectionDef {
            address: address.into(),
            name: catalog.into(),
            provider: Provider::Source(SourceDef {
                kind: "postgres".into(),
                ..Default::default()
            }),
            client_config: Default::default(),
        };
        let warehouse = with("warehouse", "a:5432/x");

        assert_eq!(check_catalog_name(&[], &with("pg", "b:5432/y")), Ok(()));
        assert_eq!(
            check_catalog_name(&[], &with("", "b:5432/y")),
            Ok(()),
            "a nameless def is called what its address mints, never refused for having no name"
        );
        for (catalog, why) in [
            ("2pg", "starts with a letter"),
            ("my catalog", "starts with a letter"),
            ("pg-main", "starts with a letter"),
            ("análisis", "starts with a letter"),
            (WORKSPACE_CATALOG, "this project's own catalog"),
            ("STRATA", "this project's own catalog"),
        ] {
            let message = check_catalog_name(&[], &with(catalog, "b:5432/y")).expect_err(catalog);
            assert!(message.contains(why), "{catalog}: {message}");
        }

        let message =
            check_catalog_name(slice::from_ref(&warehouse), &with("WAREHOUSE", "b:5432/y"))
                .expect_err("taken");
        assert!(message.contains("postgres:a:5432/x"), "{message}");
        assert_eq!(
            check_catalog_name(slice::from_ref(&warehouse), &warehouse),
            Ok(())
        );
        assert_eq!(
            check_catalog_name(
                &[warehouse],
                &ConnectionDef {
                    address: "acme-lake".into(),
                    name: String::new(),
                    provider: s3(),
                    client_config: Default::default(),
                }
            ),
            Ok(())
        );
    }

    /// Every provider round-trips, the flat source def included — its `config` is the kind's own
    /// vocabulary, and `secrets` says which of that kind's secret keys are set without saying what
    /// any of them is.
    #[test]
    fn each_provider_round_trips_with_its_own_settings() {
        for def in [
            ConnectionDef {
                address: "acme-lake".into(),
                name: String::new(),
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
                name: String::new(),
                provider: Provider::Gcs(GcsStore {
                    auth: GcsAuth::ServiceAccount {
                        path: "/keys/reader.json".into(),
                    },
                }),
                client_config: Default::default(),
            },
            ConnectionDef {
                address: "http://aserver:8484".into(),
                name: String::new(),
                provider: Provider::Http,
                client_config: Default::default(),
            },
            ConnectionDef {
                address: "db.internal:5432/analytics".into(),
                name: String::new(),
                provider: Provider::Source(SourceDef {
                    kind: "postgres".into(),
                    config: BTreeMap::from([
                        ("user".to_string(), "reader".to_string()),
                        ("sslmode".to_string(), "verify-full".to_string()),
                    ]),
                    secrets: BTreeSet::from(["password".to_string()]),
                    schemas: vec!["public".into(), "analytics".into()],
                    read_only: false,
                }),
                client_config: Default::default(),
            },
        ] {
            let json = serde_json::to_string(&def).expect("serialize");
            assert_eq!(parse(&json), def, "{json}");
        }
    }

    /// The persisted shape is the one `docs/CONNECTIONS_SPEC.md` §5 describes. Pinned as literal
    /// JSON because the file is committed and shared — a round-trip through today's structs could
    /// not catch a tag or a field name changing under it.
    #[test]
    fn the_persisted_shape_is_the_tagged_provider() {
        let json = serde_json::to_string(&ConnectionDef {
            address: "acme-lake".into(),
            name: String::new(),
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
        let json = serde_json::to_string(&ConnectionDef {
            address: "db.internal:5432/analytics".into(),
            name: "warehouse".into(),
            provider: Provider::Source(SourceDef {
                kind: "postgres".into(),
                config: BTreeMap::from([("user".to_string(), "reader".to_string())]),
                secrets: BTreeSet::from(["password".to_string()]),
                schemas: vec!["public".into()],
                ..Default::default()
            }),
            client_config: Default::default(),
        })
        .expect("serialize");
        assert_eq!(
            json,
            r#"{"address":"db.internal:5432/analytics","name":"warehouse","provider":{"provider":"source","kind":"postgres","config":{"user":"reader"},"secrets":["password"],"schemas":["public"],"read_only":true}}"#
        );
    }

    /// **A stored HTTP connection keeps working across the rename.** `serde(alias)` carries the
    /// field name; this carries the value, and without it the def reads as a URL with no scheme
    /// and the connection is refused.
    #[test]
    fn a_stored_http_connection_keeps_the_scheme_it_was_registered_under() {
        let old =
            parse(r#"{"bucket":"example.com:8080","provider":{"provider":"http"}}"#).migrated();
        assert_eq!(old.address, "https://example.com:8080");
        assert_eq!(
            old.identity(),
            "http:https://example.com:8080",
            "the origin the old shape derived, under its kind"
        );
        assert_eq!(old.provider.check_address(&old.address), Ok(()));

        for written in ["http://aserver:8484", "https://aserver:8484"] {
            let def = ConnectionDef {
                address: written.into(),
                name: String::new(),
                provider: Provider::Http,
                client_config: Default::default(),
            };
            assert_eq!(def.clone().migrated(), def, "{written}");
        }
    }

    /// A def that omits a setting reads the `Default`'s answer for it — and for a source that
    /// answer is **read-only**, so a connection nobody opted in cannot be written to.
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
            parse(r#"{"address":"db:5432/x","provider":{"provider":"source","kind":"postgres","name":"pg"}}"#)
                .provider,
            Provider::Source(SourceDef {
                kind: "postgres".into(),
                config: BTreeMap::new(),
                secrets: BTreeSet::new(),
                schemas: Vec::new(),
                read_only: true,
            }),
            "read-only is the one answer 'read_only' has to give"
        );
    }

    /// **A new connection arrives named.** The name is the address's own most name-like part,
    /// folded to something a query can say, so `s3://acme-lake` opens as `acme_lake` rather than
    /// as a blank box.
    #[test]
    fn a_name_is_minted_from_the_address() {
        for (address, minted) in [
            ("acme-lake", "acme_lake"),
            ("db.internal:5432/analytics", "analytics"),
            ("http://aserver:8484", "aserver"),
            ("https://files.example.com/", "files_example_com"),
            ("reader@db:5432/sales", "sales"),
            ("9lives", "_9lives"),
            ("", "source"),
            ("///", "source"),
        ] {
            assert_eq!(mint_name(address), minted, "{address}");
            assert_eq!(check_catalog(&mint_name(address)), Ok(()), "{address}");
        }
    }

    /// A second connection to one address is numbered rather than refused, because the clash is
    /// the *name's* and a name is what a user renames.
    #[test]
    fn a_taken_name_is_numbered() {
        let taken = vec!["acme_lake".to_string(), "acme_lake2".to_string()];
        assert_eq!(mint_free_name("acme-lake", &[]), "acme_lake");
        assert_eq!(mint_free_name("acme-lake", &taken), "acme_lake3");
        assert_eq!(
            mint_free_name("acme-lake", &["ACME_LAKE".to_string()]),
            "acme_lake2",
            "a name is taken however it is cased, because queries fold it"
        );
    }

    /// A source's identity is its kind and its address, so two kinds over one address are two
    /// connections and the same kind twice is one.
    #[test]
    fn a_sources_identity_is_its_kind_and_its_address() {
        let def = |kind: &str| ConnectionDef {
            address: "db.internal:5432/analytics".into(),
            name: String::new(),
            provider: Provider::Source(SourceDef {
                kind: kind.into(),
                ..Default::default()
            }),
            client_config: Default::default(),
        };
        assert_eq!(
            def("postgres").identity(),
            "postgres:db.internal:5432/analytics"
        );
        assert_ne!(def("postgres").identity(), def("mysql").identity());
        assert_eq!(def("postgres").provider.to_string(), "postgres");
    }
}
