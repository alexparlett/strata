//! The data source editor's data: what is being edited ([`SourceTarget`]), what the user has
//! chosen ([`Data sourceDraft`]), and why a draft cannot be saved yet.
//!
//! **A draft is a source and nothing else, and this module names no source.** [`Data sourceDraft`]
//! is a key-to-value map beside the keys the registry handed over ([`SourceSetting`]), so a kind
//! the engine gains is a kind this form already edits, and a kind it does not serve has no shape
//! here at all. The same rule the flat `SourceDef` follows one layer down: what a source is
//! configured by is the source's business.

use std::collections::BTreeMap;

use strata_engine::{Field, SourceInfo, SourceSetting, When};
use strata_model::{check_catalog, SecretRef, Secrets, SourceDef};

/// What this window is editing: a new data source, or an existing one by
/// [`named`](SourceDef::named).
///
/// The name is the handle — the one thing every surface addresses a data source by — and it is
/// also what makes this window single-instance per target: two windows on one def would both
/// `upsert_data source` and both persist.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SourceTarget {
    New,
    Edit(String),
}

impl SourceTarget {
    /// The window's title.
    pub fn title(&self) -> &'static str {
        match self {
            Self::New => "New data source",
            Self::Edit(_) => "Edit data source",
        }
    }

    /// The line under the title: the data source this window opened on. A new one has nothing to
    /// say there yet — its name is being typed.
    pub fn subtitle(&self) -> Option<&str> {
        self.editing()
    }

    /// The data source this window opened on, if any — what a rename is measured against.
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
    /// Expected, this machine does not hold it, and the kind **requires** one: nothing can
    /// connect until it is entered here.
    Missing,
    /// Expected, this machine does not hold it, and the kind does **not** require one.
    ///
    /// States the fact and asks for nothing: a key declared `required: false` is one a data
    /// source may simply not have, so an empty box is not a gap to be filled. Distinct from
    /// [`Missing`](Self::Missing), which is the same absence where the kind does require one.
    Optional,
    /// Expected, and this machine's entry is being removed at Save. Other machines keep theirs,
    /// which is the whole difference from [`Unused`](Self::Unused).
    Removing,
    Asking,
    Refused(String),
}

impl SecretRow {
    /// What the def expects with the box empty, whether anything is typed, whether a removal is
    /// pending, and what the keystore said.
    pub fn of(
        required: bool,
        expected: bool,
        typed: bool,
        removed: bool,
        probe: &SecretProbe,
    ) -> Self {
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
                SecretProbe::Refused(why) => Self::Refused(why.clone()),
                SecretProbe::Absent if required => Self::Missing,
                SecretProbe::Absent => Self::Optional,
            },
        }
    }

    /// Whether Save should keep the def's expectation of this secret.
    ///
    /// The one place the box's empty state is interpreted, because it means two things and only
    /// this value can tell them apart: with something stored here it means *leave it alone* (a
    /// stored secret is never rendered, so empty is its resting state), and with nothing stored
    /// and nothing required it means *there is none*.
    ///
    /// True for every arm but [`Unused`](Self::Unused), which is the def already saying there is
    /// none: an expectation is dropped by an act of the user, never by this machine's keystore
    /// happening to be empty. Opening a shared def on a machine with no entry and saving it
    /// untouched must leave the def exactly as it was.
    pub fn keeps_expectation(&self) -> bool {
        !matches!(self, Self::Unused { .. })
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
                format!("This data source signs in without a {noun}.")
            }
            Self::Unused { forgetting: true } => format!(
                "This data source signs in without a {noun}. The one stored on this machine is \
                 removed when you save."
            ),
            Self::Stored => {
                format!("A {noun} is stored on this machine. Type a new one to replace it.")
            }
            Self::Missing => format!(
                "This data source needs a {noun} and none is stored on this machine. Enter it \
                 here."
            ),
            Self::Optional => {
                format!("No {noun} is stored on this machine.")
            }
            Self::Removing => format!(
                "The {noun} stored on this machine is removed when you save. This data source \
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
}

/// A declared key's label, set in prose — what the sentences about it call it.
///
/// The label is the row's own eyebrow (`PASSWORD`), and a sentence cannot carry it in that
/// register. Derived rather than declared, so a key that reaches the form reaches its sentences
/// with it and a source has one fewer thing to state.
pub fn noun(key: &SourceSetting) -> String {
    key.label.to_lowercase()
}

