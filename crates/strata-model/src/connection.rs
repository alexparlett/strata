//! **Connections** — the persisted description of one remote source a project reads from: an
//! object store (W7) or a database (DB workstream). Exactly what `.strata/project.json`
//! stores, like the catalog defs beside it. Spec: `docs/CONNECTIONS_SPEC.md`.
//!
//! The rule the whole feature is built around: **no arm of this module holds a secret
//! value.** A connection carries only non-secret metadata — a bucket, a region, an endpoint,
//! a host and a role name — plus, where credentials are needed, a *reference* to where they
//! live: a named `~/.aws` profile, a service-account key **file path**, or the bare
//! expectation that this machine's OS keystore holds a password
//! ([`PgPassword::Keystore`]). Nothing here is a key, so nothing here has to be gitignored:
//! the whole def is shareable, which is why it rides the committed `project.json` rather
//! than the local `session.json`.
//!
//! **That is a rewrite, not a relaxation** (settled 2026-08-13). The original rule was
//! "Strata never stores, prompts for, or reads a secret", and it was a consequence of the OS
//! keystore not existing when W7 was built rather than a standing prohibition — object
//! stores happen to have host-side credential chains, and databases do not. A Postgres
//! password is captured exactly as an assistant provider key is (`strata_core::secret`) and
//! read **per use** inside the connection pool; what changes here is one enum arm saying
//! *there is one*, and even that is not a `SecretRef`: the keystore slot is **derived** from
//! the connection's own identity (`SecretRef::derived("pg-password", def.url())`), so the
//! committed def carries no machine-local id and two colleagues' keystores never fight over
//! it through git. Storing a derivable value beside the fields it derives from would be two
//! statements of one fact that can disagree.
//!
//! There is still no arm carrying a secret *value*, anywhere in this module. That is the
//! enforcement: an access-key or password field cannot be added without adding a variant
//! that says so out loud.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// One project-scoped connection: the address it names, and the provider that serves it.
///
/// **Identity is [`url`](Self::url), not the bucket** — scheme *and* authority, which is
/// exactly what DataFusion's object-store registry keys on (see `strata_core::engine::store`).
/// The distinction is not academic: `s3://lake` and `gs://lake` share a bucket and are two
/// different connections over two different stores, so anything addressing one of them —
/// a registration outcome, a store row, a Configure dropdown — has to say which.
///
/// **[`address`](Self::address) is not a bucket name**, which is why it is not called one: an
/// object store is addressed by a bucket, whose scheme its provider states (`acme-lake` under
/// S3 is `s3://acme-lake`), an HTTP origin is addressed by the URL itself, scheme included,
/// and a database is addressed by `host:port/database`. One field either way, because a
/// connection has exactly one address; what a *provider* makes of it is the provider's
/// business.
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
    /// - **Postgres** — `host:port/database`, one typed string on HTTP's precedent: the
    ///   server's own spelling of what you dial. The `postgres` scheme is the provider's, and
    ///   the *role* is [`PgStore::user`] rather than userinfo in here, because it is settings
    ///   the form asks for separately.
    ///
    /// Never a path on an object store: the object-store registry keys on scheme and
    /// authority ([`url`](Self::url)), so a path there would register under a key nothing
    /// looks up. What reads a path is the table's own source. (A database's address ends in
    /// `/database`, which is not a path *inside* the source but half of naming the source —
    /// there is no `postgres://host:5432` to connect to.)
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
    /// **Empty on a database connection, and not offered by its editor**: it is
    /// `object_store`'s HTTP-client vocabulary, and a Postgres connection speaks no HTTP.
    ///
    /// Absent when empty, so a project file gains nothing until a connection sets one, and a def
    /// written before the field existed still loads.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub client_config: BTreeMap<String, String>,
}

