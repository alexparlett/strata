//! The connection editor's data: what is being edited ([`ConnectionTarget`]), what the user has
//! chosen ([`ConnectionDraft`]), and why a draft cannot be saved yet.
//!
//! **The draft keeps every provider's fields side by side; the def keeps only the chosen
//! provider's.** [`strata_model::Provider`] *is* its own settings — a region cannot be written
//! down on a GCS connection — but flipping the picker to GCS and back must not forget the region
//! you typed, so the draft holds all of them flat and [`ConnectionDraft::def`] projects the one
//! in play. The same split the Configure and Export windows make over `SourceFormat`, for the
//! same reason.
//!
//! The auth **modes** are split from their references for exactly that reason too:
//! `S3Auth::Profile { name }` carries the profile inside the variant, so a draft that stored an
//! `S3Auth` would lose the profile name every time the user glanced at Anonymous.
//!
//! **Switching provider cannot produce an invalid auth pair** — not because anything sanitises
//! it (spec §1 asks for that), but because `S3Auth` and `GcsAuth` are different types and the
//! draft keeps a mode per provider. There is no state to guard against.

use std::collections::{BTreeMap, BTreeSet};

use strata_engine::store::{check_client_config, client_key, ClientKey, CLIENT_KEYS};
use strata_model::{
    ConnectionDef, GcsAuth, GcsStore, PgPassword, PgStore, Provider, ProviderId, S3Auth, S3Store,
};

/// What this window is editing: a new connection, or an existing one by
/// [`url`](ConnectionDef::url).
///
/// The URL is the identity — scheme *and* authority, because `s3://lake` and `gs://lake` are two
/// connections over one bucket — and it is also what makes this window single-instance per
/// target: two windows on one def would both `upsert_connection` and both persist.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ConnectionTarget {
    New,
    Edit(String),
}

impl ConnectionTarget {
    /// The window's title.
    pub fn title(&self) -> &'static str {
        match self {
            Self::New => "New connection",
            Self::Edit(_) => "Edit connection",
        }
    }

    /// The line under the title: the connection this window opened on. A new one has nothing to
    /// say there yet — its URL is being typed.
    pub fn subtitle(&self) -> Option<&str> {
        self.editing()
    }

    /// The URL this window opened on, if any — what a moved identity is measured against.
    pub fn editing(&self) -> Option<&str> {
        match self {
            Self::New => None,
            Self::Edit(url) => Some(url),
        }
    }
}

/// Which S3 auth mode the pill is on, without the reference the mode carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum S3AuthId {
    Ambient,
    Profile,
    Anonymous,
}

impl S3AuthId {
    pub const ALL: [S3AuthId; 3] = [Self::Ambient, Self::Profile, Self::Anonymous];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ambient => "Ambient",
            Self::Profile => "Named profile",
            Self::Anonymous => "Anonymous",
        }
    }
}

/// Which GCS auth mode the pill is on. Separate from [`S3AuthId`] because the two providers'
/// modes are separate types in the def, and naming them one enum here would be the invalid pair
/// the model exists without.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GcsAuthId {
    Ambient,
    ServiceAccount,
    Anonymous,
}

impl GcsAuthId {
    pub const ALL: [GcsAuthId; 3] = [Self::Ambient, Self::ServiceAccount, Self::Anonymous];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ambient => "Ambient / ADC",
            Self::ServiceAccount => "Service-account file",
            Self::Anonymous => "Anonymous",
        }
    }
}

/// What this machine's keystore said about the entry the def expects. Read once at mount, and
/// only where the def expects one.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum PasswordProbe {
    #[default]
    Asking,
    Stored,
    Absent,
    /// The keystore refused to answer, in its own words — never folded into
    /// [`Absent`](Self::Absent), which would claim a fact nobody established.
    Refused(String),
}

/// What the PASSWORD row is showing: its sentence and both of its presses, off one value.
///
/// A password is optional (`trust`, `peer`, certificate), so absence is a state rather than a
/// mode and there is no pill; every arm is reachable by typing into the box or by one of the two
/// presses.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PasswordRow {
    /// Something is typed: it lands in this machine's keystore at Save.
    Typed,
    /// No password expected. `forgetting` when this machine's entry goes in the same Save.
    Unused {
        forgetting: bool,
    },
    /// Expected, and this machine holds it.
    Stored,
    /// Expected, and this machine does not — a colleague opening a shared project.
    Missing,
    /// Expected, and this machine's entry is being removed at Save. Other machines keep theirs,
    /// which is the whole difference from [`Unused`](Self::Unused).
    Removing,
    Asking,
    Refused(String),
}

impl PasswordRow {
    /// What the def expects with the box empty, whether anything is typed, whether a removal is
    /// pending, and what the keystore said.
    pub fn of(expected: PgPassword, typed: bool, removed: bool, probe: &PasswordProbe) -> Self {
        if typed {
            return Self::Typed;
        }
        match expected {
            PgPassword::None => Self::Unused {
                forgetting: removed,
            },
            PgPassword::Keystore if removed => Self::Removing,
            PgPassword::Keystore => match probe {
                PasswordProbe::Asking => Self::Asking,
                PasswordProbe::Stored => Self::Stored,
                PasswordProbe::Absent => Self::Missing,
                PasswordProbe::Refused(why) => Self::Refused(why.clone()),
            },
        }
    }

    /// The line under the box, each arm about **this machine** — the half a committed def cannot
    /// state. A marker echoing `PgPassword::Keystore` would read "a password is stored" on a
    /// machine that has never held one.
    pub fn note(&self) -> String {
        match self {
            Self::Typed => "This password goes into this machine's keystore when you save.".into(),
            Self::Unused { forgetting: false } => {
                "This connection signs in without a password.".into()
            }
            Self::Unused { forgetting: true } => "This connection signs in without a password. \
                 The one stored on this machine is removed when you save."
                .into(),
            Self::Stored => {
                "A password is stored on this machine. Type a new one to replace it.".into()
            }
            Self::Missing => "This connection expects a password and none is stored on this \
                 machine. Enter it here."
                .into(),
            Self::Removing => "The password stored on this machine is removed when you save. \
                 This connection still expects one, so other machines keep theirs."
                .into(),
            Self::Asking => "Checking this machine's keystore…".into(),
            Self::Refused(why) => why.clone(),
        }
    }