/// Everything the user has chosen: which registered kind serves this data source, and a value per
/// key that kind declared.
///
/// **One type, because a data source is one thing.** The address is a declared key like the rest
/// and lands in the same map, so nothing here special-cases it.
///
/// **Nothing here is named after a source.** The keys arrive from the registry
/// ([`SourceInfo::keys`]) and travel with the kind that declared them, so a value typed for one
/// kind survives a trip through the picker while [`def`](Self::def) writes only what the kind in
/// play declares. That is what makes registering a `DataSource` put a working form in front of it
/// with no code here.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SourceDraft {
    /// Which registered kind serves this data source — the picker's answer, and the registry key.
    pub kind: String,
    /// What that kind declares it takes, as the registry handed it over. Set with
    /// [`kind`](Self::kind) and never apart from it, so the rows drawn and the values written
    /// cannot describe different sources.
    pub settings: &'static [SourceSetting],
    /// **The identity**: what the user calls this source, and the catalog its relations are
    /// addressed under. Never derived — a blank one is refused, not filled in.
    ///
    /// **The editor's own row, not a declared one**: the store keys by it, the catalog is named
    /// by it and `check_catalog` judges it, and a source has no opinion about any of that. A kind
    /// that could omit it could produce an unnameable data source.
    pub name: String,
    /// A value per declared key, as typed. Trimmed into the def, never on the
    /// way in. **No [`Field::Secret`] value is ever here** — those go to this machine's keystore.
    pub config: BTreeMap<String, String>,
    /// Which of the kind's secret-typed keys this data source has a value for — the def's
    /// expectation, which no control writes directly (the window's own slots derive it).
    pub secrets: BTreeMap<String, SecretRef>,
    pub schemas: Vec<String>,
    pub read_only: bool,
}

