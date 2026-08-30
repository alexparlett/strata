//! **Data sources** — the persisted description of one source a project reads from: an object
//! store, or a server a registered kind speaks to. Exactly what `.strata/project.json` stores,
//! like the catalog defs beside it. Spec: `docs/CONNECTIONS_SPEC.md`.
//!
//! **One flat def, and everything about it is a property.** [`SourceDef`] is a `kind`, the `name`
//! the user gave it, and the settings that kind declares — the address among them, under the
//! kind's own key. A source the engine gains needs no change here, none is more first-class than
//! another, and nothing in this crate knows what any particular setting means.
//!
//! **The name is the identity, and the user writes it.** It is not derived from an address or
//! from anything else, and two sources may describe the very same endpoint and differ only by
//! name — four servers behind one SSM tunnel differ in credentials and in nothing else. Whether
//! *that* is a mistake is the kind's own rule (`SourceKind::UNIQUE`, engine-side, where the
//! registry is): an object store says two of its sources may not share a bucket, and a server
//! says nothing at all.
//!
//! The rule the whole feature is built around: **no part of this module holds a secret value.** A
//! source carries non-secret settings plus, where credentials are needed, a *reference* to where
//! they live — a named `~/.aws` profile, a key **file path**, or the bare expectation that this
//! machine's keystore holds one ([`SourceDef::secrets`]). Nothing here has to be gitignored,
//! which is why the def rides the committed `project.json`.
//!
//! The keystore slot is **derived** from the source's own name and the key it is for, so the
//! committed def carries no machine-local id and two colleagues' keystores never fight over it
//! through git.

use std::collections::{BTreeMap, BTreeSet};

use crate::secret::SecretRef;
use serde::{Deserialize, Serialize};

/// One project-scoped source: which kind serves it, what it is called, and how that kind was
/// configured.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct SourceDef {
    /// **Required.** Which kind serves this source — the registry key, and the prefix of the
    /// keystore family each of its secrets is filed under. A def whose kind nothing answers to
    /// settles failed, naming the fix.
    pub kind: String,
    /// **The identity**: what the user called it. The key the project's rows are held under, what
    /// a table def points at, what a query writes as a catalog prefix, and what the keystore slot
    /// derives from — one field, so nothing can spell it two ways.
    ///
    /// Never derived from anything. Two sources may hold identical settings and differ only here.
    pub name: String,
    /// The settings the kind declares, by the keys it documents — **the address among them**.
    /// Outside this crate's vocabulary on purpose: what a source is configured by is the source's
    /// own business, and this crate has no registry to ask.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, String>,
    /// Which of the kind's secret-typed keys this source has a value for, and **the keystore slot
    /// each is filed under** — never a value. The values live in this machine's keystore, or
    /// arrive through the kind's own environment convention, so a colleague pulling the project
    /// gets "no entry on this machine, here is the fix" rather than silence.
    ///
    /// The slot is recorded rather than inferred, and that is the point: see [`Secrets`].
    #[serde(default, skip_serializing_if = "Secrets::is_empty")]
    pub secrets: Secrets,
    /// The namespaces this source **shows**: `DataGrip`'s "N of M schemas" choice.
    ///
    /// Display only, never a filter the engine applies — registration exposes every namespace the
    /// source can see (the providers are lazy, so that costs nothing), and a query naming one that
    /// is not enabled still resolves and runs. This scopes the data-sources tree and completion:
    /// "what am I working with", not "what may I read".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub schemas: Vec<String>,
    /// Whether Strata refuses to **change** what this source holds: `INSERT` into one of its
    /// relations, and `CREATE TABLE … AS SELECT` making one.
    ///
    /// **Default `true`.** The gate is the def rather than a machine-local preference because a
    /// source is committed and shared — a colleague pulling the project gets the same answer about
    /// the same server. **Strata's own policy**, which is why it is not one of the kind's
    /// settings: a kind declares what it takes, not what we allow.
    pub read_only: bool,
}

/// Hand-written for `read_only` alone: a derived `false` would make a def that omits the field
/// writable.
impl Default for SourceDef {
    fn default() -> Self {
        Self {
            kind: String::new(),
            name: String::new(),
            config: BTreeMap::new(),
            secrets: Secrets::default(),
            schemas: Vec::new(),
            read_only: true,
        }
    }
}

/// Where each of a source's secrets is filed.
///
/// The slot is recorded rather than derived from the def. Deriving it took it from things the
/// user can change while the keystore entry it addresses sits on machines no edit reaches, so a
/// rename stranded every colleague's secret; a ref written once when the secret is first filed
/// survives a rename and a change of kind, and is never rewritten by the colleague who enters
/// their own value under it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Secrets {
    /// A key, and the slot it is filed under. What every save writes.
    Filed(BTreeMap<String, SecretRef>),
    /// A def written before the slot was recorded: keys only, each still addressing the slot the
    /// old derivation gives. Read on load, adopted once, and never written again.
    Derived(BTreeSet<String>),
}

