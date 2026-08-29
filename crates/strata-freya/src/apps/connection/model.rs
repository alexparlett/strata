//! The connection editor's data: what is being edited ([`ConnectionTarget`]), what the user has
//! chosen ([`ConnectionDraft`]), and why a draft cannot be saved yet.
//!
//! **A draft is a source and nothing else, and this module names no source.** [`ConnectionDraft`]
//! is a key-to-value map beside the keys the registry handed over ([`ConnectionKey`]), so a kind
//! the engine gains is a kind this form already edits, and a kind it does not serve has no shape
//! here at all. The same rule the flat `SourceDef` follows one layer down: what a source is
//! configured by is the source's business.
//!
//! **The object stores are not here.** `Provider::{S3, Gcs, Http}` are typed arms with typed
//! settings, which is the one thing a declaration-driven form cannot render — so rather than
//! keep a second, hand-written dress beside the generic one, the editor serves registrants only
//! until EA-25 makes S3, GCS and HTTP `DataSource`s like any other. Defs already on disk keep
//! working: they are listed, queried and forgotten exactly as before, and only *editing* one is
//! withheld ([`crate::apps::project::views::sidebar::catalog::menu`] parks it).

use std::collections::{BTreeMap, BTreeSet};

use strata_engine::{ConnectionKey, Field, Slot, SourceInfo, When};
use strata_model::{check_catalog, mint_name, ConnectionDef, Provider, SourceDef};

/// What this window is editing: a new connection, or an existing one by
/// [`named`](ConnectionDef::named).
///
/// The name is the handle — the one thing every surface addresses a connection by — and it is
/// also what makes this window single-instance per target: two windows on one def would both
/// `upsert_connection` and both persist.
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
    /// say there yet — its name is being typed.
    pub fn subtitle(&self) -> Option<&str> {
        self.editing()
    }

    /// The connection this window opened on, if any — what a rename is measured against.
    pub fn editing(&self) -> Option<&str> {
        match self {
            Self::New => None,
            Self::Edit(name) => Some(name),
        }
    }
}

/// What this machine's keystore said about the entry a declared secret expects. Read once at
/// mount, and only where the def expects one.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum SecretProbe {
    #[default]
    Asking,
    Stored,
    Absent,
    /// The keystore refused to answer, in its own words — never folded into
    /// [`Absent`](Self::Absent), which would claim a fact nobody established.
    Refused(String),
}

/// What a [`Field::Secret`] row is showing: its sentence and both of its presses, off one value.
///
/// A secret is optional wherever its key says so, so absence is a state rather than a mode and
/// there is no pill; every arm is reachable by typing into the box or by one of the two presses.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SecretRow {
    /// Something is typed: it lands in this machine's keystore at Save.
    Typed,
    /// Nothing expected. `forgetting` when this machine's entry goes in the same Save.
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

impl SecretRow {
    /// What the def expects with the box empty, whether anything is typed, whether a removal is
    /// pending, and what the keystore said.
    pub fn of(expected: bool, typed: bool, removed: bool, probe: &SecretProbe) -> Self {
        if typed {
            return Self::Typed;
        }
        match expected {
            false => Self::Unused {
                forgetting: removed,
            },
            true if removed => Self::Removing,
            true => match probe {
                SecretProbe::Asking => Self::Asking,
                SecretProbe::Stored => Self::Stored,
                SecretProbe::Absent => Self::Missing,
                SecretProbe::Refused(why) => Self::Refused(why.clone()),
            },
        }
    }

    /// The line under the box, each arm about **this machine** — the half a committed def cannot
    /// state. A marker echoing the def's own expectation would read "a password is stored" on a
    /// machine that has never held one.
    ///
    /// `noun` is the key's own label, set in prose ([`noun`]), so a source with
    /// two credentials says which one each row is about.
    pub fn note(&self, noun: &str) -> String {
        match self {
            Self::Typed => {
                format!("This {noun} goes into this machine's keystore when you save.")
            }
            Self::Unused { forgetting: false } => {
                format!("This connection signs in without a {noun}.")
            }
            Self::Unused { forgetting: true } => format!(
                "This connection signs in without a {noun}. The one stored on this machine is \
                 removed when you save."
            ),
            Self::Stored => {
                format!("A {noun} is stored on this machine. Type a new one to replace it.")
            }
            Self::Missing => format!(
                "This connection expects a {noun} and none is stored on this machine. Enter it \
                 here."
            ),
            Self::Removing => format!(
                "The {noun} stored on this machine is removed when you save. This connection \
                 still expects one, so other machines keep theirs."
            ),
            Self::Asking => "Checking this machine's keystore…".into(),
            Self::Refused(why) => why.clone(),
        }
    }

