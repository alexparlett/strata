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

use serde::{Deserialize, Serialize};

/// One project-scoped data source: which kind serves it, what it is called, and how that kind was
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
    /// Which of the kind's secret-typed keys this source has a value for — the **expectation**,
    /// never a reference and never a value. The values live in this machine's keystore, or arrive
    /// through the kind's own environment convention, so a colleague pulling the project gets "no
    /// entry on this machine, here is the fix" rather than silence.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub secrets: BTreeSet<String>,
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
            secrets: BTreeSet::new(),
            schemas: Vec::new(),
            read_only: true,
        }
    }
}

impl SourceDef {
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
