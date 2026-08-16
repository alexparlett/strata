//! **Connections** — the persisted description of one remote source a project reads from: an
//! object store (W7) or a database (DB workstream). Exactly what `.strata/project.json`
//! stores, like the catalog defs beside it. Spec: `docs/CONNECTIONS_SPEC.md`.
//!
//! The rule the whole feature is built around: **no arm of this module holds a secret value.** A
//! connection carries non-secret metadata plus, where credentials are needed, a *reference* to
//! where they live — a named `~/.aws` profile, a key **file path**, or the bare expectation that
//! this machine's keystore holds a password ([`PgPassword::Keystore`]). Nothing here has to be
//! gitignored, which is why the def rides the committed `project.json`.
//!
//! The keystore slot is **derived** from the connection's own identity
//! (`SecretRef::derived("pg-password", def.url())`), so the committed def carries no machine-local
//! id and two colleagues' keystores never fight over it through git.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// One project-scoped connection: the address it names, and the provider that serves it.
///
/// **Identity is [`url`](Self::url), not the bucket** — scheme *and* authority, which is what
/// DataFusion's object-store registry keys on. `s3://lake` and `gs://lake` share a bucket and are
/// two different connections over two different stores.
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
    /// - **Postgres** — `host:port/database`. The scheme is the provider's, and the *role* is
    ///   [`PgStore::user`] rather than userinfo, because the form asks for it separately.
    ///
    /// Never a path on an object store: the registry keys on scheme and authority, so a path there
    /// would register under a key nothing looks up. `alias = "bucket"` is what this was called
    /// before HTTP carried its own scheme.
    #[serde(alias = "bucket")]
    pub address: String,
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
    /// The URL this connection is identified by — `s3://acme-lake`, `gs://lake`,
    /// `http://aserver:8484`, `postgres://reader@db.internal:5432/analytics`.
    ///
    /// For an **object store** that is scheme + authority and nothing else, because that is what
    /// the registry keys on. (Which is also why there is no `Provider::scheme`: not every arm could
    /// answer.)
    ///
    /// A **database** connection registers a catalog rather than an object store, so nothing parses
    /// this back — it carries the two further things that make two connections different, the
    /// **database** and the **role**. Two roles over one database really are two connections, with
    /// two sets of visible schemas.
    pub fn url(&self) -> String {
        match &self.provider {
            Provider::Http => self.address.clone(),
            Provider::S3(_) => format!("s3://{}", self.address),
            Provider::Gcs(_) => format!("gs://{}", self.address),
            Provider::Postgres(pg) => {
                format!("postgres://{}@{}", pg.user.trim(), self.address.trim())
            }
        }
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
/// **[`Postgres`](Self::Postgres) is a fourth arm rather than a second kind of thing.** It
/// registers a DataFusion *catalog* where the others register an object store, and that difference
/// lives entirely in `strata_engine`: everything here is the same def, `Reg` row, editor
/// window, registration pass and Forget confirm. A further database would be a further arm, not a
/// generic RDBMS abstraction — the per-arm `match` *is* the mechanism.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "provider", rename_all = "lowercase")]
pub enum Provider {
    S3(S3Store),
    Gcs(GcsStore),
    /// A public HTTP(S) origin. No settings and no auth: reads are anonymous, and the one thing
    /// there is to say — `http` or `https` — is part of the
    /// [address](ConnectionDef::address) itself.
    Http,
    Postgres(PgStore),
}

impl Provider {
    /// Which provider this is, without its settings.
    pub fn id(&self) -> ProviderId {
        match self {
            Self::S3(_) => ProviderId::S3,
            Self::Gcs(_) => ProviderId::Gcs,
            Self::Http => ProviderId::Http,
            Self::Postgres(_) => ProviderId::Postgres,
        }
    }

    /// Whether `address` is one this provider will actually accept — **checked here, so the engine
    /// and the connection editor cannot disagree about it**.
    ///
    /// Four different questions, because the providers address different things. **Not exhaustive,
    /// on purpose:** each provider reserves further names no local check can settle, and a bucket
    /// that exists is still one you may not be able to read. This catches what is *statically*
    /// wrong, so the user is told at the field instead of by a signing error.
    pub fn check_address(&self, address: &str) -> Result<(), String> {
        match self {
            Self::S3(_) => check_s3_bucket(address),
            Self::Gcs(_) => check_gcs_bucket(address),
            Self::Http => check_http_url(address),
            Self::Postgres(_) => check_pg_address(address),
        }
    }
}

/// **The catalog name a database connection registers under** — checked against the project's other
/// connections, so the engine's registration and the editor's blocker cannot disagree.
///
/// `existing` is the connections to fold `candidate` against, `candidate` excluded: the project's
/// stored defs for the editor, and the databases already registered on the session for
/// `strata_engine::db::connect`. Different sets on purpose — a connection that failed to
/// connect reserves nothing, which is why the engine's set is the live one.
///
/// A no-op for every provider that registers an object store.
pub fn check_catalog_name(
    existing: &[ConnectionDef],
    candidate: &ConnectionDef,
) -> Result<(), String> {
    let Provider::Postgres(pg) = &candidate.provider else {
        return Ok(());
    };
    pg.check_catalog()?;
    let name = pg.catalog.trim();
    for other in existing {
        let Provider::Postgres(theirs) = &other.provider else {
            continue;
        };
        if other.url() == candidate.url() {
            continue;
        }
        if theirs.catalog.trim().eq_ignore_ascii_case(name) {
            return Err(format!(
                "'{name}' is already the catalog name of the connection '{}'. Give this one a \
                 different name.",
                other.url()
            ));
        }
    }
    Ok(())
}

/// How a provider is **named to the user** — `S3` / `GCS` / `HTTP`. Deliberately not the URL's own
/// word for it, which belongs to the registry: the row badge and the editor's picker have to agree,
/// and a name typed twice is a name that can disagree.
impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id().label())
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
    Postgres,
}