    /// Whether **Remove from this machine** is offered: there has to be an entry here to remove.
    pub fn offers_removal(&self) -> bool {
        matches!(self, Self::Stored)
    }

    /// Whether **This connection uses no …** is offered — wherever one is still expected,
    /// including while the keystore is asked or refusing, since the press edits the def rather
    /// than this machine.
    pub fn offers_disuse(&self) -> bool {
        !matches!(self, Self::Typed | Self::Unused { .. })
    }
}

/// A declared key's label, set in prose — what the sentences about it call it.
///
/// The label is the row's own eyebrow (`PASSWORD`), and a sentence cannot carry it in that
/// register. Derived rather than declared, so a key that reaches the form reaches its sentences
/// with it and a source has one fewer thing to state.
pub fn noun(key: &ConnectionKey) -> String {
    key.label.to_lowercase()
}

/// Everything the user has chosen: which registered kind serves this connection, and a value per
/// key that kind declared.
///
/// **One type, because a connection is one thing.** The address is a declared key like the rest
/// ([`Slot::Address`]); it simply lands on a typed field rather than in the open map, which is
/// what [`value`](Self::value) and [`set`](Self::set) route. This is the shape the *def* takes
/// when `ConnectionDef` and `SourceDef` collapse (EA-25) — the draft gets there first because it
/// has no serde to migrate.
///
/// **Nothing here is named after a source.** The keys arrive from the registry
/// ([`SourceInfo::keys`]) and travel with the kind that declared them, so a value typed for one
/// kind survives a trip through the picker while [`def`](Self::def) writes only what the kind in
/// play declares. That is what makes registering a `DataSource` put a working form in front of it
/// with no code here.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ConnectionDraft {
    /// Which registered kind serves this connection — the picker's answer, and the registry key.
    pub kind: String,
    /// What that kind declares it takes, as the registry handed it over. Set with
    /// [`kind`](Self::kind) and never apart from it, so the rows drawn and the values written
    /// cannot describe different sources.
    pub keys: &'static [ConnectionKey],
    /// The handle: the connection's display name and its SQL catalog identifier, one field.
    /// Blank is not nameless — [`named`](Self::named) mints one from the address.
    ///
    /// **The editor's own row, not a declared one**: the store keys by it, the catalog is named
    /// by it and `check_catalog` judges it, and a source has no opinion about any of that. A kind
    /// that could omit it could produce an unnameable connection.
    pub name: String,
    /// Where the connection points, in its kind's own terms — the [`Slot::Address`] key's value.
    pub address: String,
    /// A value per declared [`Slot::Setting`] key, as typed. Trimmed into the def, never on the
    /// way in. **No [`Field::Secret`] value is ever here** — those go to this machine's keystore.
    pub config: BTreeMap<String, String>,
    /// Which of the kind's secret-typed keys this connection has a value for — the def's
    /// expectation, which no control writes directly (the window's own slots derive it).
    pub secrets: BTreeSet<String>,
    pub schemas: Vec<String>,
    pub read_only: bool,
}

impl ConnectionDraft {
    /// A new connection, on the first source the engine serves.
    ///
    /// Picking one rather than opening blank: the picker is a list of registrants, so "none
    /// chosen" is a state only a build with no sources can be in, and a form with no rows and no
    /// way to get any is not a starting point.
    pub fn new(registrants: &[SourceInfo]) -> Self {
        let mut draft = Self::default();
        if let Some(first) = registrants.first() {
            draft.adopt(first);
        }
        draft
    }

    /// Adopt `info`'s kind and its declaration together.
    pub fn adopt(&mut self, info: &SourceInfo) {
        self.kind = info.kind.to_string();
        self.keys = info.keys;
    }