impl ConnectionDef {
    /// The URL this connection is identified by — `s3://acme-lake`, `gs://lake`,
    /// `http://aserver:8484`, `postgres://reader@db.internal:5432/analytics`.
    ///
    /// For an **object store** that is scheme + authority and nothing else, because that is the
    /// whole of what the object-store registry keys on. The two bucket providers compose it from
    /// their provider's scheme; HTTP's address **is** this URL, so there is nothing to compose.
    /// Which is also why there is no `Provider::scheme`: not every arm could answer.
    ///
    /// A **database** connection registers a catalog rather than an object store, so nothing
    /// parses this back into a scheme and an authority — it is the project's identity for the
    /// connection and the key its keystore entry derives from, and so it carries the two further
    /// things that make two connections different: the **database** (a path segment, which no
    /// object-store URL may have) and the **role**. Two roles over one database really are two
    /// connections, with two sets of visible schemas — and the provider crate's own join-pushdown
    /// context agrees, keying on host + port + db + user.
    pub fn url(&self) -> String {
        match &self.provider {
            Provider::Http => self.address.clone(),
            Provider::S3(_) => format!("s3://{}", self.address),
            Provider::Gcs(_) => format!("gs://{}", self.address),
            // **Trimmed, both halves.** This URL is the project's identity for the connection,
            // the `Reg` row's key, the engine's membership string *and* the input its keystore
            // slot derives from — while everything that dials trims. Untrimmed they disagree:
            // a def carrying `" reader "` logs in as `reader` but files its password under a
            // slot with spaces in it, which the next edit that tidies the field cannot find.
            Provider::Postgres(pg) => {
                format!("postgres://{}@{}", pg.user.trim(), self.address.trim())
            }
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
/// Three object stores, and deliberately no fourth: Azure was dropped in the spec's v11.
/// S3-compatible stores (Cloudflare R2, MinIO, Alibaba OSS, Tencent COS) ride [`S3`](Self::S3)
/// via its [`endpoint`](S3Store::endpoint) rather than each becoming a provider of its own.
///
/// **[`Postgres`](Self::Postgres) is a fourth arm rather than a second kind of thing** (DB
/// workstream). It registers a DataFusion *catalog* where the others register an object store,
/// and that difference lives entirely in `strata_core::engine` — everything here is the same
/// def, the same `Reg` row, the same editor window, the same registration pass and the same
/// Forget confirm, which is the lesson `TableOrigin` settled. `MySQL` or `SQLite` later would be
/// further arms over that same mechanism; we do not build a generic RDBMS abstraction ahead of
/// a second database, because the per-arm `match` *is* the mechanism.
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

    /// Whether `address` is one this provider will actually accept — **checked here, so the
    /// engine and the connection editor cannot disagree about it**.
    ///
    /// Four different questions, because the providers address different things: S3 has no
    /// underscores where GCS does, GCS reserves `goog` and `google` where S3 does not, HTTP is
    /// not asking about a bucket name at all but about a URL, and Postgres is asking about a
    /// server and a database. A form that kept its own copy of any of that would drift from the
    /// store's the first time either changed, so both call this.
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
            Self::Postgres(_) => check_pg_address(address),
        }
    }
}

/// **The catalog name a database connection registers under** — checked against the project's
/// other connections, so the engine's registration and the editor's blocker cannot disagree
/// about which names are free.
///
/// `existing` is the connections to fold `candidate` against, `candidate` excluded — the
/// project's stored defs for the editor, and the databases already registered on the session
/// for `strata_core::engine::db::connect`. The two ask different sets on purpose: what is
/// *stored* is what the editor can warn about before anything is dialled, and what is
/// *registered* is what would actually collide. A connection that failed to connect reserves
/// nothing, which is why the engine's set is the live one.
///
/// A no-op for every provider that registers an object store: they are keyed by URL and have no
/// catalog name to clash over.
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
    Postgres,
}

impl ProviderId {
    /// The providers a picker offers, in the order it offers them (spec §1).
    ///
    /// **Pinned by test rather than by the compiler.** This is a fixed-length const and a loop
    /// over it takes whatever is in it, so a new arm ships into every picker silently — which
    /// is right for a picker of *providers* and wrong wherever the question is narrower. That
    /// narrower question is [`OBJECT_STORES`](Self::OBJECT_STORES).
    pub const ALL: [ProviderId; 4] = [Self::S3, Self::Gcs, Self::Http, Self::Postgres];