    /// Whether **Remove from this machine** is offered: there has to be an entry here to remove.
    pub fn offers_removal(&self) -> bool {
        matches!(self, Self::Stored)
    }

    /// Whether **This connection uses no password** is offered — wherever one is still expected,
    /// including while the keystore is asked or refusing, since the press edits the def rather
    /// than this machine.
    pub fn offers_disuse(&self) -> bool {
        !matches!(self, Self::Typed | Self::Unused { .. })
    }
}

/// Everything the user has chosen.
#[derive(Clone, PartialEq, Debug)]
pub struct ConnectionDraft {
    pub provider: ProviderId,
    /// Where the connection points, in its provider's own terms — a bucket name for S3 and GCS,
    /// a whole origin URL for HTTP. [`ConnectionDef::address`]'s value, and the box's, verbatim.
    pub address: String,
    pub region: String,
    pub s3_auth: S3AuthId,
    pub profile: String,
    pub endpoint: String,
    pub allow_http: bool,
    pub gcs_auth: GcsAuthId,
    pub sa_path: String,
    /// A database connection's settings, edited in place by the Postgres rows. Held whole where
    /// the S3 and GCS fields are held flat, because it carries no mode-plus-reference pair for a
    /// trip through the picker to lose. [`password`](PgStore::password) is the one field no
    /// control writes directly — [`ConnectionCtx`](super::ConnectionCtx)'s slots derive it.
    ///
    /// Not cloned into the def: [`def`](Self::def) rebuilds it field by field with no `..`, so
    /// its text is trimmed like every other field here and a field added to `PgStore` has to be
    /// answered rather than silently carried untrimmed.
    pub pg: PgStore,
    /// The connection's client options, **as rows** — see [`ConfigRows`].
    pub client_config: ConfigRows,
}

impl Default for ConnectionDraft {
    /// A new connection: S3, nothing filled in.
    ///
    /// **The region starts blank**, though the canvas seeds `us-east-1`. That seed is the very
    /// failure the engine refuses a blank region to prevent: `AmazonS3Builder` silently defaults
    /// to `us-east-1` (arrow-rs#2795), which resolves to a real endpoint serving a different
    /// bucket's worth of nothing — and the credential probe still passes, so the connection
    /// registers green over the wrong region. A pre-filled guess is that default wearing a
    /// user's handwriting. Blank blocks Save and says why; `us-east-1` remains the box's
    /// placeholder, which is the canvas's own hint and cannot be saved by accident.
    fn default() -> Self {
        Self {
            provider: ProviderId::S3,
            address: String::new(),
            region: String::new(),
            s3_auth: S3AuthId::Ambient,
            profile: String::new(),
            endpoint: String::new(),
            allow_http: false,
            gcs_auth: GcsAuthId::Ambient,
            sa_path: String::new(),
            pg: PgStore::default(),
            client_config: ConfigRows::default(),
        }
    }
}

/// One row of the client-options table: an option and its value, under an id that outlives both.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConfigRow {
    pub id: u64,
    pub key: String,
    pub value: String,
}

/// The client options **as the table edits them**, rather than as the def stores them.
///
/// A `BTreeMap` is the right shape to apply and the wrong shape to edit: it cannot hold the row
/// you just added and have not named, cannot hold two rows with the same option long enough for
/// you to fix one, and reorders itself under the cursor. So this is an ordered list of identified
/// rows, projected back into the map on every read ([`to_map`](Self::to_map)).
///
/// **Row identity is a counter, never the option name** — the name is the one thing the row
/// exists to let you change. The Settings window's engine-properties grid settled all of this;
/// this is deliberately *not* that type, which is welded to `ENGINE_KEYS`, a selection, an
/// autocomplete and an inspector pane. What is shared is the rule, not the code.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ConfigRows {
    rows: Vec<ConfigRow>,
    next_id: u64,
}

impl ConfigRows {
    /// Seed from a stored map — one row per entry, in the map's own sorted order.
    pub fn of(config: &BTreeMap<String, String>) -> Self {
        let mut list = Self::default();
        for (key, value) in config {
            list.add(key.clone(), value.clone());
        }
        list
    }

    pub fn rows(&self) -> &[ConfigRow] {
        &self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Project back into the map the store is configured from: keys trimmed, unnamed rows
    /// dropped, and a duplicated key resolved the way the table shows it — the last row wins.
    ///
    /// Total by design. A list carrying errors still projects; what stops it reaching the engine
    /// is [`blocker`](ConnectionDraft::blocker) refusing to save, not this returning something
    /// partial.
    pub fn to_map(&self) -> BTreeMap<String, String> {
        self.rows
            .iter()
            .filter(|row| !row.key.trim().is_empty())
            .map(|row| (row.key.trim().to_string(), row.value.trim().to_string()))
            .collect()
    }

    /// Append a row. Returns its id, so the toolbar can select what it just added.
    pub fn add(&mut self, key: String, value: String) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.rows.push(ConfigRow { id, key, value });
        id
    }

    /// Drop the row `id`, and hand back the row that takes its place — the one below it, or the
    /// new last row when it was last, or nothing when the list is now empty. The toolbar selects
    /// that, exactly as the source-path list does: a Remove that left the highlight on a row that
    /// no longer exists would arm the next press at nothing.
    pub fn remove(&mut self, id: u64) -> Option<u64> {
        let Some(at) = self.rows.iter().position(|row| row.id == id) else {
            return self.rows.last().map(|row| row.id);
        };
        self.rows.remove(at);
        self.rows
            .get(at)
            .or_else(|| self.rows.last())
            .map(|row| row.id)
    }