impl Default for Secrets {
    fn default() -> Self {
        Secrets::Filed(BTreeMap::new())
    }
}

impl Secrets {
    /// Whether this source was saved with no secrets at all.
    pub fn is_empty(&self) -> bool {
        match self {
            Secrets::Filed(filed) => filed.is_empty(),
            Secrets::Derived(keys) => keys.is_empty(),
        }
    }

    /// The keys a secret is expected for, in order.
    pub fn keys(&self) -> Vec<&String> {
        match self {
            Secrets::Filed(filed) => filed.keys().collect(),
            Secrets::Derived(keys) => keys.iter().collect(),
        }
    }

    /// Whether a secret is expected for `key`.
    pub fn expects(&self, key: &str) -> bool {
        match self {
            Secrets::Filed(filed) => filed.contains_key(key),
            Secrets::Derived(keys) => keys.contains(key),
        }
    }

    /// Whether every slot is recorded — false for a def still on the old derivation, which
    /// [`SourceDef::adopt_secret_slots`] is what settles.
    pub fn is_filed(&self) -> bool {
        matches!(self, Secrets::Filed(_))
    }
}

impl SourceDef {
    /// The keystore slot `key`'s secret is filed under, or `None` if this source expects none.
    ///
    /// The one place a slot is resolved, so nothing else has to know whether this def records its
    /// slots or is still on the old derivation.
    pub fn secret_slot(&self, key: &str) -> Option<SecretRef> {
        match &self.secrets {
            Secrets::Filed(filed) => filed.get(key).cloned(),
            Secrets::Derived(keys) => keys
                .contains(key)
                .then(|| SecretRef::derived(&self.secret_family(key), &self.named())),
        }
    }

    /// The family half of the old derivation — `"{kind}-{key}"`. Only [`secret_slot`] and the
    /// adoption need it; it is not an identity anything else should compose.
    ///
    /// [`secret_slot`]: Self::secret_slot
    fn secret_family(&self, key: &str) -> String {
        format!("{}-{key}", self.kind.trim())
    }

    /// Record a slot for every secret this def expects, answering the moves an adopting caller
    /// owes the keystore as `(old, new)` pairs.
    ///
    /// **Load-time, once.** A def already recording its slots answers nothing and is left alone.
    /// One written before them mints a ref per key and reports the old derived slot beside it, so
    /// the caller can move whatever this machine has under it — and a machine holding nothing
    /// simply moves nothing, which is the ordinary case for a colleague.
    pub fn adopt_secret_slots(&mut self) -> Vec<(SecretRef, SecretRef)> {
        let Secrets::Derived(keys) = &self.secrets else {
            return Vec::new();
        };
        let mut moves = Vec::new();
        let mut filed = BTreeMap::new();
        for key in keys.clone() {
            let was = SecretRef::derived(&self.secret_family(&key), &self.named());
            let now = SecretRef::mint();
            moves.push((was, now.clone()));
            filed.insert(key, now);
        }
        self.secrets = Secrets::Filed(filed);
        moves
    }

    /// File `key`'s secret under a freshly minted slot, or answer the one it already has.
    ///
    /// Minting here rather than at save is what makes the ref *write-once*: a def that already
    /// records a slot keeps it, so entering a new password overwrites this machine's entry rather
    /// than moving every machine's.
    pub fn secret_slot_or_mint(&mut self, key: &str) -> SecretRef {
        if let Some(held) = self.secret_slot(key) {
            return held;
        }
        let minted = SecretRef::mint();
        let mut filed = match std::mem::take(&mut self.secrets) {
            Secrets::Filed(filed) => filed,
            Secrets::Derived(keys) => keys
                .into_iter()
                .map(|held| {
                    let slot = SecretRef::derived(
                        &format!("{}-{held}", self.kind.trim()),
                        self.name.trim(),
                    );
                    (held, slot)
                })
                .collect(),
        };
        filed.insert(key.to_string(), minted.clone());
        self.secrets = Secrets::Filed(filed);
        minted
    }

    /// Stop expecting a secret for `key`, answering the slot it was filed under so the caller can
    /// clear this machine's entry.
    pub fn forget_secret(&mut self, key: &str) -> Option<SecretRef> {
        let slot = self.secret_slot(key)?;
        let mut filed = match std::mem::take(&mut self.secrets) {
            Secrets::Filed(filed) => filed,
            Secrets::Derived(keys) => keys
                .into_iter()
                .map(|held| {
                    let at = SecretRef::derived(
                        &format!("{}-{held}", self.kind.trim()),
                        self.name.trim(),
                    );
                    (held, at)
                })
                .collect(),
        };
        filed.remove(key);
        self.secrets = Secrets::Filed(filed);
        Some(slot)
    }

    /// What this source is called, trimmed — the handle every surface addresses it by.
    pub fn named(&self) -> String {
        self.name.trim().to_string()
    }