    /// The providers that register an **object store**, in the same order — what a surface
    /// offers when the question is "which connection do these *files* read through".
    ///
    /// A separate list rather than a filter written at each site, because getting it wrong is
    /// silent: the Configure window's LOCATION **TYPE** pill would offer "read these parquet
    /// files through my Postgres connection", which is not a thing, and the CONNECTION picker
    /// under it would then be empty with nothing saying why.
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

/// A database connection's address is **`host:port/database`** — the server's own spelling of
/// what you dial, in one box, on HTTP's precedent.
///
/// The port is not optional and not defaulted to 5432. A Postgres that is not on 5432 is the
/// ordinary case for a container, a tunnel or a pooler, and a def whose address reads
/// `db.internal/analytics` while it means `:5432` is a def that shows one thing and connects to
/// another — the same argument that keeps S3's region out of `object_store`'s silent default.
///
/// Userinfo is refused for the reason the HTTP arm refuses it: every word of this def rides in
/// the committed `project.json`. The role is [`PgStore::user`], asked for by its own field, and
/// the password is in the OS keystore.
fn check_pg_address(address: &str) -> Result<(), String> {
    parse_pg_address(address).map(|_| ())
}

/// A database connection's address, taken apart — the **one** parse of `host:port/database`.
///
/// `strata_core::engine::db` dials with exactly these parts rather than splitting the string a
/// second time. Two parses of one grammar drift the first time the shape moves (the IPv6 rule
/// below is precisely the kind of thing that lands in one copy and not the other), and a second
/// copy's refusals are unreachable prose that reads like live validation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PgAddress<'a> {
    /// Unbracketed: `[::1]` arrives here as `::1`, which is what a driver's `host=` takes.
    pub host: &'a str,
    pub port: u16,
    pub database: &'a str,
}

/// Read `address` as `host:port/database`, or say what is wrong with it in the field's own terms.
///
/// See [`check_pg_address`] for why each rule is here. The **connection-string** rules
/// ([`check_conn_value`]) apply to the host and the database for the same reason they apply to
/// [`PgStore::user`]: all three are interpolated into a libpq string with no quoting, so a value
/// the string cannot carry has to be refused by name rather than mangled or handed to the
/// driver's parser.
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
    // **An IPv6 literal loses its brackets here**, and here only. `[::1]:5432` is how every other
    // tool prints one and so is what gets pasted into the box, but a driver's `host=` takes the
    // address itself and would read the brackets as part of a hostname. The port is the last `:`
    // either way, which is what makes both spellings readable.
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
/// **Refused by name, because the layer below refuses it namelessly, or worse does not refuse it
/// at all.** The driver's parameters are assembled by plain interpolation
/// (`dbname={db} user={user} `), and its parser then reads `\` as an escape and `'` as a quote:
/// a database legitimately named `sales\2024` becomes `dbname=sales\2024`, which parses as
/// `sales2024` — so without this the app connects to, enumerates and federates a **different
/// database** with nothing anywhere saying so. `=` and whitespace end the value early in the
/// same way. Postgres will happily create all of these; they simply cannot be dialled through
/// this stack, and saying which is the honest answer.
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

/// How a `PostgreSQL` database is reached, and how it is **addressed in SQL**.
///
/// The [catalog name](Self::catalog) is the first field on any provider that is an SQL
/// identifier, and it exists because SQL cannot address `postgres://host:5432/analytics`: a
/// database connection registers a DataFusion catalog, and its relations have to be reachable
/// as `pg.public.orders`. It is the user's choice rather than derived from the database name,
/// because two connections to two servers' `analytics` databases would derive the same name.
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
    /// A root-certificate **file path**, for the two verifying modes. The path, never the
    /// certificate — the [`GcsAuth::ServiceAccount`] rule, and a certificate is not a secret
    /// anyway; what makes this a path is that the file is the user's own.
    pub sslrootcert: String,
    /// Whether this connection expects a password in this machine's OS keystore. The
    /// **expectation**, never a reference — see [`PgPassword`].
    pub password: PgPassword,
    /// The schemas this connection **shows**: DataGrip's "N of M schemas" choice, committed
    /// configuration like everything else here.
    ///
    /// Display only, and deliberately not a filter the engine applies. Registration exposes
    /// every schema the role can see — the providers are lazy, so that costs nothing — which
    /// means a query naming a schema that is not enabled still resolves and runs. What this
    /// scopes is the data-sources tree and completion: the answer to "what am I working with",
    /// not to "what may I read".
    ///
    /// Defaults to `public`, which is the schema a Postgres database has.
    pub schemas: Vec<String>,
}