impl ProviderId {
    /// The providers a picker offers, in the order it offers them (spec §1). A new arm ships into
    /// every picker silently, which is right for a picker of *providers* and wrong wherever the
    /// question is narrower — that narrower question is [`OBJECT_STORES`](Self::OBJECT_STORES).
    pub const ALL: [ProviderId; 4] = [Self::S3, Self::Gcs, Self::Http, Self::Postgres];

    /// The providers that register an **object store** — what a surface offers when the question is
    /// "which connection do these *files* read through".
    ///
    /// A separate list rather than a filter written at each site, because getting it wrong is
    /// silent: Configure's LOCATION TYPE pill would offer a Postgres connection to read parquet
    /// through, and the CONNECTION picker under it would be empty with nothing saying why.
    pub const OBJECT_STORES: [ProviderId; 3] = [Self::S3, Self::Gcs, Self::Http];

    /// Whether this provider registers an object store (rather than a database catalog) —
    /// [`OBJECT_STORES`](Self::OBJECT_STORES) asked of one value.
    pub fn is_object_store(self) -> bool {
        !matches!(self, Self::Postgres)
    }

    /// The product's name for this provider — see [`Display for Provider`](Provider).
    pub fn label(self) -> &'static str {
        match self {
            Self::S3 => "S3",
            Self::Gcs => "GCS",
            Self::Http => "HTTP",
            Self::Postgres => "PG",
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

/// A database connection's address is **`host:port/database`** — the server's own spelling of what
/// you dial, in one box, on HTTP's precedent.
///
/// The port is not optional and not defaulted to 5432: a def whose address reads
/// `db.internal/analytics` while it means `:5432` shows one thing and connects to another. Userinfo
/// is refused for the reason the HTTP arm refuses it.
fn check_pg_address(address: &str) -> Result<(), String> {
    parse_pg_address(address).map(|_| ())
}

/// A database connection's address, taken apart — the **one** parse of `host:port/database`.
///
/// `strata_engine::db` dials with exactly these parts rather than splitting the string a
/// second time: two parses of one grammar drift the first time the shape moves, and a second copy's
/// refusals are unreachable prose that reads like live validation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PgAddress<'a> {
    /// Unbracketed: `[::1]` arrives here as `::1`, which is what a driver's `host=` takes.
    pub host: &'a str,
    pub port: u16,
    pub database: &'a str,
}

/// Read `address` as `host:port/database`, or say what is wrong with it in the field's own terms.
///
/// The **connection-string** rules ([`check_conn_value`]) apply to the host and the database for
/// the same reason they apply to [`PgStore::user`]: all three are interpolated into a libpq string
/// with no quoting.
pub fn parse_pg_address(address: &str) -> Result<PgAddress<'_>, String> {
    if address.is_empty() {
        return Err("This connection has no server.".into());
    }
    if address.chars().any(char::is_whitespace) {
        return Err("A PostgreSQL address can't contain spaces.".into());
    }
    if address.contains("://") {
        return Err(
            "A PostgreSQL address is 'host:port/database', without a scheme. Drop the '://'."
                .into(),
        );
    }
    if let Some(at) = address.find('@') {
        return Err(format!(
            "A PostgreSQL address can't carry a user or password. Drop '{}' and set the user in \
             its own field.",
            &address[..=at],
        ));
    }
    let Some((server, database)) = address.split_once('/') else {
        return Err(
            "A PostgreSQL connection needs a database: write 'host:5432/analytics'.".into(),
        );
    };
    if database.is_empty() {
        return Err(
            "A PostgreSQL connection needs a database: write 'host:5432/analytics'.".into(),
        );
    }
    if database.contains('/') {
        return Err("A PostgreSQL address names one database, so it has one '/'.".into());
    }
    check_conn_value("database", database)?;
    let Some((host, port)) = server.rsplit_once(':') else {
        return Err(format!(
            "A PostgreSQL connection needs a port: write '{server}:5432/{database}'."
        ));
    };
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    if host.is_empty() {
        return Err("A PostgreSQL connection needs a host.".into());
    }
    check_conn_value("host", host)?;
    let port = match port.parse::<u16>() {
        Ok(port) if port > 0 => port,
        _ => return Err(format!("'{port}' is not a port number.")),
    };
    Ok(PgAddress {
        host,
        port,
        database,
    })
}