    /// Seed the draft from an existing def — every field it holds, so the window opens showing
    /// what is really stored and Save with nothing touched writes back the def that was there.
    ///
    /// `registrants` is what the engine can serve, which is where the declaration comes from: a
    /// def naming a kind nothing answers to opens with no rows, which is the honest form for a
    /// connection this build cannot describe.
    ///
    /// # Panics
    ///
    /// If `def` is not a source. Unreachable by gesture — the editor is only offered for a
    /// registrant's def — and a silent empty draft here would let Save rewrite an object store's
    /// connection as something else.
    pub fn of(def: &ConnectionDef, registrants: &[SourceInfo]) -> Self {
        let Provider::Source(source) = &def.provider else {
            panic!(
                "edit '{}': its provider is not a registered source, so there is no form for it",
                def.named()
            );
        };
        let keys = registrants
            .iter()
            .find(|info| info.kind == source.kind.trim())
            .map_or(&[][..], |info| info.keys);
        Self {
            kind: source.kind.trim().to_string(),
            keys,
            name: def.named(),
            address: def.address.clone(),
            config: source.config.clone(),
            secrets: source.secrets.clone(),
            schemas: source.schemas.clone(),
            read_only: source.read_only,
        }
    }

    /// The key `key` is declared as, or `None` where this kind does not take it.
    pub fn declared(&self, key: &str) -> Option<&'static ConnectionKey> {
        self.keys.iter().find(|declared| declared.key == key)
    }

    /// What the box for `key` shows: what has been typed, or what the key declares when nothing
    /// has. A key with neither shows nothing, which is what an optional free-text setting is.
    ///
    /// Routed by the key's [`Slot`], which is the whole of what a slot means here.
    pub fn value(&self, key: &str) -> String {
        let declared = self.declared(key);
        if declared.is_some_and(|declared| declared.slot == Slot::Address) {
            return self.address.clone();
        }
        match self.config.get(key) {
            Some(typed) => typed.clone(),
            None => declared
                .and_then(|declared| declared.default)
                .unwrap_or_default()
                .to_string(),
        }
    }

    /// Type into `key`'s box.
    ///
    /// **An address loses a scheme typed with it**, because the kind states the scheme and the
    /// identity puts it back. Stripped on the way in rather than on the way out, the rule a
    /// length-capped field follows: a box showing `postgres://db:5432/analytics` over a def
    /// storing `db:5432/analytics` shows one thing and means another.
    pub fn set(&mut self, key: &str, value: String) {
        match self.declared(key).map(|declared| declared.slot) {
            Some(Slot::Address) => self.address = strip_scheme(&value).to_string(),
            _ => {
                self.config.insert(key.to_string(), value);
            }
        }
    }

    /// Whether `declared` is offered at all — its [`When`] asked of the key that decides.
    ///
    /// A hidden setting **keeps its value**, so moving the deciding key back brings the box back
    /// with what was in it, and [`def`](Self::def) still writes it: what a mode reads is the
    /// source's business, and a path typed under one is not a thing to discard because the mode
    /// moved.
    pub fn shows(&self, declared: &ConnectionKey) -> bool {
        match declared.when {
            None => true,
            Some(When { key, values }) => values.contains(&self.value(key).trim()),
        }
    }

    /// The def this draft describes — **projected through the declaration**, so a value left
    /// behind by a trip through the picker is held for the user and written for nobody, and every
    /// value is trimmed here rather than on the way in.
    pub fn def(&self) -> ConnectionDef {
        let mut config = BTreeMap::new();
        let mut secrets = BTreeSet::new();
        for declared in self.keys.iter().filter(|d| d.slot == Slot::Setting) {
            match declared.field {
                Field::Secret => {
                    if self.secrets.contains(declared.key) {
                        secrets.insert(declared.key.to_string());
                    }
                }
                _ => {
                    let value = self.value(declared.key);
                    let value = value.trim();
                    if !value.is_empty() {
                        config.insert(declared.key.to_string(), value.to_string());
                    }
                }
            }
        }
        ConnectionDef {
            address: self.address.trim().to_string(),
            name: self.named(),
            client_config: BTreeMap::new(),
            provider: Provider::Source(SourceDef {
                kind: self.kind.trim().to_string(),
                config,
                secrets,
                schemas: self.schemas.clone(),
                read_only: self.read_only,
            }),
        }
    }

    /// What this connection is called — **the handle**, and the SQL catalog identifier with it.
    ///
    /// The name box, or [`mint_name`] over the address for anything left blank.
    /// [`ConnectionDef::named`]'s own rule, so the draft and the def cannot disagree about what a
    /// blank name means.
    pub fn named(&self) -> String {
        match self.name.trim() {
            "" => self.minted(),
            named => named.to_string(),
        }
    }

    /// What this connection is called when nothing has been typed in the name box — the address's
    /// own mint, which is what the box shows as its placeholder.
    pub fn minted(&self) -> String {
        mint_name(self.address.trim())
    }

    /// Why this draft cannot be saved yet, or `None` when it can.
    ///
    /// Two questions, and only two: whether the handle is one a query could write, and whether
    /// every question the form actually **put** has an answer. A key that is not shown is not
    /// asked about — a question nobody put cannot be unanswered — and what a *value* may be is
    /// the kind's own rule, asked by [`connect`](strata_engine::DataSource::connect), which is
    /// the real gate.
    ///
    /// What a **kind** makes of an address, and whether another connection already holds this
    /// name, are the footer's: both need something this value does not have — the registry, and
    /// the project's other rows.
    pub fn blocker(&self) -> Option<String> {
        if let Err(why) = check_catalog(&self.named()) {
            return Some(why);
        }
        self.keys
            .iter()
            .filter(|declared| declared.required && self.shows(declared))
            .find(|declared| match declared.field {
                Field::Secret => !self.secrets.contains(declared.key),
                _ => self.value(declared.key).trim().is_empty(),
            })
            .map(|declared| format!("This connection has no {}.", noun(declared)))
    }
}