    pub fn set_key(&mut self, id: u64, key: String) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.id == id) {
            row.key = key;
        }
    }

    pub fn set_value(&mut self, id: u64, value: String) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.id == id) {
            row.value = value;
        }
    }

    /// What row `id` currently holds — what each box seeds from and compares against, so neither
    /// direction writes over a change the other made.
    pub fn name_of(&self, id: u64) -> Option<String> {
        self.rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.key.clone())
    }

    pub fn value_of(&self, id: u64) -> Option<String> {
        self.rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.value.clone())
    }

    /// What row `id`'s name box is worth offering — the properties grid's contract, key for key.
    ///
    /// Matches **anywhere** in the name rather than only at the front (`proxy` finds
    /// `proxy_ca_certificate`), hides what another row already claims (offering a name that would
    /// immediately read "set twice" is not an offer), and goes quiet on an exact hit, because
    /// suggesting back what is already typed is a panel that will not close. Capped, so a blank
    /// box shows a readable list rather than the whole catalogue.
    pub fn suggestions(&self, id: u64) -> Vec<&'static ClientKey> {
        let Some(row) = self.rows.iter().find(|row| row.id == id) else {
            return Vec::new();
        };
        let typed = row.key.trim().to_lowercase();
        let claimed: BTreeSet<&str> = self
            .rows
            .iter()
            .filter(|other| other.id != id)
            .map(|other| other.key.trim())
            .filter(|key| !key.is_empty())
            .collect();
        let matches: Vec<&'static ClientKey> = CLIENT_KEYS
            .iter()
            .filter(|entry| !claimed.contains(entry.name))
            .filter(|entry| typed.is_empty() || entry.name.contains(&typed))
            .collect();
        match matches.as_slice() {
            [only] if only.name == typed => Vec::new(),
            _ => matches,
        }
    }

    /// Whether row `id`'s name is one the store will take — what tints the box. A name that is not
    /// in the catalogue is an **error** here rather than the properties grid's warning: an engine
    /// key it has not heard of may simply be newer than this build, where `check_client_config`
    /// refuses an unknown client option outright.
    pub fn names_a_client_option(&self, id: u64) -> Option<bool> {
        let row = self.rows.iter().find(|row| row.id == id)?;
        let key = row.key.trim();
        (!key.is_empty()).then(|| client_key(key).is_some())
    }

    /// The first problem the *list* can have, which is the pair the map cannot represent: a value
    /// with no option to set it on, and one option set twice. Everything else about an option is
    /// `check_client_config`'s, on the projected map.
    pub fn blocker(&self) -> Option<String> {
        if self
            .rows
            .iter()
            .any(|row| row.key.trim().is_empty() && !row.value.trim().is_empty())
        {
            return Some("A client option row has a value but no option.".into());
        }
        let mut seen = Vec::new();
        for key in self.rows.iter().map(|row| row.key.trim()) {
            if key.is_empty() {
                continue;
            }
            if seen.contains(&key) {
                return Some(format!("The client option '{key}' is set twice."));
            }
            seen.push(key);
        }
        None
    }
}

impl ConnectionDraft {
    /// Seed the draft from an existing def — every field it holds, so the window opens showing
    /// what is really stored and Save with nothing touched writes back the def that was there.
    ///
    /// The providers it *isn't* keep their defaults: the def has nothing to say about them.
    pub fn of(def: &ConnectionDef) -> Self {
        let mut draft = Self {
            provider: def.provider.id(),
            address: def.address.clone(),
            client_config: ConfigRows::of(&def.client_config),
            ..Default::default()
        };
        match &def.provider {
            Provider::S3(s3) => {
                draft.region = s3.region.clone();
                draft.endpoint = s3.endpoint.clone();
                draft.allow_http = s3.allow_http;
                match &s3.auth {
                    S3Auth::Ambient => draft.s3_auth = S3AuthId::Ambient,
                    S3Auth::Anonymous => draft.s3_auth = S3AuthId::Anonymous,
                    S3Auth::Profile { name } => {
                        draft.s3_auth = S3AuthId::Profile;
                        draft.profile = name.clone();
                    }
                }
            }
            Provider::Gcs(gcs) => match &gcs.auth {
                GcsAuth::Ambient => draft.gcs_auth = GcsAuthId::Ambient,
                GcsAuth::Anonymous => draft.gcs_auth = GcsAuthId::Anonymous,
                GcsAuth::ServiceAccount { path } => {
                    draft.gcs_auth = GcsAuthId::ServiceAccount;
                    draft.sa_path = path.clone();
                }
            },
            Provider::Http => {}
            Provider::Postgres(pg) => draft.pg = pg.clone(),
        }
        draft
    }

    /// Type into the address box.
    ///
    /// **HTTP takes what it is given, whole.** Its address *is* the URL — scheme included, because
    /// `http://` and `https://` are two different origins and only the person typing knows which
    /// their server speaks — so there is nothing here to normalise and nothing to correct. What
    /// is not a legal origin is refused by [`Provider::check_address`] and named in the footer,
    /// never trimmed off behind the user's back.
    ///
    /// **Every other provider loses a scheme typed with the address**, because theirs is the
    /// picker's answer and `ConnectionDef::url` puts it back. Stripped on the way in rather than
    /// on the way out, the rule a length-capped field follows: a box showing `s3://acme-lake`
    /// over a def storing `acme-lake` shows one thing and means another. A pasted
    /// `postgres://db:5432/analytics` lands the same way, on the same rule.
    pub fn set_address(&mut self, typed: String) {
        self.address = match self.provider {
            ProviderId::Http => typed,
            _ => strip_scheme(&typed).to_string(),
        };
    }