/// Hand-written rather than derived, for `schemas` alone: a def written before the field
/// existed must land on `["public"]` and not on an empty list, and `#[serde(default)]` reads
/// this — so a derived `Vec::new()` would have made a fresh `PgStore` and a stored one that
/// omits the field disagree about what a connection shows.
impl Default for PgStore {
    fn default() -> Self {
        Self {
            catalog: String::new(),
            user: String::new(),
            sslmode: PgSslMode::default(),
            sslrootcert: String::new(),
            password: PgPassword::default(),
            schemas: vec!["public".to_string()],
        }
    }
}

impl PgStore {
    /// Whether [`catalog`](Self::catalog) is a name this connection may register under, on its
    /// own terms — the half of [`check_catalog_name`] that needs no other connection, so the
    /// engine can ask it with nothing but the def in hand.
    ///
    /// A **bare** SQL identifier: a leading letter or underscore, then letters, digits and
    /// underscores, ASCII only. Narrower than what DataFusion could resolve — a quoted name
    /// may hold anything — because every surface that renders `pg.public.orders` would have to
    /// quote it, and a catalog is typed far more often than it is chosen.
    ///
    /// Case-folded against the reserved name, because unquoted identifiers are: `STRATA` and
    /// `strata` are one catalog, and the workspace's is not up for grabs.
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
    /// **Refused by name, because the layer below refuses it namelessly.** The driver's
    /// parameters are assembled into a libpq connection string by plain interpolation, with no
    /// quoting: a role holding a space or an `=` produces `user=read only dbname=…`, which the
    /// parser reads as an unknown key and rejects in words that name neither the field nor the
    /// value. Postgres will happily `CREATE ROLE "read only"`, so the honest thing is to say
    /// that this is a role Strata cannot dial rather than to let it fail as a parse error — the
    /// plain-`http` endpoint precedent.
    ///
    /// It is also half of [`ConnectionDef::url`], which is the project's identity for the
    /// connection *and* the input its keystore slot derives from, so a value that cannot survive
    /// the connection string should not become one either.
    pub fn check_user(&self) -> Result<(), String> {
        let user = self.user.trim();
        if user.is_empty() {
            return Err("This connection has no user.".into());
        }
        // `@` on top of the shared rule: the user is half of [`ConnectionDef::url`], whose two
        // halves are separated by one, so a second would make the identity unsplittable.
        if user.contains('@') {
            return Err("A PostgreSQL user can't contain '@'.".into());
        }
        check_conn_value("user", user)
    }
}

/// The catalog the project's own tables, views and results live in — what a database
/// connection's catalog name may not be ([`PgStore::check_catalog`]).
///
/// Written here rather than in the engine that registers it, because both crates need it and a
/// name typed twice is a name that can disagree: `strata_core::engine::CATALOG` reads it.
pub const WORKSPACE_CATALOG: &str = "strata";