/// `postgres://db:5432/analytics` → `db:5432/analytics`. Anything that is not a scheme is left
/// alone, so a host with a port (`example.com:8080`) survives — `://` is what makes it a scheme,
/// not the colon.
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
    use strata_engine::SourceMode;

    use super::*;

    /// A declaration exercising every facet a form is drawn from — an address, two groups, a
    /// default, a conditional key and a required one — and named after nothing shipped, because
    /// the form is not allowed to know a shipped kind either.
    const TEST_KEYS: &[ConnectionKey] = &[
        ConnectionKey {
            key: "address",
            label: "ADDRESS",
            field: Field::Text,
            slot: Slot::Address,
            group: Some("CONNECTION"),
            required: true,
            default: None,
            when: None,
            hint: Some("Where the test source is"),
            placeholder: None,
        },
        ConnectionKey {
            key: "user",
            label: "USER",
            field: Field::Text,
            slot: Slot::Setting,
            group: Some("CONNECTION"),
            required: true,
            default: None,
            when: None,
            hint: Some("The role to log in as"),
            placeholder: None,
        },
        ConnectionKey {
            key: "password",
            label: "PASSWORD",
            field: Field::Secret,
            slot: Slot::Setting,
            group: Some("CONNECTION"),
            required: false,
            default: None,
            when: None,
            hint: None,
            placeholder: None,
        },
        ConnectionKey {
            key: "mode",
            label: "MODE",
            field: Field::Choice(&["off", "on"]),
            slot: Slot::Setting,
            group: Some("SECURITY"),
            required: false,
            default: Some("off"),
            when: None,
            hint: None,
            placeholder: None,
        },
        ConnectionKey {
            key: "certificate",
            label: "ROOT CERTIFICATE",
            field: Field::Path,
            slot: Slot::Setting,
            group: Some("SECURITY"),
            required: true,
            default: None,
            when: Some(When {
                key: "mode",
                values: &["on"],
            }),
            hint: None,
            placeholder: Some("/path/to/root.pem"),
        },
    ];

    /// A second kind, taking nothing but an address.
    const OTHER_KEYS: &[ConnectionKey] = &[ConnectionKey {
        key: "address",
        label: "ADDRESS",
        field: Field::Text,
        slot: Slot::Address,
        group: None,
        required: true,
        default: None,
        when: None,
        hint: None,
        placeholder: None,
    }];

    fn info(keys: &'static [ConnectionKey]) -> SourceInfo {
        SourceInfo {
            kind: "test",
            label: "Test source",
            badge: "TST",
            mode: SourceMode::Catalog,
            keys,
            writable: true,
        }
    }

    fn source_draft() -> ConnectionDraft {
        ConnectionDraft {
            kind: "test".into(),
            keys: TEST_KEYS,
            name: "warehouse".into(),
            address: "db.internal:5432/analytics".into(),
            config: [("user".to_string(), "reader".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }

    /// The round trip an edit is: open on a def, touch nothing, Save writes back what was there.
    #[test]
    fn a_def_survives_the_draft_untouched() {
        let def = ConnectionDef {
            address: "db.internal:5432/analytics".into(),
            name: "warehouse".into(),
            provider: Provider::Source(SourceDef {
                kind: "test".into(),
                config: [
                    ("user", "reader"),
                    ("mode", "on"),
                    ("certificate", "/c.pem"),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
                secrets: BTreeSet::from(["password".to_string()]),
                schemas: vec!["public".into(), "analytics".into()],
                read_only: false,
            }),
            client_config: Default::default(),
        };
        assert_eq!(ConnectionDraft::of(&def, &[info(TEST_KEYS)]).def(), def);
    }

    /// **The address is a declared key whose value lands on a typed field.** That is the whole of
    /// what a [`Slot`] means: the row is declared like any other — label, hint, placeholder,
    /// required — while `identity()` and [`mint_name`] still read an address every connection is
    /// guaranteed to have, which a key in the open map could not promise.
    #[test]
    fn the_address_is_a_declared_key_over_a_typed_field() {
        let mut draft = source_draft();
        assert_eq!(draft.value("address"), "db.internal:5432/analytics");

        draft.set("address", "postgres://other:5433/sales".into());
        assert_eq!(
            draft.address, "other:5433/sales",
            "written through the router, and a pasted scheme goes"
        );
        assert_eq!(draft.value("address"), "other:5433/sales");

        let def = draft.def();
        assert_eq!(def.address, "other:5433/sales");
        let Provider::Source(source) = &def.provider else {
            panic!("a source def");
        };
        assert!(
            !source.config.contains_key("address"),
            "and it never reaches the open map: {:?}",
            source.config
        );
    }

    /// **A new connection opens on the first source the engine serves**, because the picker is a
    /// list of registrants: "none chosen" is a state only a build with no sources can reach.
    #[test]
    fn a_new_draft_adopts_the_first_registrant() {
        let draft = ConnectionDraft::new(&[info(TEST_KEYS)]);
        assert_eq!(draft.kind, "test");
        assert_eq!(draft.keys, TEST_KEYS);

        let bare = ConnectionDraft::new(&[]);
        assert!(bare.kind.is_empty(), "nothing to offer, nothing set");
        assert!(bare.keys.is_empty());
    }

    /// **A def naming a kind nothing is registered for opens with no rows** rather than with the
    /// last kind's — and it keeps what it holds, because nothing here writes a config it cannot
    /// draw.
    #[test]
    fn a_def_whose_kind_is_not_registered_opens_with_no_declaration() {
        let def = ConnectionDef {
            address: "somewhere".into(),
            name: "mystery".into(),
            provider: Provider::Source(SourceDef {
                kind: "mongo".into(),
                ..Default::default()
            }),
            client_config: Default::default(),
        };
        let draft = ConnectionDraft::of(&def, &[info(TEST_KEYS)]);
        assert_eq!(draft.kind, "mongo");
        assert!(draft.keys.is_empty(), "nothing declares its rows");
    }

    /// **The form writes what the kind declares and nothing else.** A value typed under one kind
    /// survives a trip through the picker — that is the whole reason the draft holds a map rather
    /// than a def — while the def in play carries only the keys the kind in play takes. The
    /// address is every kind's, so it is the one value a trip cannot strand.
    #[test]
    fn only_the_declared_keys_reach_the_def() {
        let mut draft = source_draft();
        draft.set("certificate", "/certs/rds.pem".into());

        let Provider::Source(def) = draft.def().provider else {
            panic!("a source def");
        };
        assert_eq!(def.config.get("user").map(String::as_str), Some("reader"));
        assert_eq!(
            def.config.get("mode").map(String::as_str),
            Some("off"),
            "a key the box never touched is written as the key declares it"
        );

        draft.adopt(&info(OTHER_KEYS));
        let moved = draft.def();
        let Provider::Source(other) = &moved.provider else {
            panic!("a source def");
        };
        assert!(
            other.config.is_empty(),
            "this kind takes none of them: {:?}",
            other.config
        );
        assert_eq!(moved.address, "db.internal:5432/analytics");

        draft.adopt(&info(TEST_KEYS));
        assert_eq!(
            draft.value("certificate"),
            "/certs/rds.pem",
            "and the trip forgot nothing"
        );
    }

    /// **A required key that is empty is the one thing a generic form can be wrong about.** What
    /// a value may *be* is the kind's rule, asked by connect — and the address is refused here on
    /// exactly the same terms as any other key, because it is one.
    #[test]
    fn a_required_key_with_nothing_in_it_blocks_the_save() {
        let good = source_draft();
        assert_eq!(good.blocker(), None);

        let mut userless = good.clone();
        userless.set("user", "  ".into());
        assert_eq!(
            userless.blocker(),
            Some("This connection has no user.".into())
        );

        let mut addressless = good;
        addressless.set("address", String::new());
        assert_eq!(
            addressless.blocker(),
            Some("This connection has no address.".into())
        );
    }

    /// **A setting another setting has made irrelevant is not a control.** The kind declares when
    /// its key is offered ([`When`]); the form neither knows the condition nor invents one.
    ///
    /// Three things follow: the row is absent rather than disabled, a *required* key that is not
    /// shown does not block a save (a question nobody put cannot be unanswered), and the value is
    /// **kept** — moving the deciding key back brings the box back with what was in it.
    #[test]
    fn a_key_another_keys_answer_hides_is_not_asked_about() {
        let cert = TEST_KEYS
            .iter()
            .find(|declared| declared.key == "certificate")
            .expect("the conditional key");

        let mut draft = source_draft();
        assert_eq!(draft.value("mode"), "off", "the declared default");
        assert!(!draft.shows(cert), "'off' does not read one");
        assert_eq!(
            draft.blocker(),
            None,
            "and it is required, which is exactly what must not block while it is unasked"
        );

        draft.set("mode", "on".into());
        assert!(draft.shows(cert));
        assert_eq!(
            draft.blocker(),
            Some("This connection has no root certificate.".into()),
            "now it is asked, so now it blocks"
        );

        draft.set("certificate", "/certs/rds.pem".into());
        assert_eq!(draft.blocker(), None);

        draft.set("mode", "off".into());
        assert!(!draft.shows(cert));
        assert_eq!(
            draft.value("certificate"),
            "/certs/rds.pem",
            "hidden is not forgotten"
        );
        let Provider::Source(def) = draft.def().provider else {
            panic!("a source def");
        };
        assert_eq!(
            def.config.get("certificate").map(String::as_str),
            Some("/certs/rds.pem"),
            "and what a mode reads is the source's business, not a reason to discard the path"
        );
    }

    /// **A blank name box is not a nameless connection**: the address mints one, which is what
    /// the box shows as its placeholder and what every surface then addresses it by.
    #[test]
    fn a_blank_name_is_minted_from_the_address() {
        let mut draft = source_draft();
        draft.name = String::new();
        assert_eq!(draft.minted(), "analytics");
        assert_eq!(draft.named(), "analytics");
        assert_eq!(draft.blocker(), None);

        draft.name = " depot ".into();
        assert_eq!(draft.named(), "depot", "and a typed one wins, trimmed");
    }

    /// **The name is checked on the terms a catalog identifier is**, so a name this form accepts
    /// is one registration accepts, in the same words.
    #[test]
    fn a_name_is_one_a_query_could_write() {
        let mut reserved = source_draft();
        reserved.name = "STRATA".into();
        assert!(reserved
            .blocker()
            .unwrap()
            .contains("this project's own catalog"));

        let mut spaced = source_draft();
        spaced.name = "two words".into();
        assert!(spaced.blocker().unwrap().contains("not a name queries"));
    }

    /// **A source def's text is trimmed into the def like every other field here.** The engine
    /// trims at use, so an untrimmed value still connects — while the committed, *shared*
    /// `project.json` would record the spaces, and the def is what every surface displays.
    #[test]
    fn a_source_defs_text_is_trimmed() {
        let mut draft = source_draft();
        draft.address = "  db.internal:5432/analytics  ".into();
        draft.name = " warehouse ".into();
        draft.set("user", " reader ".into());
        draft.secrets.insert("password".into());
        draft.read_only = false;

        let def = draft.def();
        assert_eq!(def.address, "db.internal:5432/analytics");
        assert_eq!(def.named(), "warehouse");
        let Provider::Source(source) = &def.provider else {
            panic!("a source def");
        };
        assert_eq!(
            source.config.get("user").map(String::as_str),
            Some("reader")
        );
        assert!(
            source.secrets.contains("password"),
            "carried, never trimmed"
        );
        assert!(!source.read_only, "and so is the write opt-in");
    }

    /// **A secret row is about *this machine*, not about the def.** A committed def can only say
    /// a secret is expected; conflating that with "one is stored" tells a colleague opening a
    /// shared project that theirs is already here.
    #[test]
    fn a_secret_row_reports_this_machine_rather_than_the_def() {
        use SecretProbe as P;

        let row =
            |expected, typed, removed, probe: &P| SecretRow::of(expected, typed, removed, probe);

        assert_eq!(
            row(false, false, false, &P::Absent),
            SecretRow::Unused { forgetting: false }
        );
        assert_eq!(row(true, false, false, &P::Stored), SecretRow::Stored);
        assert_eq!(
            row(true, false, false, &P::Absent),
            SecretRow::Missing,
            "expected, and this machine has none"
        );
        assert_eq!(row(true, false, false, &P::Asking), SecretRow::Asking);
        assert_eq!(
            row(true, false, false, &P::Refused("locked".into())),
            SecretRow::Refused("locked".into())
        );

        assert_eq!(
            row(true, true, false, &P::Stored),
            SecretRow::Typed,
            "what is being typed outranks what is stored, in every state"
        );
        assert_eq!(row(false, true, true, &P::Absent), SecretRow::Typed);
    }

    /// **Every sentence a secret row says names the key it is about**, off the key's own label,
    /// so a source with two credentials has two rows that read differently.
    #[test]
    fn a_secret_rows_sentences_name_the_key_they_are_about() {
        let key = TEST_KEYS
            .iter()
            .find(|declared| declared.key == "password")
            .expect("the declaration");
        assert_eq!(noun(key), "password");
        assert!(SecretRow::Typed.note(&noun(key)).contains("This password"));

        let other = ConnectionKey {
            label: "SECRET ACCESS KEY",
            ..*key
        };
        assert!(SecretRow::Missing
            .note(&noun(&other))
            .contains("expects a secret access key"));
    }

    /// **The two clearing gestures are not the same gesture.** *Remove from this machine* leaves
    /// the expectation standing so a colleague keeps their own secret; *this connection uses no
    /// …* edits the shared def.
    #[test]
    fn removing_a_secret_locally_is_not_declaring_the_connection_has_none() {
        let removing = SecretRow::of(true, false, true, &SecretProbe::Stored);
        assert_eq!(removing, SecretRow::Removing);
        assert!(
            removing
                .note("password")
                .contains("other machines keep theirs"),
            "{}",
            removing.note("password")
        );

        let unused = SecretRow::of(false, false, true, &SecretProbe::Stored);
        assert_eq!(unused, SecretRow::Unused { forgetting: true });
        assert!(
            unused.note("password").contains("without a password"),
            "{}",
            unused.note("password")
        );
    }

    /// **Neither press is offered where it would mean nothing**, and neither is a dead end.
    #[test]
    fn the_secret_presses_are_offered_where_they_do_something() {
        assert!(SecretRow::Stored.offers_removal());
        for row in [
            SecretRow::Typed,
            SecretRow::Missing,
            SecretRow::Removing,
            SecretRow::Asking,
            SecretRow::Unused { forgetting: false },
        ] {
            assert!(!row.offers_removal(), "{row:?}: nothing here to remove");
        }

        for row in [
            SecretRow::Stored,
            SecretRow::Missing,
            SecretRow::Removing,
            SecretRow::Asking,
            SecretRow::Refused("locked".into()),
        ] {
            assert!(row.offers_disuse(), "{row:?}: a secret is still expected");
        }
        assert!(
            !SecretRow::Unused { forgetting: false }.offers_disuse(),
            "already the answer"
        );
        assert!(
            !SecretRow::Typed.offers_disuse(),
            "a box with a secret in it is not the place to say there is none"
        );
    }
}