    /// What the address box is called, and what it is called *in prose* — a bucket for the two
    /// object stores, a URL for HTTP. One pair, so the label and every sentence about it agree.
    /// The `host:port` half of a database address, and the database half — the two boxes the
    /// Postgres form splits [`address`](Self::address) into.
    ///
    /// A split of the one stored string rather than two draft fields, so `ConnectionDef::address`
    /// stays `host:port/database` and `parse_pg_address` remains the only parse of that grammar.
    /// Total where that parse is fallible: this is what the *boxes* show while the address is
    /// being typed and is still wrong, and `check_address` is what refuses it.
    pub fn pg_server(&self) -> &str {
        match self.address.split_once('/') {
            Some((server, _)) => server,
            None => &self.address,
        }
    }

    pub fn pg_database(&self) -> &str {
        self.address.split_once('/').map_or("", |(_, db)| db)
    }

    /// Type into the URL box. Loses a pasted scheme for [`set_address`]'s reason, and **stops at
    /// the first `/`**.
    ///
    /// Neither half may carry the separator, or the two boxes stop being a view of the address:
    /// each box's effect subscribes to its own buffer alone, so a `host:port/appdb` pasted here
    /// would write the database half while the DATABASE box — which never re-runs — went on
    /// showing nothing, and the next keystroke in it would drop what was pasted. Truncating is
    /// what makes that unrepresentable; the database is then simply not set, which the blocker
    /// says in its own words.
    pub fn set_pg_server(&mut self, typed: String) {
        let database = self.pg_database().to_string();
        let typed = strip_scheme(&typed);
        let server = typed.split_once('/').map_or(typed, |(server, _)| server);
        self.compose_pg(server, &database);
    }

    /// Type into the DATABASE box. A `/` is dropped for [`set_pg_server`]'s reason, and because
    /// one kept here would compose an address naming two databases.
    pub fn set_pg_database(&mut self, typed: String) {
        let server = self.pg_server().to_string();
        self.compose_pg(&server, &typed.replace('/', ""));
    }

    /// `host:port` + `database` back into the one address. The separator is written only when
    /// there is a database, so an untouched DATABASE box leaves the address as the server alone
    /// and the blocker asks for a database rather than complaining about a trailing `/`.
    fn compose_pg(&mut self, server: &str, database: &str) {
        self.address = match database.is_empty() {
            true => server.to_string(),
            false => format!("{server}/{database}"),
        };
    }

    pub fn address_label(&self) -> &'static str {
        match self.provider {
            ProviderId::Http | ProviderId::Postgres => "URL",
            _ => "BUCKET",
        }
    }

    pub fn address_noun(&self) -> &'static str {
        match self.provider {
            ProviderId::Http | ProviderId::Postgres => "URL",
            _ => "bucket",
        }
    }

    /// The def this draft describes.
    pub fn def(&self) -> ConnectionDef {
        ConnectionDef {
            address: self.address.trim().to_string(),
            client_config: self.client_config.to_map(),
            provider: match self.provider {
                ProviderId::S3 => Provider::S3(S3Store {
                    region: self.region.trim().to_string(),
                    auth: match self.s3_auth {
                        S3AuthId::Ambient => S3Auth::Ambient,
                        S3AuthId::Anonymous => S3Auth::Anonymous,
                        S3AuthId::Profile => S3Auth::Profile {
                            name: self.profile.trim().to_string(),
                        },
                    },
                    endpoint: self.endpoint.trim().to_string(),
                    allow_http: self.allow_http && !self.endpoint.trim().is_empty(),
                }),
                ProviderId::Gcs => Provider::Gcs(GcsStore {
                    auth: match self.gcs_auth {
                        GcsAuthId::Ambient => GcsAuth::Ambient,
                        GcsAuthId::Anonymous => GcsAuth::Anonymous,
                        GcsAuthId::ServiceAccount => GcsAuth::ServiceAccount {
                            path: self.sa_path.trim().to_string(),
                        },
                    },
                }),
                ProviderId::Http => Provider::Http,
                ProviderId::Postgres => Provider::Postgres(PgStore {
                    catalog: self.pg.catalog.trim().to_string(),
                    user: self.pg.user.trim().to_string(),
                    sslmode: self.pg.sslmode,
                    sslrootcert: self.pg.sslrootcert.trim().to_string(),
                    password: self.pg.password,
                    schemas: self.pg.schemas.clone(),
                }),
            },
        }
    }

    /// Why this draft cannot be saved yet, or `None` when it can.
    ///
    /// Only what the *draft* can answer — a URL another connection already holds is the store's
    /// question and lives in the footer beside this one, exactly as the Configure window's name
    /// clash does.
    ///
    /// The **address is checked by the def's own rules** ([`Provider::check_address`]) rather
    /// than by a copy kept here, so a name this form accepts is exactly a name
    /// `strata_engine::store::connect` accepts, in the same words. That is stronger than
    /// the two agreeing by inspection: S3's rules and GCS's differ in four places, and a form
    /// holding its own copy would drift from the store's the first time either moved.
    ///
    /// Everything else is this form's, because it is about a *control* rather than about the def:
    /// an auth mode with nothing chosen behind it is a half-answered question, not an invalid
    /// connection.
    pub fn blocker(&self) -> Option<String> {
        let def = self.def();
        if def.address.is_empty() {
            return Some(format!("A connection needs a {}.", self.address_noun()));
        }
        if let Err(why) = def.provider.check_address(&def.address) {
            return Some(why);
        }
        match self.provider {
            ProviderId::S3 => {
                if self.region.trim().is_empty() {
                    return Some(
                        "An S3 connection needs a region. It can't be auto-detected.".into(),
                    );
                }
                if self.s3_auth == S3AuthId::Profile && self.profile.trim().is_empty() {
                    return Some("Choose the AWS profile this connection signs with.".into());
                }
            }
            ProviderId::Gcs => {
                if self.gcs_auth == GcsAuthId::ServiceAccount && self.sa_path.trim().is_empty() {
                    return Some("Choose the service-account file this connection reads.".into());
                }
            }
            ProviderId::Http => {}
            ProviderId::Postgres => {
                if let Err(why) = self.pg.check_catalog() {
                    return Some(why);
                }
                if let Err(why) = self.pg.check_user() {
                    return Some(why);
                }
            }
        }
        if self.provider.is_object_store() {
            if let Some(why) = self.client_config.blocker() {
                return Some(why);
            }
            if let Err(why) = check_client_config(&def.client_config) {
                return Some(why);
            }
        }
        None
    }

    /// The standing note at the foot of the form: where this provider's credentials come from,
    /// and what Strata does *not* keep. The canvas's padlock paragraph.
    ///
    /// Per provider, because the answer is: HTTP has no credentials at all, and pointing a GCS
    /// user at their AWS profiles would be worse than saying nothing.
    pub fn note(&self) -> &'static str {
        match self.provider {
            ProviderId::S3 => {
                "Credentials resolve at query time from this machine's environment or the named \
                 AWS profile. Strata never stores a key: the project file keeps only the bucket, \
                 provider, region, endpoint and auth mode."
            }
            ProviderId::Gcs => {
                "Credentials resolve at query time from Application Default Credentials or the \
                 service-account file. Strata never stores a key, and never reads the file: the \
                 project file keeps only the bucket, provider, auth mode and that path."
            }
            ProviderId::Http => {
                "HTTP(S) sources are always read anonymously. There are no credentials and no \
                 region to configure."
            }
            ProviderId::Postgres => {
                "The password is kept in this machine's keystore and read per connection. The \
                 project file keeps only the server, database, user, catalog name and SSL mode, \
                 so a colleague opening this project enters their own password once."
            }
        }
    }
}

