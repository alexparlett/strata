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

use strata_core::engine::store::{check_client_config, client_key, ClientKey, CLIENT_KEYS};
use strata_model::{ConnectionDef, GcsAuth, GcsStore, Provider, ProviderId, S3Auth, S3Store};

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
            // Named for what it reads, because "Ambient" alone says nothing about *which*
            // ambient: GCS's chain is Application Default Credentials, which is the term the
            // user's own `gcloud` uses.
            Self::Ambient => "Ambient / ADC",
            Self::ServiceAccount => "Service-account file",
            Self::Anonymous => "Anonymous",
        }
    }
}

/// Everything the user has chosen.
#[derive(Clone, PartialEq, Debug)]
pub struct ConnectionDraft {
    pub provider: ProviderId,
    /// Where the connection points, in its provider's own terms — a bucket name for S3 and GCS,
    /// a whole origin URL for HTTP. [`ConnectionDef::address`]'s value, and the box's, verbatim.
    pub address: String,
    // --- S3 ---
    pub region: String,
    pub s3_auth: S3AuthId,
    pub profile: String,
    pub endpoint: String,
    pub allow_http: bool,
    // --- GCS ---
    pub gcs_auth: GcsAuthId,
    pub sa_path: String,
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
            client_config: ConfigRows::default(),
        }
    }
}

/// The catalogue is small enough to offer whole: the name box shows **every** match and scrolls,
/// where the properties grid caps its list at seven. What is capped here is the panel's *height*
/// (`views::form::SUGGEST_ROWS`), not the answer — an option cut from the list is one the user
/// cannot find by typing more, since these names share so many substrings.

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
    /// **S3 and GCS lose a scheme typed with the bucket**, because theirs is the picker's answer
    /// and `ConnectionDef::url` puts it back. Stripped on the way in rather than on the way out,
    /// the rule a length-capped field follows: a box showing `s3://acme-lake` over a def storing
    /// `acme-lake` shows one thing and means another.
    pub fn set_address(&mut self, typed: String) {
        self.address = match self.provider {
            ProviderId::Http => typed,
            _ => strip_scheme(&typed).to_string(),
        };
    }

    /// What the address box is called, and what it is called *in prose* — a bucket for the two
    /// object stores, a URL for HTTP. One pair, so the label and every sentence about it agree.
    pub fn address_label(&self) -> &'static str {
        match self.provider {
            ProviderId::Http => "URL",
            _ => "BUCKET",
        }
    }

    pub fn address_noun(&self) -> &'static str {
        match self.provider {
            ProviderId::Http => "URL",
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
                    // Only meaningful with an endpoint set (AWS itself is HTTPS), so a toggle
                    // left on from an endpoint that has since been cleared does not ride along
                    // into the def.
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
    /// `strata_core::engine::store::connect` accepts, in the same words. That is stronger than
    /// the two agreeing by inspection: S3's rules and GCS's differ in four places, and a form
    /// holding its own copy would drift from the store's the first time either moved.
    ///
    /// Everything else is this form's, because it is about a *control* rather than about the def:
    /// an auth mode with nothing chosen behind it is a half-answered question, not an invalid
    /// connection.
    pub fn blocker(&self) -> Option<String> {
        let def = self.def();
        if def.address.is_empty() {
            // Said in the terms of the field rather than of the row: the store's own message for
            // an empty address names the def ("This connection has no bucket"), which reads oddly
            // over a box the user is still filling in.
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
        }
        // The client options last: they are the same on every provider, and a half-typed one is
        // less urgent than a connection with no region.
        if let Some(why) = self.client_config.blocker() {
            return Some(why);
        }
        if let Err(why) = check_client_config(&def.client_config) {
            return Some(why);
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
            // The plain-`http` origin, which is a different origin and not a laxer way of
            // reaching the same one — so it has to survive the round trip as itself.
            ConnectionDef {
                address: "http://aserver:8484".into(),
                provider: Provider::Http,
                client_config: Default::default(),
            },
        ] {
            assert_eq!(ConnectionDraft::of(&def).def(), def, "{}", def.url());
        }
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

        // A **path** is a validation error we point out, not something trimmed off behind the
        // user: the registry keys on scheme and authority, so a connection carrying one would go
        // in under a key nothing looks up while the box went on showing it. The message quotes
        // the part to drop.
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

        // A scheme is not optional either — the field is the URL, so half of one is refused
        // rather than completed on the user's behalf.
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
            // Not a scheme: a host and its port.
            ("example.com:8080", "example.com:8080"),
            // Nor is this — the leading segment has to look like a scheme.
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

        // A path in a *bucket* name is refused by the charset rule — a slash is simply not one
        // of the characters a bucket may hold. (HTTP's own path case is above, where the address
        // is a URL and the message quotes the part to drop.)
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

        // HTTP has nothing to configure but its URL, so a URL is the whole of its validation.
        let http = ConnectionDraft {
            provider: ProviderId::Http,
            address: "https://example.com".into(),
            ..Default::default()
        };
        assert_eq!(http.blocker(), None);
        // …and the prose about it is the URL's, not the bucket's.
        let mut urlless = http.clone();
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

        // A fresh row blocks nothing: it is the state the Add button leaves behind.
        let first = draft.client_config.add(String::new(), String::new());
        assert_eq!(draft.blocker(), None);
        assert!(draft.def().client_config.is_empty(), "and reaches no def");

        // A value with nowhere to go is refused rather than dropped.
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

        // The same option twice has no meaning in the map, so it is named before it gets there.
        let second = draft.client_config.add("timeout".into(), "5s".into());
        assert!(draft.blocker().unwrap().contains("'timeout' is set twice"));
        draft.client_config.set_key(second, "user_agent".into());
        assert_eq!(draft.blocker(), None);

        // An option `object_store` has never heard of is the engine's own answer, reached
        // through the same call `connect` makes — not a second list kept here.
        draft.client_config.set_key(second, "nonsense".into());
        assert!(draft
            .blocker()
            .unwrap()
            .contains("'nonsense' is not a client option"));

        // …and a row removed is a row gone from the def.
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

        // `user_agent`, not `timeout`: the quiet arm fires when the typed text matches exactly
        // one catalogue name, and `timeout` is a substring of four of them.
        let mut rows = ConfigRows::default();
        let exact = rows.add("user_agent".into(), String::new());
        assert!(
            rows.suggestions(exact).is_empty(),
            "offering back what is already typed is not a suggestion"
        );

        // A blank box offers the **whole** catalogue: the panel scrolls rather than truncating,
        // because these names share so many substrings that a cut entry is one typing cannot find.
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

        // A row in the middle hands over the one below it.
        assert_eq!(rows.remove(ids[1]), Some(ids[2]));
        // The last row hands over the new last.
        assert_eq!(rows.remove(ids[2]), Some(ids[0]));
        // …and the only row hands over nothing, which is what disarms the button.
        assert_eq!(rows.remove(ids[0]), None);
        assert!(rows.is_empty());
        // An id the list never held selects the last row rather than reporting a removal.
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