/// Whether `value` is one a libpq connection string can carry — the rule [`PgStore::check_user`],
/// the host and the database all share.
///
/// **Refused by name, because the layer below refuses it namelessly or not at all.** The driver's
/// parameters are assembled by plain interpolation and its parser reads `\` as an escape and `'` as
/// a quote, so a database named `sales\2024` parses as `sales2024` — the app would connect to and
/// federate a **different database** with nothing saying so. Postgres creates all of these happily;
/// they simply cannot be dialled through this stack.
fn check_conn_value(noun: &str, value: &str) -> Result<(), String> {
    if value.chars().any(char::is_whitespace) {
        return Err(format!("A PostgreSQL {noun} can't contain spaces."));
    }
    match value.chars().find(|c| matches!(c, '=' | '\'' | '\\')) {
        Some(bad) => Err(format!("A PostgreSQL {noun} can't contain '{bad}'.")),
        None => Ok(()),
    }
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

/// How a `PostgreSQL` database is reached, and how it is **addressed in SQL**.
///
/// The [catalog name](Self::catalog) exists because SQL cannot address
/// `postgres://host:5432/analytics`: the connection registers a DataFusion catalog, and its
/// relations have to be reachable as `pg.public.orders`. The user's choice rather than derived from
/// the database name, because two servers' `analytics` databases would derive the same name.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct PgStore {
    /// **Required.** How queries name this database — the catalog half of
    /// `catalog.schema.table`. A valid SQL identifier, distinct from the workspace's own
    /// catalog and from every other connection's ([`check_catalog_name`]).
    pub catalog: String,
    /// **Required.** The role this connection logs in as, and half its
    /// [identity](ConnectionDef::url): two roles over one database see two sets of schemas.
    pub user: String,
    pub sslmode: PgSslMode,
    /// A root-certificate **file path**, for the two verifying modes — the
    /// [`GcsAuth::ServiceAccount`] rule: the file is the user's own.
    pub sslrootcert: String,
    /// Whether this connection expects a password in this machine's OS keystore. The
    /// **expectation**, never a reference — see [`PgPassword`].
    pub password: PgPassword,
    /// The schemas this connection **shows**: DataGrip's "N of M schemas" choice.
    ///
    /// Display only, never a filter the engine applies — registration exposes every schema the role
    /// can see (the providers are lazy, so that costs nothing), and a query naming a schema that is
    /// not enabled still resolves and runs. This scopes the data-sources tree and completion: "what
    /// am I working with", not "what may I read". Defaults to `public`.
    pub schemas: Vec<String>,
    /// Whether Strata refuses to **change** this database: `INSERT` into one of its relations, and
    /// `CREATE TABLE … AS SELECT` making one (DB-10).
    ///
    /// **Default `true`**, which is what makes shipping writes change nothing: a stored def that
    /// predates the field deserializes read-only, and so does a connection nobody has opted in.
    /// The gate is the def rather than a machine-local preference because a connection is
    /// committed and shared — a colleague pulling the project gets the same answer about the same
    /// server.
    pub read_only: bool,
}