/// `s3://acme-lake` → `acme-lake`. Anything that is not a scheme is left alone, so a host with a
/// port (`example.com:8080`) survives — `://` is what makes it a scheme, not the colon.
fn strip_scheme(typed: &str) -> &str {
    let Some((scheme, rest)) = typed.split_once("://") else {
        return typed;
    };
    match !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        true => rest,
        false => typed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s3_draft() -> ConnectionDraft {
        ConnectionDraft {
            address: "acme-lake".into(),
            region: "eu-west-2".into(),
            ..Default::default()
        }
    }

    /// The round trip an edit is: open on a def, touch nothing, Save writes back what was there.
    /// Every provider, because the draft holds all three flat and only one of them is projected.
    #[test]
    fn a_def_survives_the_draft_untouched() {
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
                address: "https://example.com:8080".into(),
                provider: Provider::Http,
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
                    sslmode: strata_model::PgSslMode::VerifyFull,
                    sslrootcert: "/certs/rds.pem".into(),
                    password: PgPassword::Keystore,
                    schemas: vec!["public".into(), "analytics".into()],
                }),
                client_config: Default::default(),
            },
        ] {
            assert_eq!(ConnectionDraft::of(&def).def(), def, "{}", def.url());
        }
    }

    /// **A stored database connection opens as one.** `of` used to clamp a non-object-store def
    /// to S3, so a Postgres def opened as an S3 connection and a Save with nothing touched
    /// replaced it with one. The round trip above cannot catch that: it never looks at a provider.
    #[test]
    fn a_database_def_opens_on_the_database_arm() {
        let def = ConnectionDef {
            address: "db.internal:5432/analytics".into(),
            provider: Provider::Postgres(PgStore {
                catalog: "warehouse".into(),
                user: "reader".into(),
                ..Default::default()
            }),
            client_config: Default::default(),
        };
        let draft = ConnectionDraft::of(&def);
        assert_eq!(draft.provider, ProviderId::Postgres);
        assert_eq!(draft.address_label(), "URL");
        assert_eq!(draft.blocker(), None);
    }

    /// **A database's blockers are the model's own**, so a name this form accepts is one
    /// `engine::db::connect` accepts, in the same words. The project-wide catalog clash is the
    /// footer's, since it needs the other connections.
    #[test]
    fn a_database_draft_is_refused_on_the_engines_terms() {
        let good = ConnectionDraft {
            provider: ProviderId::Postgres,
            address: "db.internal:5432/analytics".into(),
            pg: PgStore {
                catalog: "warehouse".into(),
                user: "reader".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(good.blocker(), None);

        let mut portless = good.clone();
        portless.address = "db.internal/analytics".into();
        assert!(portless.blocker().unwrap().contains("needs a port"));

        let mut nameless = good.clone();
        nameless.pg.catalog = String::new();
        assert!(nameless.blocker().unwrap().contains("no catalog name"));

        let mut reserved = good.clone();
        reserved.pg.catalog = "STRATA".into();
        assert!(reserved
            .blocker()
            .unwrap()
            .contains("this project's own catalog"));

        let mut userless = good.clone();
        userless.pg.user = "  ".into();
        assert!(userless.blocker().unwrap().contains("no user"));

        let mut spaced = good.clone();
        spaced.pg.user = "read only".into();
        assert!(spaced.blocker().unwrap().contains("spaces"));

        let mut certless = good;
        certless.pg.sslmode = strata_model::PgSslMode::VerifyFull;
        assert_eq!(
            certless.blocker(),
            None,
            "a blank root certificate is the driver's own trust store, not a missing answer"
        );
    }

    /// **The database fields are trimmed into the def like every other field here.** `engine::db`
    /// trims at use, so an untrimmed name still registers as `pg` — while the committed,
    /// *shared* `project.json` would record `"pg "`, and the def is what every surface displays.
    #[test]
    fn a_database_defs_text_is_trimmed() {
        let draft = ConnectionDraft {
            provider: ProviderId::Postgres,
            address: "  db.internal:5432/analytics  ".into(),
            pg: PgStore {
                catalog: " warehouse ".into(),
                user: " reader ".into(),
                sslmode: strata_model::PgSslMode::VerifyFull,
                sslrootcert: " /certs/rds.pem ".into(),
                password: PgPassword::Keystore,
                schemas: vec!["public".into()],
            },
            ..Default::default()
        };
        let def = draft.def();
        assert_eq!(def.address, "db.internal:5432/analytics");
        let Provider::Postgres(pg) = &def.provider else {
            panic!("a database def");
        };
        assert_eq!(pg.catalog, "warehouse");
        assert_eq!(pg.user, "reader");
        assert_eq!(pg.sslrootcert, "/certs/rds.pem");
        assert_eq!(pg.password, PgPassword::Keystore, "carried, never trimmed");
        assert_eq!(def.url(), "postgres://reader@db.internal:5432/analytics");
    }

    /// **URL and DATABASE are two boxes over one stored address**, so the def keeps
    /// `host:port/database` and `parse_pg_address` stays the only parse of that grammar. Total in
    /// both directions while the address is half-typed, which is most of the time.
    #[test]
    fn the_two_database_boxes_split_and_recompose_one_address() {
        let mut draft = ConnectionDraft {
            provider: ProviderId::Postgres,
            pg: PgStore {
                catalog: "pg".into(),
                user: "reader".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        draft.set_pg_server("db.internal:5432".into());
        assert_eq!(
            draft.address, "db.internal:5432",
            "no database, no separator"
        );
        assert!(draft.blocker().unwrap().contains("needs a database"));

        draft.set_pg_database("analytics".into());
        assert_eq!(draft.address, "db.internal:5432/analytics");
        assert_eq!(draft.blocker(), None);

        assert_eq!(draft.pg_server(), "db.internal:5432");
        assert_eq!(draft.pg_database(), "analytics");

        draft.set_pg_server("postgres://other:5433".into());
        assert_eq!(
            draft.address, "other:5433/analytics",
            "a pasted scheme goes, and the database is untouched"
        );

        draft.set_pg_database("a/b".into());
        assert_eq!(
            draft.address, "other:5433/ab",
            "one database per connection, so a '/' typed here cannot compose a second"
        );

        draft.set_pg_database(String::new());
        assert_eq!(
            draft.address, "other:5433",
            "clearing it drops the separator"
        );

        draft.set_pg_database("analytics".into());
        draft.set_pg_server("host:5432/pasted".into());
        assert_eq!(
            draft.address, "host:5432/analytics",
            "a whole address pasted into the URL box stops at the '/': neither box may carry the \
             separator, or one of them shows something the address does not say"
        );
        assert_eq!(draft.pg_server(), "host:5432");
        assert_eq!(draft.pg_database(), "analytics");

        let stored = ConnectionDraft::of(&ConnectionDef {
            address: "db:5432/analytics".into(),
            provider: Provider::Postgres(PgStore::default()),
            client_config: Default::default(),
        });
        assert_eq!(
            stored.pg_server(),
            "db:5432",
            "and a stored def splits back"
        );
        assert_eq!(stored.pg_database(), "analytics");
    }

    /// **A pasted `postgres://` URL loses its scheme like every other non-HTTP address**: the
    /// picker states the scheme and `url()` puts it back.
    #[test]
    fn a_database_address_loses_a_pasted_scheme() {
        let mut draft = ConnectionDraft {
            provider: ProviderId::Postgres,
            pg: PgStore {
                catalog: "pg".into(),
                user: "reader".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        draft.set_address("postgres://db.internal:5432/analytics".into());
        assert_eq!(draft.address, "db.internal:5432/analytics");
        assert_eq!(
            draft.def().url(),
            "postgres://reader@db.internal:5432/analytics"
        );
    }

    /// **The PASSWORD row is about *this machine*, not about the def.** A committed def can only
    /// say a password is expected; conflating that with "one is stored" tells a colleague opening
    /// a shared project that theirs is already here. "Not asked yet" and "refused to say" are not
    /// "there is none" either.
    #[test]
    fn the_password_row_reports_this_machine_rather_than_the_def() {
        use PasswordProbe as P;

        let row =
            |expected, typed, removed, probe: &P| PasswordRow::of(expected, typed, removed, probe);

        assert_eq!(
            row(PgPassword::None, false, false, &P::Absent),
            PasswordRow::Unused { forgetting: false }
        );
        assert_eq!(
            row(PgPassword::Keystore, false, false, &P::Stored),
            PasswordRow::Stored
        );
        assert_eq!(
            row(PgPassword::Keystore, false, false, &P::Absent),
            PasswordRow::Missing,
            "expected, and this machine has none"
        );
        assert_eq!(
            row(PgPassword::Keystore, false, false, &P::Asking),
            PasswordRow::Asking
        );
        assert_eq!(
            row(
                PgPassword::Keystore,
                false,
                false,
                &P::Refused("locked".into())
            ),
            PasswordRow::Refused("locked".into())
        );

        assert_eq!(
            row(PgPassword::Keystore, true, false, &P::Stored),
            PasswordRow::Typed,
            "what is being typed outranks what is stored, in every state"
        );
        assert_eq!(
            row(PgPassword::None, true, true, &P::Absent),
            PasswordRow::Typed
        );
    }

    /// **The two clearing gestures are not the same gesture.** *Remove from this machine* leaves
    /// the expectation standing so a colleague keeps their own password; *this connection uses no
    /// password* edits the shared def.
    #[test]
    fn removing_a_password_locally_is_not_declaring_the_connection_has_none() {
        let removing = PasswordRow::of(PgPassword::Keystore, false, true, &PasswordProbe::Stored);
        assert_eq!(removing, PasswordRow::Removing);
        assert!(
            removing.note().contains("other machines keep theirs"),
            "{}",
            removing.note()
        );

        let unused = PasswordRow::of(PgPassword::None, false, true, &PasswordProbe::Stored);
        assert_eq!(unused, PasswordRow::Unused { forgetting: true });
        assert!(
            unused.note().contains("without a password"),
            "{}",
            unused.note()
        );
    }

    /// **Neither press is offered where it would mean nothing**, and neither is a dead end —
    /// typing a password gets back to expecting one from every arm.
    #[test]
    fn the_password_presses_are_offered_where_they_do_something() {
        assert!(PasswordRow::Stored.offers_removal());
        for row in [
            PasswordRow::Typed,
            PasswordRow::Missing,
            PasswordRow::Removing,
            PasswordRow::Asking,
            PasswordRow::Unused { forgetting: false },
        ] {
            assert!(!row.offers_removal(), "{row:?}: nothing here to remove");
        }

        for row in [
            PasswordRow::Stored,
            PasswordRow::Missing,
            PasswordRow::Removing,
            PasswordRow::Asking,
            PasswordRow::Refused("locked".into()),
        ] {
            assert!(row.offers_disuse(), "{row:?}: a password is still expected");
        }
        assert!(
            !PasswordRow::Unused { forgetting: false }.offers_disuse(),
            "already the answer"
        );
        assert!(
            !PasswordRow::Typed.offers_disuse(),
            "a box with a password in it is not the place to say there is none"
        );
    }

    /// **The HTTP box is one input holding a whole URL**, scheme and all: there is no scheme
    /// picker and no chip, because `http://` and `https://` are two different origins and only
    /// the person typing knows which their server speaks. What is typed is what is stored and
    /// what is registered.
    #[test]
    fn an_http_address_is_the_url_the_user_typed() {
        let mut draft = ConnectionDraft {
            provider: ProviderId::Http,
            ..Default::default()
        };
        for typed in ["http://aserver:8484", "https://aserver:8484"] {
            draft.set_address(typed.into());
            assert_eq!(draft.address, typed, "kept verbatim");
            assert_eq!(draft.def().url(), typed, "and registered as itself");
            assert_eq!(draft.blocker(), None, "{typed}");
        }

        draft.set_address("https://aserver:8484/fake".into());
        assert_eq!(
            draft.address, "https://aserver:8484/fake",
            "what was typed stays on screen while it is being corrected"
        );
        let why = draft.blocker().expect("a path is not an origin");
        assert!(
            why.contains("not a path") && why.contains("'/fake'"),
            "{why}"
        );

        draft.set_address("aserver:8484".into());
        let why = draft.blocker().expect("no scheme");
        assert!(why.contains("scheme"), "{why}");
    }

    /// **Flipping the provider and back forgets nothing.** The def cannot hold an S3 region on a
    /// GCS connection, so a draft that stored a `Provider` would drop the region the moment the
    /// picker moved — and silently, because a blank region is a state the form already has copy
    /// for.
    #[test]
    fn every_providers_fields_survive_the_picker() {
        let mut draft = ConnectionDraft {
            profile: "analytics".into(),
            s3_auth: S3AuthId::Profile,
            sa_path: "/keys/reader.json".into(),
            gcs_auth: GcsAuthId::ServiceAccount,
            ..s3_draft()
        };
        draft.provider = ProviderId::Gcs;
        assert_eq!(
            draft.def().provider,
            Provider::Gcs(GcsStore {
                auth: GcsAuth::ServiceAccount {
                    path: "/keys/reader.json".into()
                }
            })
        );
        draft.provider = ProviderId::S3;
        assert_eq!(draft.region, "eu-west-2");
        assert_eq!(
            draft.def().provider,
            Provider::S3(S3Store {
                region: "eu-west-2".into(),
                auth: S3Auth::Profile {
                    name: "analytics".into()
                },
                endpoint: String::new(),
                allow_http: false,
            })
        );
    }

    /// A pasted URL is a bucket with a scheme on it, and the S3 / GCS box must not keep the
    /// scheme — those two store the bucket alone and `url()` puts the provider's own scheme back.
    /// (HTTP is the opposite case, above: its address *is* the URL.)
    #[test]
    fn the_box_strips_a_pasted_scheme_and_keeps_a_port() {
        let mut draft = ConnectionDraft::default();
        for (typed, kept) in [
            ("s3://acme-lake", "acme-lake"),
            ("https://example.com", "example.com"),
            ("example.com:8080", "example.com:8080"),
            ("//acme-lake", "//acme-lake"),
        ] {
            draft.set_address(typed.into());
            assert_eq!(draft.address, kept, "{typed}");
        }
    }

    /// Every rule `engine::store::connect` refuses on is refused here first, in the field's own
    /// terms — and a draft with nothing wrong with it is saveable.
    #[test]
    fn the_blocker_names_what_the_engine_would_refuse() {
        assert_eq!(s3_draft().blocker(), None);

        let blank = ConnectionDraft::default();
        assert!(blank.blocker().unwrap().contains("bucket"));

        let mut pathy = s3_draft();
        pathy.address = "acme-lake/year=2024".into();
        assert!(pathy
            .blocker()
            .unwrap()
            .contains("lowercase letters, numbers, dots and hyphens"));

        let mut regionless = s3_draft();
        regionless.region = "  ".into();
        assert!(regionless.blocker().unwrap().contains("region"));

        let mut profileless = s3_draft();
        profileless.s3_auth = S3AuthId::Profile;
        assert!(profileless.blocker().unwrap().contains("AWS profile"));

        let mut keyless = ConnectionDraft {
            provider: ProviderId::Gcs,
            gcs_auth: GcsAuthId::ServiceAccount,
            ..s3_draft()
        };
        assert!(keyless.blocker().unwrap().contains("service-account"));
        keyless.sa_path = "/keys/reader.json".into();
        assert_eq!(keyless.blocker(), None, "GCS asks for no region");

        let http = ConnectionDraft {
            provider: ProviderId::Http,
            address: "https://example.com".into(),
            ..Default::default()
        };
        assert_eq!(http.blocker(), None);
        let mut urlless = http;
        urlless.address = String::new();
        assert!(urlless.blocker().unwrap().contains("URL"));
    }

    /// **Client options are edited as rows and committed as a map**, so the two states a map
    /// cannot hold are the list's own to refuse: a value with no option, and one option set
    /// twice. An unnamed *empty* row is neither — it is a row you have just added.
    #[test]
    fn client_options_are_rows_going_in_and_a_map_coming_out() {
        let mut draft = s3_draft();
        assert_eq!(draft.blocker(), None);

        let first = draft.client_config.add(String::new(), String::new());
        assert_eq!(draft.blocker(), None);
        assert!(draft.def().client_config.is_empty(), "and reaches no def");

        draft.client_config.set_value(first, "30s".into());
        assert!(draft.blocker().unwrap().contains("no option"));

        draft.client_config.set_key(first, "timeout".into());
        assert_eq!(draft.blocker(), None);
        assert_eq!(
            draft.def().client_config,
            [("timeout".to_string(), "30s".to_string())]
                .into_iter()
                .collect()
        );

        let second = draft.client_config.add("timeout".into(), "5s".into());
        assert!(draft.blocker().unwrap().contains("'timeout' is set twice"));
        draft.client_config.set_key(second, "user_agent".into());
        assert_eq!(draft.blocker(), None);

        draft.client_config.set_key(second, "nonsense".into());
        assert!(draft
            .blocker()
            .unwrap()
            .contains("'nonsense' is not a client option"));

        draft.client_config.remove(second);
        draft.client_config.remove(first);
        assert_eq!(draft.blocker(), None);
        assert!(draft.def().client_config.is_empty());
    }

    /// The name box's autocomplete, on the properties grid's contract: matches **anywhere**, hides
    /// what another row claims, and goes quiet on an exact hit rather than offering back what is
    /// already typed (a panel that would never close).
    #[test]
    fn suggestions_match_anywhere_hide_claimed_options_and_stop_at_an_exact_hit() {
        let mut rows = ConfigRows::default();
        let editing = rows.add("keep".into(), String::new());
        assert!(
            rows.suggestions(editing)
                .iter()
                .any(|e| e.name == "http2_keep_alive_interval"),
            "a substring matches, not only a prefix"
        );

        rows.add("http2_keep_alive_interval".into(), "5s".into());
        assert!(
            !rows
                .suggestions(editing)
                .iter()
                .any(|e| e.name == "http2_keep_alive_interval"),
            "another row already claims it"
        );

        let mut rows = ConfigRows::default();
        let exact = rows.add("user_agent".into(), String::new());
        assert!(
            rows.suggestions(exact).is_empty(),
            "offering back what is already typed is not a suggestion"
        );

        let mut rows = ConfigRows::default();
        let blank = rows.add(String::new(), String::new());
        assert_eq!(rows.suggestions(blank).len(), CLIENT_KEYS.len());
    }

    /// The box is tinted by whether the store will take the name — and an unknown one is an
    /// **error** here, where the properties grid calls it a warning: an engine key it has not
    /// heard of may be newer than this build, but `check_client_config` refuses an unknown client
    /// option outright.
    #[test]
    fn an_unknown_client_option_reads_as_an_error_rather_than_a_warning() {
        let mut rows = ConfigRows::default();
        let blank = rows.add(String::new(), String::new());
        let known = rows.add("timeout".into(), "30s".into());
        let unknown = rows.add("nonsense".into(), "1".into());

        assert_eq!(rows.names_a_client_option(blank), None, "nothing typed yet");
        assert_eq!(rows.names_a_client_option(known), Some(true));
        assert_eq!(rows.names_a_client_option(unknown), Some(false));
    }

    /// **Remove hands back the row that takes the removed one's place**, because the toolbar acts
    /// on a selection: a Remove that left the highlight on a row that no longer exists would arm
    /// the next press at nothing. The source-path list's contract, one key along.
    #[test]
    fn removing_a_client_option_names_the_row_to_select_next() {
        let mut rows = ConfigRows::default();
        let ids: Vec<u64> = ["timeout", "user_agent", "proxy_url"]
            .into_iter()
            .map(|key| rows.add(key.into(), "x".into()))
            .collect();

        assert_eq!(rows.remove(ids[1]), Some(ids[2]));
        assert_eq!(rows.remove(ids[2]), Some(ids[0]));
        assert_eq!(rows.remove(ids[0]), None);
        assert!(rows.is_empty());
        assert_eq!(rows.remove(99), None);
    }

    /// A def's options survive the round trip through the rows, in the map's own order.
    #[test]
    fn stored_client_options_come_back_as_rows() {
        let def = ConnectionDef {
            address: "acme-lake".into(),
            provider: Provider::S3(S3Store {
                region: "eu-west-2".into(),
                ..Default::default()
            }),
            client_config: [("user_agent", "strata"), ("timeout", "30s")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let draft = ConnectionDraft::of(&def);
        assert_eq!(
            draft
                .client_config
                .rows()
                .iter()
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>(),
            ["timeout", "user_agent"],
            "the map's sorted order is the table's"
        );
        assert_eq!(draft.def(), def);
    }

    /// Allow-HTTP is only meaningful beside an endpoint, so clearing the endpoint clears it —
    /// otherwise a def carries "allow plain http" against AWS itself, which is a claim the
    /// connection cannot act on and a reader has to explain away.
    #[test]
    fn allow_http_does_not_outlive_the_endpoint_it_qualifies() {
        let mut draft = ConnectionDraft {
            endpoint: "http://localhost:9000".into(),
            allow_http: true,
            ..s3_draft()
        };
        assert_eq!(
            draft.def().provider,
            Provider::S3(S3Store {
                region: "eu-west-2".into(),
                auth: S3Auth::Ambient,
                endpoint: "http://localhost:9000".into(),
                allow_http: true,
            })
        );
        draft.endpoint = String::new();
        let Provider::S3(s3) = draft.def().provider else {
            panic!("an S3 def");
        };
        assert!(!s3.allow_http);
    }
}
