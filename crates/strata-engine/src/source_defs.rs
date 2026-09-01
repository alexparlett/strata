//! The data sources this engine has been told about — membership, not connectivity.
//!
//! The last def handed to [`Sources::connect`](crate::Sources::connect) for each name, keyed by
//! that name. It answers what this project holds a data source for, whether or not what the def
//! describes went in; what the *session* holds right now is
//! [`Sources::listing`](crate::Sources::listing)'s `live`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use strata_model::SourceDef;

use crate::sources;

/// The data sources this engine has been told about: the last def handed to
/// [`Sources::connect`](crate::Sources::connect) for each name, keyed by that name.
///
/// It answers two questions from the one map — may a typed `CREATE EXTERNAL TABLE` name this
/// bucket, and what does this engine hold a data source for ([`Sources::listing`](crate::Sources::listing)). The def rather
/// than the identity alone is what makes the second answerable without asking the host: an engine
/// told about a data source can say what kind serves it and what it registers, live or not.
///
/// It is not a second copy of the catalog. What a host's row says about a data source — whether it
/// is waiting, the sentence a failure left — is the host's, and nothing here records it.
///
/// **Membership, not connectivity.** [`Sources::connect`](crate::Sources::connect) notes the def whether what it describes
/// went in or not, because a data source that cannot resolve a credential today is still a
/// data source this project has: the def a statement writes is durable and the fix (`aws sso
/// login`, a region typed into the editor, ↻) happens afterwards. Asking DataFusion's object-store
/// registry instead would have answered *no* for exactly those, in a sentence — "not a data source
/// in this project" — that would then be false. (What the *session* holds right now is a different
/// question, and [`Sources::listing`](crate::Sources::listing) answers it as `live`.)
///
/// Rebuilt by the pass, like the origin set: the registration pass's first phase calls `connect` for
/// every def, and [`Sources::disconnect`](crate::Sources::disconnect) — the Forget gesture and the edit that moves a
/// data source's identity — is the one removal.
#[derive(Clone, Debug, Default)]
pub struct SourceDefs(Arc<Mutex<BTreeMap<String, SourceDef>>>);

/// What a data source is *registered* as, for the one question `sync` asks of it: has this def
/// moved where it went on the session?
///
/// `{kind}:{address}` — both halves, because the registration URL is composed from both
/// ([`SourceKind::SCHEME`](crate::SourceKind::SCHEME) plus the address). It is **not** an
/// identity in any other sense: what a data source *is* is its name, which the user writes and
/// nothing derives (`SourceDef::named`).
fn source_identity(def: &SourceDef) -> String {
    format!("{}:{}", def.kind.trim(), def.setting("address"))
}

impl SourceDefs {
    /// The data source `name` addresses, **in the data source's own spelling** — `None` when this
    /// project has none.
    ///
    /// Answering with the stored string rather than a bool is what keeps a def's `data source`
    /// field equal to the name everything else addresses it by: the store's picker, the table
    /// spec's path composition and the Forget confirm all match on that exact string.
    ///
    /// The fallback compares **case-insensitively**, because a data source's name is a SQL
    /// identifier and queries fold one. The exact hit is tried first so the ordinary case costs
    /// one lookup.
    pub fn resolve(&self, name: &str) -> Option<String> {
        let held = self.0.lock().unwrap();
        if held.contains_key(name) {
            return Some(name.to_string());
        }
        held.keys()
            .find(|held| held.eq_ignore_ascii_case(name))
            .cloned()
    }

    /// The def this engine was last handed for the data source called `name`, matched the way
    /// [`resolve`](Self::resolve) matches.
    pub(crate) fn def(&self, name: &str) -> Option<SourceDef> {
        let held = self.0.lock().unwrap();
        held.get(name).cloned().or_else(|| {
            held.iter()
                .find(|(held, _)| held.eq_ignore_ascii_case(name))
                .map(|(_, def)| def.clone())
        })
    }

    /// Every data source this engine has been told about, in name order — what
    /// [`Sources::listing`](crate::Sources::listing) walks.
    ///
    /// **Membership, not liveness**, exactly as the rest of this type is: a data source whose
    /// credentials this machine cannot resolve today is still one the project has, and the
    /// listing says so by answering `live: false` rather than by leaving it out.
    pub(crate) fn all(&self) -> Vec<SourceDef> {
        self.0.lock().unwrap().values().cloned().collect()
    }