/// Hand-written rather than derived, for `schemas` alone: `#[serde(default)]` reads this, so a
/// derived `Vec::new()` would make a fresh `PgStore` and a stored one that omits the field disagree
/// about what a connection shows.
impl Default for PgStore {
    fn default() -> Self {
        Self {
            catalog: String::new(),
            user: String::new(),
            sslmode: PgSslMode::default(),
            sslrootcert: String::new(),
            password: PgPassword::default(),
            schemas: vec!["public".to_string()],
            read_only: true,
        }
    }
}

impl PgStore {
    /// Whether [`catalog`](Self::catalog) is a name this connection may register under, on its own
    /// terms — the half of [`check_catalog_name`] that needs no other connection.
    ///
    /// A **bare** SQL identifier, narrower than what DataFusion could resolve, because every
    /// surface that renders `pg.public.orders` would otherwise have to quote it. Case-folded
    /// against the reserved name, because unquoted identifiers are.
    pub fn check_catalog(&self) -> Result<(), String> {
        let name = self.catalog.trim();
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

    /// Whether [`user`](Self::user) is a role this connection can actually log in as — the
    /// address's rule, for the other half of the identity.
    ///
    /// **Refused by name, because the layer below refuses it namelessly.** A role holding a space
    /// or an `=` produces `user=read only dbname=…`, which the parser rejects in words naming
    /// neither the field nor the value. It is also half of [`ConnectionDef::url`], the input the
    /// keystore slot derives from.
    pub fn check_user(&self) -> Result<(), String> {
        let user = self.user.trim();
        if user.is_empty() {
            return Err("This connection has no user.".into());
        }
        if user.contains('@') {
            return Err("A PostgreSQL user can't contain '@'.".into());
        }
        check_conn_value("user", user)
    }
}

/// The catalog the project's own tables, views and results live in — what a database connection's
/// catalog name may not be ([`PgStore::check_catalog`]). Here rather than in the engine that
/// registers it, because both crates need it: `strata_engine::CATALOG` reads it.
pub const WORKSPACE_CATALOG: &str = "strata";

/// How the connection to the server is encrypted — libpq's own vocabulary in libpq's own spellings,
/// because the value is handed to the driver as written.
///
/// [`Prefer`](Self::Prefer) is the default for the reason it is libpq's. The two verifying modes are
/// the provider crate's emulation over `tokio-postgres` and read [`PgStore::sslrootcert`].
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PgSslMode {
    Disable,
    #[default]
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

impl PgSslMode {
    /// Every mode, in the order a picker offers them — weakest first, libpq's own ordering, which
    /// reads as a dial rather than a list.
    pub const ALL: [PgSslMode; 5] = [
        Self::Disable,
        Self::Prefer,
        Self::Require,
        Self::VerifyCa,
        Self::VerifyFull,
    ];

    /// The driver's own word for this mode — what goes on the wire, and what the label shows.
    /// One string, because a label that is not the parameter is a second thing to keep true.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disable => "disable",
            Self::Prefer => "prefer",
            Self::Require => "require",
            Self::VerifyCa => "verify-ca",
            Self::VerifyFull => "verify-full",
        }
    }

    /// Whether this mode reads [`PgStore::sslrootcert`] — the two verifying ones.
    pub fn verifies(self) -> bool {
        matches!(self, Self::VerifyCa | Self::VerifyFull)
    }
}

/// Whether this connection expects a password, and nothing more.
///
/// **The expectation, never a reference.** The keystore slot is *derived* from the connection's
/// identity (`SecretRef::derived("pg-password", def.url())`), so it is the same slot on every
/// machine while each machine's keystore holds its own entry, and the committed def gains no
/// machine-local id for two colleagues to ping-pong through git.
///
/// On a machine with no entry the connection settles failed, naming the fix, exactly as an expired
/// SSO session does — and entering the password touches nothing in the project file.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum PgPassword {
    /// No password: `trust`, `peer` or certificate authentication.
    #[default]
    None,
    /// This machine's OS keystore holds one.
    Keystore,
}

#[cfg(test)]
mod tests {
    use std::slice;

    use super::*;

    fn parse(json: &str) -> ConnectionDef {
        serde_json::from_str(json).expect("a connection def")
    }