    /// What `key` is set to, trimmed, or empty where this source says nothing about it.
    ///
    /// The only reader of the settings map in this crate, and it knows no key by name: which keys
    /// exist, and what any of them means, is the kind's declaration.
    pub fn setting(&self, key: &str) -> &str {
        self.config.get(key).map(|v| v.trim()).unwrap_or_default()
    }
}

/// **The name a source registers under** — checked against the project's other sources, so the
/// engine's registration and the editor's blocker cannot disagree.
///
/// `existing` is the sources to fold `candidate` against, **`candidate` excluded by the caller**:
/// the project's stored defs for the editor, and the sources already registered on the session for
/// `strata_engine::sources::connect`. Different sets on purpose — a source that failed to connect
/// reserves nothing, which is why the engine's set is the live one.
///
/// **The name is all this checks.** Whether two sources may describe the same *place* is the
/// kind's own rule and is asked of the registry (`SourceKind::UNIQUE`).
pub fn check_catalog_name(existing: &[SourceDef], candidate: &SourceDef) -> Result<(), String> {
    let name = candidate.named();
    check_catalog(&name)?;
    for other in existing {
        if other.named().eq_ignore_ascii_case(&name) {
            return Err(format!(
                "'{name}' is already the name of another source. Give this one a different name."
            ));
        }
    }
    Ok(())
}

/// Whether `name` is one a data source may register under, on its own terms — the half of
/// [`check_catalog_name`] that needs no other data source.
///
/// A **bare** SQL identifier, narrower than what DataFusion could resolve, because every surface
/// that renders `pg.public.orders` would otherwise have to quote it. Case-folded against the
/// reserved name, because unquoted identifiers are.
pub fn check_catalog(catalog: &str) -> Result<(), String> {
    let name = catalog.trim();
    if name.is_empty() {
        return Err("This data source has no catalog name.".into());
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
            "'{WORKSPACE_CATALOG}' is this project's own catalog. Give this data source a \
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
mod slot_tests {
    use super::*;

    fn pg(name: &str, secrets: Secrets) -> SourceDef {
        SourceDef {
            kind: "postgres".into(),
            name: name.into(),
            secrets,
            ..Default::default()
        }
    }

    /// **A def written before slots were recorded still resolves its secret**, through the old
    /// derivation and nothing else.
    #[test]
    fn a_pre_ref_def_resolves_through_the_old_derivation() {
        let def = pg(
            "pg",
            Secrets::Derived(BTreeSet::from(["password".to_string()])),
        );
        assert_eq!(
            def.secret_slot("password"),
            Some(SecretRef::derived("postgres-password", "pg"))
        );
        assert_eq!(def.secret_slot("token"), None);
        assert!(!def.secrets.is_filed());
    }

    /// **Adoption records a slot per key and says which entry to move onto it**, once.
    ///
    /// The old slot is reported beside the new one because only the caller can move a keystore
    /// entry, and a def already recording its slots answers nothing — so a second open does no
    /// keystore work at all.
    #[test]
    fn adoption_mints_a_slot_and_names_the_entry_to_move() {
        let mut def = pg(
            "pg",
            Secrets::Derived(BTreeSet::from(["password".to_string()])),
        );
        let moves = def.adopt_secret_slots();

        assert_eq!(moves.len(), 1);
        let (was, now) = &moves[0];
        assert_eq!(was, &SecretRef::derived("postgres-password", "pg"));
        assert_eq!(def.secret_slot("password").as_ref(), Some(now));
        assert!(def.secrets.is_filed());

        assert!(
            def.adopt_secret_slots().is_empty(),
            "a def that records its slots has nothing left to adopt"
        );
    }

    /// **A recorded slot survives a rename and a change of kind**, which is the whole point: the
    /// derivation moved with both, and only the machine making the change could move its own
    /// keystore entry to follow.
    #[test]
    fn a_recorded_slot_outlives_the_identity_it_was_minted_under() {
        let mut def = pg("pg", Secrets::default());
        let slot = def.secret_slot_or_mint("password");

        let renamed = SourceDef {
            name: "warehouse".into(),
            ..def.clone()
        };
        assert_eq!(renamed.secret_slot("password"), Some(slot.clone()));

        let rekinded = SourceDef {
            kind: "mysql".into(),
            ..def.clone()
        };
        assert_eq!(rekinded.secret_slot("password"), Some(slot.clone()));

        assert_eq!(
            def.secret_slot_or_mint("password"),
            slot,
            "and minting is write-once, so a second secret overwrites this machine's entry \
             rather than moving every machine's"
        );
    }

    /// **Both shapes read from the file**, and only the recorded one is ever written back.
    #[test]
    fn a_pre_ref_file_still_parses() {
        let old: SourceDef =
            serde_json::from_str(r#"{"kind":"postgres","name":"pg","secrets":["password"]}"#)
                .expect("a pre-ref def");
        assert!(old.secrets.expects("password"));
        assert!(!old.secrets.is_filed());

        let json = serde_json::to_string(&pg("pg", Secrets::default())).expect("serialize");
        assert!(
            !json.contains("secrets"),
            "a source with none writes no key at all: {json}"
        );
    }
}