    /// The data source whose `(kind, address)` is `identity` — for the one caller that arrives
    /// with a written location rather than with a name: a typed `CREATE EXTERNAL TABLE … LOCATION
    /// 's3://acme-lake/events/'`, which has to be matched against what the project holds.
    pub fn named(&self, identity: &str) -> Option<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|(_, held)| held.named().eq_ignore_ascii_case(identity))
            .map(|(name, _)| name.clone())
    }

    /// The data sources a set of defs describes, for a caller that holds defs rather than a live
    /// engine.
    ///
    /// The registration pass composes its table specs **before** its first phase registers
    /// anything, so at that moment no engine can answer what a table's data source is; the defs in
    /// hand are the only thing that can. Building the same type from them rather than reading the
    /// defs directly is what keeps one lookup rule — including the case-insensitive fallback,
    /// which a hand-rolled `find` over the defs would quietly drop.
    /// The `scheme://authority` the data source called `name` hangs its remote paths off, or
    /// `None` for one that reads no files.
    ///
    /// The registry answers the scheme, because it is the *kind's*; this holds the def that names
    /// the kind. Both halves are needed and neither has the other, which is why the composition
    /// lives here rather than on either.
    pub fn prefix(&self, registrants: &sources::source::Registrants, name: &str) -> Option<String> {
        registrants.prefix(&self.def(name)?)
    }

    /// The data source a written `scheme://authority/…` reads through, by name.
    ///
    /// [`prefix`](Self::prefix) backwards, for the one caller that arrives with a URL rather than
    /// with a data source — a typed `CREATE EXTERNAL TABLE … LOCATION 's3://acme-lake/events/'`,
    /// which has to be matched against the project's own data sources. Matched by **prefix**
    /// rather than by parsing the URL into a kind, because two kinds can share a scheme and only
    /// the project's own defs say which bucket is which.
    pub fn by_prefix(
        &self,
        registrants: &sources::source::Registrants,
        url: &str,
    ) -> Option<String> {
        let url = url.trim_end_matches('/');
        self.all().into_iter().find_map(|def| {
            let prefix = registrants.prefix(&def)?;
            let prefix = prefix.trim_end_matches('/');
            url.eq_ignore_ascii_case(prefix).then(|| def.named())
        })
    }

    /// The connections `defs` describes.
    pub fn of(defs: &[SourceDef]) -> Self {
        let held = Self::default();
        for def in defs {
            held.note(def);
        }
        held
    }

    /// Every data source this engine has been told about, as `(name, identity)` — what
    /// [`sync`](crate::register::sync) diffs a desired set against.
    ///
    /// Both halves, because a def whose bucket or **kind** was edited keeps its name and changes
    /// the URL its object store went in under: a diff by name alone leaves that URL registered
    /// with nothing addressing it.
    ///
    /// The identity is [`source_identity`] — the pair, not the address alone, because the URL is
    /// composed from both and an `s3` bucket re-pointed at `gcs` keeps its address while moving
    /// where it registered.
    pub(crate) fn held(&self) -> Vec<(String, String)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(name, def)| (name.clone(), source_identity(def)))
            .collect()
    }

    /// Whether `def` would register somewhere other than where this engine currently holds it —
    /// the one question [`sync`](crate::register::sync)'s diff asks about a name it is keeping.
    ///
    /// Both sides are computed here, and looked up through [`def`](Self::def) so the match folds
    /// case: a name is a SQL identifier, and comparing the two halves any other way answers
    /// "moved" for a source that has not, which costs every live source a teardown per pass.
    ///
    /// A name this engine holds nothing for has not moved: there is nothing to take back.
    pub(crate) fn moved(&self, def: &SourceDef) -> bool {
        self.def(&def.named())
            .is_some_and(|held| source_identity(&held) != source_identity(def))
    }

    /// Hold `def` under its own spelling, replacing whatever this engine held for that name.
    ///
    /// Case-folded on the way in, so a source renamed `Lake` to `lake` replaces its entry rather
    /// than sitting beside it: two entries for one source are what [`held`](Self::held) reports,
    /// so the diff would carry a phantom name and a forget of one would leave the other
    /// answering.
    pub(crate) fn note(&self, def: &SourceDef) {
        let name = def.named();
        let mut held = self.0.lock().unwrap();
        held.retain(|key, _| !key.eq_ignore_ascii_case(&name));
        held.insert(name, def.clone());
    }

    /// Stop holding the data source called `name`, matched the way [`def`](Self::def) matches it.
    pub(crate) fn forget(&self, name: &str) {
        self.0
            .lock()
            .unwrap()
            .retain(|key, _| !key.eq_ignore_ascii_case(name));
    }
}