/// How the connection to the server is encrypted — libpq's own vocabulary, in libpq's own
/// spellings, because the value is handed to the driver as written.
///
/// [`Prefer`](Self::Prefer) is the default for the reason it is libpq's: a connection described
/// the way `psql` would describe it behaves the way `psql` does. The two verifying modes are the
/// provider crate's emulation over `tokio-postgres` (which itself knows only disable / prefer /
/// require) and read [`PgStore::sslrootcert`].
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
    /// Every mode, in the order a picker offers them — weakest first, which is libpq's own
    /// ordering and reads as a dial rather than a list.
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
/// identity (`strata_core::secret::SecretRef::derived("pg-password", def.url())`), so it is the
/// same slot on every machine while each machine's keystore holds its own entry — and the
/// committed def gains no machine-local id that two colleagues' "enter my password" would
/// ping-pong through git forever. Storing the derived ref here as well would be two statements
/// of one fact that can disagree the moment the identity moves.
///
/// The consequence is carried honestly: on a machine with no entry the connection settles
/// failed, naming the fix, exactly as an expired SSO session does — and entering the password
/// touches nothing in the project file.
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
        // A database's identity carries the **role** as well: two roles over one database are
        // two connections with two sets of visible schemas.
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
        // **`ALL` is the full set, and it is a fixed-length const, so this is what notices a
        // fifth arm.** It deliberately no longer claims anything about a *picker*: since the
        // database arm landed, both pickers offer `OBJECT_STORES` (a table reads files; the
        // connection editor has no database rows until DB-04), so an assertion about `ALL`
        // guarding a picker would be false. What DB-04 owes is moving `ProviderPicker` back to
        // `ALL`, and its own task file carries that as its first line.
        assert_eq!(ProviderId::ALL.len(), 4, "every provider there is");
        // …and the narrower list is exactly the arms that register an object store, asserted
        // from the other side too: a surface asking "which connection do these *files* read
        // through" must not be offered a database.
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

    /// **A database address is `host:port/database`**, and every part of it is required: the
    /// port because a Postgres off 5432 is the ordinary case for a container or a pooler, the
    /// database because there is no server-wide connection to make.
    ///
    /// Userinfo is the case worth pinning beside HTTP's: it is the shape that would put a
    /// password in the committed project file, and the message points at the field that
    /// actually holds the role.
    #[test]
    fn a_postgres_address_is_a_server_and_a_database() {
        for good in [
            "db.internal:5432/analytics",
            "localhost:5432/postgres",
            "127.0.0.1:65535/a",
            // An IPv6 literal, both spellings. The port is the **last** `:`, which is what makes
            // the bare form readable at all; the bracketed one is what every other tool prints,
            // and `engine::db` unwraps the brackets before the driver sees the host.
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

    /// **A role is checked on the same terms as an address**, and for a sharper reason: the
    /// driver's parameters are interpolated into a connection string with no quoting, so a
    /// space or an `=` in the user does not fail as "that user is wrong" but as a connection
    /// string the parser cannot read. It is also half of [`ConnectionDef::url`], so it is half
    /// of the connection's identity and of the keystore slot that identity derives.
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
            // `CREATE ROLE "read only"` is legal Postgres and simply cannot be dialled here.
            ("read only", "spaces"),
            ("user=x", "'='"),
            ("o'brien", "'''"),
            ("dom\\user", "'\\'"),
            // An `@` would put a second one in `postgres://user@host`, so the URL would no
            // longer split back into the halves every surface reads it as.
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
            // Unquoted identifiers fold, so the reserved name is reserved in every casing.
            ("STRATA", "this project's own catalog"),
        ] {
            let message =
                check_catalog_name(&[], &with(catalog, "reader", "b:5432/y")).expect_err(catalog);
            assert!(message.contains(why), "{catalog}: {message}");
        }

        // Against the project's other connections, folded and naming the one already holding
        // the name.
        let message = check_catalog_name(
            slice::from_ref(&warehouse),
            &with("WAREHOUSE", "reader", "b:5432/y"),
        )
        .expect_err("taken");
        assert!(message.contains("postgres://reader@a:5432/x"), "{message}");
        // …but a def compared against **itself** is not a collision: re-saving a connection
        // with nothing changed, and re-connecting one, both ask this.
        assert_eq!(
            check_catalog_name(slice::from_ref(&warehouse), &warehouse),
            Ok(())
        );
        // An object store has no catalog name to clash over, on either side.
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
                }),
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
        // **A database connection's persisted shape, and the thing that is not in it.** The
        // password field is the bare expectation; there is no id anywhere in this string,
        // because the keystore slot is derived from the connection's identity rather than
        // minted. A UUID appearing here would be a machine-local fact in a committed, shared
        // file — see [`PgPassword`].
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
            r#"{"address":"db.internal:5432/analytics","provider":{"provider":"postgres","catalog":"warehouse","user":"reader","sslmode":"prefer","sslrootcert":"","password":"keystore","schemas":["public"]}}"#
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
        // A database def stating only what it must: the schemas it shows land on `public`
        // rather than on nothing, which is the one field whose absent value is not the type's
        // zero — see [`PgStore`]'s hand-written `Default`.
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
            })
        );
    }

    /// The SSL modes are libpq's, in libpq's spellings, and the value on the wire is the value
    /// in the file — the provider crate matches these strings literally, so a rename here is a
    /// connection that fails with "Invalid parameter: sslmode".
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