    /// **An object store's scheme is its provider's; an HTTP connection's address is already the
    /// URL.**
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
        let pg = |user: &str| ConnectionDef {
            address: "db.internal:5432/analytics".into(),
            provider: Provider::Postgres(PgStore {
                catalog: "pg".into(),
                user: user.into(),
                ..Default::default()
            }),
            client_config: Default::default(),
        };
        assert_eq!(
            pg("reader").url(),
            "postgres://reader@db.internal:5432/analytics"
        );
        assert_ne!(pg("reader").url(), pg("writer").url());
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
            (
                Provider::Postgres(PgStore::default()),
                ProviderId::Postgres,
                "PG",
            ),
        ] {
            assert_eq!(provider.to_string(), name);
            assert_eq!(provider.id(), id);
            assert_eq!(id.label(), name);
        }
        assert_eq!(ProviderId::ALL.len(), 4, "every provider there is");
        assert_eq!(
            ProviderId::OBJECT_STORES.to_vec(),
            ProviderId::ALL
                .into_iter()
                .filter(|id| id.is_object_store())
                .collect::<Vec<_>>()
        );
        assert!(!ProviderId::Postgres.is_object_store());
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

    fn pg_store() -> PgStore {
        PgStore {
            catalog: "pg".into(),
            user: "reader".into(),
            ..Default::default()
        }
    }

    fn pg() -> Provider {
        Provider::Postgres(pg_store())
    }

    /// **A database address is `host:port/database`**, and every part is required: the port because
    /// a Postgres off 5432 is the ordinary case for a container or a pooler, the database because
    /// there is no server-wide connection to make.
    #[test]
    fn a_postgres_address_is_a_server_and_a_database() {
        for good in [
            "db.internal:5432/analytics",
            "localhost:5432/postgres",
            "127.0.0.1:65535/a",
            "::1:5432/analytics",
            "[::1]:5432/analytics",
        ] {
            assert_eq!(pg().check_address(good), Ok(()), "{good}");
        }
        for (bad, why) in [
            ("", "no server"),
            ("db.internal:5432 /analytics", "spaces"),
            ("postgres://db:5432/analytics", "://"),
            ("reader@db:5432/analytics", "user or password"),
            ("db.internal:5432", "needs a database"),
            ("db.internal:5432/", "needs a database"),
            ("db.internal/analytics", "needs a port"),
            (":5432/analytics", "needs a host"),
            ("db.internal:0/analytics", "not a port number"),
            ("db.internal:pg/analytics", "not a port number"),
            ("db.internal:99999/analytics", "not a port number"),
            ("db.internal:5432/a/b", "one '/'"),
        ] {
            let message = pg().check_address(bad).expect_err(bad);
            assert!(message.to_lowercase().contains(why), "{bad}: {message}");
        }
    }

    /// **A role is checked on the same terms as an address**: a space or an `=` in the user fails
    /// not as "that user is wrong" but as a connection string the parser cannot read.
    #[test]
    fn a_postgres_user_is_one_the_connection_string_can_carry() {
        for good in ["reader", "app_user", "analytics-ro", "READER"] {
            assert_eq!(
                PgStore {
                    user: good.into(),
                    ..pg_store()
                }
                .check_user(),
                Ok(()),
                "{good}"
            );
        }
        for (bad, why) in [
            ("", "no user"),
            ("   ", "no user"),
            ("read only", "spaces"),
            ("user=x", "'='"),
            ("o'brien", "'''"),
            ("dom\\user", "'\\'"),
            ("reader@db", "'@'"),
        ] {
            let message = PgStore {
                user: bad.into(),
                ..pg_store()
            }
            .check_user()
            .expect_err(bad);
            assert!(message.to_lowercase().contains(why), "{bad}: {message}");
        }
    }

    /// **A catalog name is what queries type**, so it is a bare SQL identifier, it is not the
    /// workspace's own, and it is not another connection's — one answer for the engine's
    /// registration and the editor's blocker alike.
    #[test]
    fn a_catalog_name_is_a_free_sql_identifier() {
        let with = |catalog: &str, user: &str, address: &str| ConnectionDef {
            address: address.into(),
            provider: Provider::Postgres(PgStore {
                catalog: catalog.into(),
                user: user.into(),
                ..Default::default()
            }),
            client_config: Default::default(),
        };
        let warehouse = with("warehouse", "reader", "a:5432/x");

        assert_eq!(
            check_catalog_name(&[], &with("pg", "reader", "b:5432/y")),
            Ok(())
        );
        for (catalog, why) in [
            ("", "no catalog name"),
            ("2pg", "starts with a letter"),
            ("my catalog", "starts with a letter"),
            ("pg-main", "starts with a letter"),
            ("análisis", "starts with a letter"),
            (WORKSPACE_CATALOG, "this project's own catalog"),
            ("STRATA", "this project's own catalog"),
        ] {
            let message =
                check_catalog_name(&[], &with(catalog, "reader", "b:5432/y")).expect_err(catalog);
            assert!(message.contains(why), "{catalog}: {message}");
        }

        let message = check_catalog_name(
            slice::from_ref(&warehouse),
            &with("WAREHOUSE", "reader", "b:5432/y"),
        )
        .expect_err("taken");
        assert!(message.contains("postgres://reader@a:5432/x"), "{message}");
        assert_eq!(
            check_catalog_name(slice::from_ref(&warehouse), &warehouse),
            Ok(())
        );
        assert_eq!(
            check_catalog_name(
                &[warehouse],
                &ConnectionDef {
                    address: "acme-lake".into(),
                    provider: s3(),
                    client_config: Default::default(),
                }
            ),
            Ok(())
        );
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
            ConnectionDef {
                address: "db.internal:5432/analytics".into(),
                provider: Provider::Postgres(PgStore {
                    catalog: "warehouse".into(),
                    user: "reader".into(),
                    sslmode: PgSslMode::VerifyFull,
                    sslrootcert: "/certs/rds.pem".into(),
                    password: PgPassword::Keystore,
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
        let json = serde_json::to_string(&ConnectionDef {
            address: "db.internal:5432/analytics".into(),
            provider: Provider::Postgres(PgStore {
                catalog: "warehouse".into(),
                user: "reader".into(),
                password: PgPassword::Keystore,
                ..Default::default()
            }),
            client_config: Default::default(),
        })
        .expect("serialize");
        assert_eq!(
            json,
            r#"{"address":"db.internal:5432/analytics","provider":{"provider":"postgres","catalog":"warehouse","user":"reader","sslmode":"prefer","sslrootcert":"","password":"keystore","schemas":["public"],"read_only":true}}"#
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
            old.url(),
            "https://example.com:8080",
            "the URL the old code composed"
        );
        assert_eq!(old.provider.check_address(&old.address), Ok(()));

        for written in ["http://aserver:8484", "https://aserver:8484"] {
            let def = ConnectionDef {
                address: written.into(),
                provider: Provider::Http,
                client_config: Default::default(),
            };
            assert_eq!(def.clone().migrated(), def, "{written}");
        }
        let bucket = ConnectionDef {
            address: "acme-lake".into(),
            provider: Provider::S3(S3Store::default()),
            client_config: Default::default(),
        };
        assert_eq!(bucket.clone().migrated(), bucket);
    }

    /// A provider's settings are all `#[serde(default)]`, so a def written before a setting existed
    /// still loads. The **address** carries that rule too: it was called `bucket` until HTTP
    /// started holding a whole URL, so every def below is written the old way.
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
        assert_eq!(
            parse(
                r#"{"address":"db:5432/analytics","provider":{"provider":"postgres","catalog":"pg","user":"reader"}}"#
            )
            .provider,
            Provider::Postgres(PgStore {
                catalog: "pg".into(),
                user: "reader".into(),
                sslmode: PgSslMode::Prefer,
                sslrootcert: String::new(),
                password: PgPassword::None,
                schemas: vec!["public".into()],
                read_only: true,
            }),
            "a def that predates a field reads the Default's answer for it, and read-only is the \
             one 'read_only' has to give"
        );
    }

    /// The SSL modes are libpq's, in libpq's spellings: the provider crate matches these strings
    /// literally, so a rename here is a connection that fails with 'Invalid parameter: sslmode'.
    #[test]
    fn ssl_modes_are_the_drivers_own_words() {
        for mode in PgSslMode::ALL {
            assert_eq!(
                serde_json::to_string(&mode).expect("serialize"),
                format!("\"{}\"", mode.as_str())
            );
        }
        assert_eq!(
            PgSslMode::default(),
            PgSslMode::Prefer,
            "libpq's own default"
        );
        assert_eq!(
            PgSslMode::ALL
                .into_iter()
                .filter(|m| m.verifies())
                .collect::<Vec<_>>(),
            vec![PgSslMode::VerifyCa, PgSslMode::VerifyFull]
        );
    }
}