impl SourceDraft {
    /// A new data source, on the first source the engine serves.
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
        self.settings = info.settings;
    }

    /// Seed the draft from an existing def — every field it holds, so the window opens showing
    /// what is really stored and Save with nothing touched writes back the def that was there.
    ///
    /// `registrants` is what the engine can serve, which is where the declaration comes from: a
    /// def naming a kind nothing answers to opens with no rows, which is the honest form for a
    /// data source this build cannot describe.
    ///
    pub fn of(def: &SourceDef, registrants: &[SourceInfo]) -> Self {
        let settings = registrants
            .iter()
            .find(|info| info.kind == def.kind.trim())
            .map_or(&[][..], |info| info.settings);
        Self {
            kind: def.kind.trim().to_string(),
            settings,
            name: def.named(),
            config: def.config.clone(),
            secrets: def
                .secrets
                .keys()
                .into_iter()
                .filter_map(|key| Some((key.clone(), def.secret_slot(key)?)))
                .collect(),
            schemas: def.schemas.clone(),
            read_only: def.read_only,
        }
    }

    /// The key `key` is declared as, or `None` where this kind does not take it.
    pub fn declared(&self, key: &str) -> Option<&'static SourceSetting> {
        self.settings.iter().find(|declared| declared.key == key)
    }

    /// What the box for `key` shows: what has been typed, or what the key declares when nothing
    /// has. A key with neither shows nothing, which is what an optional free-text setting is.
    pub fn value(&self, key: &str) -> String {
        let declared = self.declared(key);
        match self.config.get(key) {
            Some(typed) => typed.clone(),
            None => declared
                .and_then(|declared| declared.default)
                .unwrap_or_default()
                .to_string(),
        }
    }

    /// Type into `key`'s box. Every setting lands the same way — there is no key this form
    /// treats differently, which is what "a source is its properties" means here.
    pub fn set(&mut self, key: &str, value: String) {
        self.config.insert(key.to_string(), value);
    }

    /// Whether `declared` is offered at all — its [`When`] asked of the key that decides.
    ///
    /// A hidden setting **keeps its value**, so moving the deciding key back brings the box back
    /// with what was in it, and [`def`](Self::def) still writes it: what a mode reads is the
    /// source's business, and a path typed under one is not a thing to discard because the mode
    /// moved.
    pub fn shows(&self, declared: &SourceSetting) -> bool {
        match declared.when {
            None => true,
            Some(When { key, values }) => values.contains(&self.value(key).trim()),
        }
    }

    /// The def this draft describes — **projected through the declaration**, so a value left
    /// behind by a trip through the picker is held for the user and written for nobody, and every
    /// value is trimmed here rather than on the way in.
    pub fn def(&self) -> SourceDef {
        let mut config = BTreeMap::new();
        let mut secrets = BTreeMap::new();
        for declared in self.settings {
            match declared.field {
                Field::Secret => {
                    if let Some(slot) = self.secrets.get(declared.key) {
                        secrets.insert(declared.key.to_string(), slot.clone());
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
        SourceDef {
            kind: self.kind.trim().to_string(),
            name: self.named(),
            config,
            secrets: Secrets::Filed(secrets),
            schemas: self.schemas.clone(),
            read_only: self.read_only,
        }
    }

    /// What this source is called — **the identity**, and the SQL catalog identifier with it.
    ///
    /// Trimmed and nothing more: a blank name is refused by [`blocker`](Self::blocker), never
    /// filled in from an address behind the user.
    pub fn named(&self) -> String {
        self.name.trim().to_string()
    }

    /// Why this draft cannot be saved yet, or `None` when it can.
    ///
    /// Two questions, and only two: whether the handle is one a query could write, and whether
    /// every question the form actually **put** has an answer. A key that is not shown is not
    /// asked about — a question nobody put cannot be unanswered — and what a *value* may be is
    /// the kind's own rule, asked by [`connect`](strata_engine::DataSource::connect), which is
    /// the real gate.
    ///
    /// What a **kind** makes of an address, and whether another data source already holds this
    /// name, are the footer's: both need something this value does not have — the registry, and
    /// the project's other rows.
    pub fn blocker(&self) -> Option<String> {
        if let Err(why) = check_catalog(&self.named()) {
            return Some(why);
        }
        self.settings
            .iter()
            .filter(|declared| declared.required && self.shows(declared))
            .find(|declared| match declared.field {
                Field::Secret => !self.secrets.contains_key(declared.key),
                _ => self.value(declared.key).trim().is_empty(),
            })
            .map(|declared| format!("This data source has no {}.", noun(declared)))
    }
}

#[cfg(test)]
mod tests {
    use strata_engine::SourceMode;

    use super::*;

    /// A declaration exercising every facet a form is drawn from — an address, two groups, a
    /// default, a conditional key and a required one — and named after nothing shipped, because
    /// the form is not allowed to know a shipped kind either.
    const TEST_SETTINGS: &[SourceSetting] = &[
        SourceSetting {
            key: "address",
            label: "ADDRESS",
            field: Field::Text,
            group: Some("CONNECTION"),
            required: true,
            default: None,
            when: None,
            hint: Some("Where the test source is"),
            placeholder: None,
        },
        SourceSetting {
            key: "user",
            label: "USER",
            field: Field::Text,
            group: Some("CONNECTION"),
            required: true,
            default: None,
            when: None,
            hint: Some("The role to log in as"),
            placeholder: None,
        },
        SourceSetting {
            key: "password",
            label: "PASSWORD",
            field: Field::Secret,
            group: Some("CONNECTION"),
            required: false,
            default: None,
            when: None,
            hint: None,
            placeholder: None,
        },
        SourceSetting {
            key: "mode",
            label: "MODE",
            field: Field::Choice(&["off", "on"]),
            group: Some("SECURITY"),
            required: false,
            default: Some("off"),
            when: None,
            hint: None,
            placeholder: None,
        },
        SourceSetting {
            key: "certificate",
            label: "ROOT CERTIFICATE",
            field: Field::Path,
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

    /// A second kind, taking one setting the first does not.
    const OTHER_SETTINGS: &[SourceSetting] = &[SourceSetting {
        key: "prefix",
        label: "PREFIX",
        field: Field::Text,
        group: None,
        required: true,
        default: None,
        when: None,
        hint: None,
        placeholder: None,
    }];

    fn info(settings: &'static [SourceSetting]) -> SourceInfo {
        SourceInfo {
            kind: "test",
            label: "Test source",
            badge: "TST",
            mode: SourceMode::Catalog,
            settings,
            writable: true,
            unique: &[],
            scheme: None,
        }
    }

    fn source_draft() -> SourceDraft {
        SourceDraft {
            kind: "test".into(),
            settings: TEST_SETTINGS,
            name: "warehouse".into(),
            config: [
                ("address".to_string(), "db.internal:5432/analytics".into()),
                ("user".to_string(), "reader".to_string()),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        }
    }

    /// The round trip an edit is: open on a def, touch nothing, Save writes back what was there.
    #[test]
    fn a_def_survives_the_draft_untouched() {
        let def = SourceDef {
            name: "warehouse".into(),
            kind: "test".into(),
            config: [
                ("user", "reader"),
                ("mode", "on"),
                ("certificate", "/c.pem"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
            secrets: Secrets::Filed(BTreeMap::from([(
                "password".to_string(),
                SecretRef::mint(),
            )])),
            schemas: vec!["public".into(), "analytics".into()],
            read_only: false,
        };
        assert_eq!(SourceDraft::of(&def, &[info(TEST_SETTINGS)]).def(), def);
    }

    /// **The address is an ordinary setting.** No routing, no typed field, no normalising: it is
    /// declared like every other key and lands in `config` beside them, which is what "a source is
    /// its kind, its name and its properties" means.
    ///
    /// What a value may *be* — a scheme pasted in front of it, say — is the kind's own rule,
    /// refused by its `check_address` and never trimmed off behind the user here.
    #[test]
    fn the_address_is_an_ordinary_setting() {
        let mut draft = source_draft();
        assert_eq!(draft.value("address"), "db.internal:5432/analytics");

        draft.set("address", "postgres://other:5433/sales".into());
        assert_eq!(
            draft.value("address"),
            "postgres://other:5433/sales",
            "kept verbatim; the kind is what refuses it"
        );
        assert_eq!(
            draft.def().config.get("address").map(String::as_str),
            Some("postgres://other:5433/sales"),
            "and it lands in the settings map like any other key"
        );
    }

    /// **A new data source opens on the first source the engine serves**, because the picker is a
    /// list of registrants: "none chosen" is a state only a build with no sources can reach.
    #[test]
    fn a_new_draft_adopts_the_first_registrant() {
        let draft = SourceDraft::new(&[info(TEST_SETTINGS)]);
        assert_eq!(draft.kind, "test");
        assert_eq!(draft.settings, TEST_SETTINGS);

        let bare = SourceDraft::new(&[]);
        assert!(bare.kind.is_empty(), "nothing to offer, nothing set");
        assert!(bare.settings.is_empty());
    }

    /// **A def naming a kind nothing is registered for opens with no rows** rather than with the
    /// last kind's — and it keeps what it holds, because nothing here writes a config it cannot
    /// draw.
    #[test]
    fn a_def_whose_kind_is_not_registered_opens_with_no_declaration() {
        let def = SourceDef {
            config: [("address".to_string(), "somewhere".into())]
                .into_iter()
                .collect(),
            name: "mystery".into(),
            kind: "mongo".into(),
            ..Default::default()
        };
        let draft = SourceDraft::of(&def, &[info(TEST_SETTINGS)]);
        assert_eq!(draft.kind, "mongo");
        assert!(draft.settings.is_empty(), "nothing declares its rows");
    }

    /// **The form writes what the kind declares and nothing else.** A value typed under one kind
    /// survives a trip through the picker — that is the whole reason the draft holds a map rather
    /// than a def — while the def in play carries only the keys the kind in play takes. The
    /// address is every kind's, so it is the one value a trip cannot strand.
    #[test]
    fn only_the_declared_keys_reach_the_def() {
        let mut draft = source_draft();
        draft.set("certificate", "/certs/rds.pem".into());

        let def = draft.def();
        assert_eq!(def.config.get("user").map(String::as_str), Some("reader"));
        assert_eq!(
            def.config.get("mode").map(String::as_str),
            Some("off"),
            "a key the box never touched is written as the key declares it"
        );

        draft.adopt(&info(OTHER_SETTINGS));
        let moved = draft.def();
        assert!(
            moved.config.is_empty(),
            "this kind takes none of them: {:?}",
            moved.config
        );

        draft.adopt(&info(TEST_SETTINGS));
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
            Some("This data source has no user.".into())
        );

        let mut addressless = good;
        addressless.set("address", String::new());
        assert_eq!(
            addressless.blocker(),
            Some("This data source has no address.".into())
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
        let cert = TEST_SETTINGS
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
            Some("This data source has no root certificate.".into()),
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
        let def = draft.def();
        assert_eq!(
            def.config.get("certificate").map(String::as_str),
            Some("/certs/rds.pem"),
            "and what a mode reads is the source's business, not a reason to discard the path"
        );
    }

    /// **A blank name box is not a nameless data source**: the address mints one, which is what
    /// **A blank name is refused, not filled in.** A source is what the user called it, so there
    /// is no address to mint one from and no silent name to be surprised by later.
    #[test]
    fn a_blank_name_is_refused() {
        let mut draft = source_draft();
        draft.name = String::new();
        assert_eq!(draft.named(), "");
        assert!(draft.blocker().unwrap().contains("no catalog name"));

        draft.name = " depot ".into();
        assert_eq!(draft.named(), "depot", "and a typed one is trimmed");
        assert_eq!(draft.blocker(), None);
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
        draft.set("address", "  db.internal:5432/analytics  ".into());
        draft.name = " warehouse ".into();
        draft.set("user", " reader ".into());
        draft.secrets.insert("password".into(), SecretRef::mint());
        draft.read_only = false;

        let def = draft.def();
        assert_eq!(def.setting("address"), "db.internal:5432/analytics");
        assert_eq!(def.named(), "warehouse");
        let source = def;
        assert_eq!(
            source.config.get("user").map(String::as_str),
            Some("reader")
        );
        assert!(source.secrets.expects("password"), "carried, never trimmed");
        assert!(!source.read_only, "and so is the write opt-in");
    }

    /// **A secret row is about *this machine*, not about the def.** A committed def can only say
    /// a secret is expected; conflating that with "one is stored" tells a colleague opening a
    /// shared project that theirs is already here.
    #[test]
    fn a_secret_row_reports_this_machine_rather_than_the_def() {
        use SecretProbe as P;

        let row = |expected, typed, removed, probe: &P| {
            SecretRow::of(true, expected, typed, removed, probe)
        };

        assert_eq!(
            row(false, false, false, &P::Absent),
            SecretRow::Unused { forgetting: false }
        );
        assert_eq!(row(true, false, false, &P::Stored), SecretRow::Stored);
        assert_eq!(
            row(true, false, false, &P::Absent),
            SecretRow::Missing,
            "required, expected, and this machine has none"
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
        let key = TEST_SETTINGS
            .iter()
            .find(|declared| declared.key == "password")
            .expect("the declaration");
        assert_eq!(noun(key), "password");
        assert!(SecretRow::Typed.note(&noun(key)).contains("This password"));

        let other = SourceSetting {
            label: "SECRET ACCESS KEY",
            ..*key
        };
        assert!(SecretRow::Missing
            .note(&noun(&other))
            .contains("needs a secret access key"));
    }

    /// **Removing a secret from this machine is not saying the data source has none.** The
    /// expectation stands, so a colleague keeps their own secret.
    #[test]
    fn removing_a_secret_locally_is_not_declaring_the_source_has_none() {
        let removing = SecretRow::of(true, true, false, true, &SecretProbe::Stored);
        assert_eq!(removing, SecretRow::Removing);
        assert!(
            removing
                .note("password")
                .contains("other machines keep theirs"),
            "{}",
            removing.note("password")
        );

        let unused = SecretRow::of(true, false, false, true, &SecretProbe::Stored);
        assert_eq!(unused, SecretRow::Unused { forgetting: true });
        assert!(
            unused.note("password").contains("without a password"),
            "{}",
            unused.note("password")
        );
    }

    /// **An empty box is an answer wherever the kind does not require one.**
    ///
    /// The declaration already said so — Postgres declares `required: false` on its password —
    /// and the renderer used to ignore it: absence read as [`Missing`](SecretRow::Missing), a
    /// demand, escapable only through a *This data source uses no password* control that looked
    /// like a sentence. The control is gone; `required` decides.
    #[test]
    fn an_optional_secret_is_answered_by_leaving_the_box_empty() {
        use SecretProbe as P;

        let optional = SecretRow::of(false, true, false, false, &P::Absent);
        assert_eq!(optional, SecretRow::Optional);
        assert!(
            !optional.note("password").contains("Enter it"),
            "an optional secret asks for nothing: {}",
            optional.note("password")
        );
        assert!(
            optional.keeps_expectation(),
            "and an absence on this machine never edits a shared def"
        );

        assert_eq!(
            SecretRow::of(true, true, false, false, &P::Absent),
            SecretRow::Missing,
            "a key the kind requires still asks for one"
        );
    }

    /// **An empty box never drops a secret this machine is holding**, whatever the kind requires.
    ///
    /// A stored secret is not rendered, so empty is its resting state: reading that as "there is
    /// none" would forget every password on every Save.
    #[test]
    fn an_empty_box_over_a_stored_secret_keeps_it() {
        use SecretProbe as P;

        for probe in [P::Stored, P::Asking, P::Refused("locked".into())] {
            let row = SecretRow::of(false, true, false, false, &probe);
            assert!(
                row.keeps_expectation(),
                "{row:?}: nothing here established that there is no secret"
            );
        }
    }

    /// **Remove from this machine is offered where there is something to remove**, and nowhere
    /// else.
    #[test]
    fn the_secret_press_is_offered_where_it_does_something() {
        assert!(SecretRow::Stored.offers_removal());
        for row in [
            SecretRow::Typed,
            SecretRow::Missing,
            SecretRow::Optional,
            SecretRow::Removing,
            SecretRow::Asking,
            SecretRow::Unused { forgetting: false },
        ] {
            assert!(!row.offers_removal(), "{row:?}: nothing here to remove");
        }
    }
}
