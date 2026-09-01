//! What each registered name reads, and what a data source is therefore holding up.
//!
//! Derived state, rebuilt by the registration pass like everything else here, and answering one
//! question: [`Sources::dependents`](crate::Sources::dependents), which the Forget confirm
//! renders. It is not a second catalog — what a host's row says about a name is the host's.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use crate::ident::fold_ident;

/// What each registered name reads: a table's data source, or a view's scans.
///
/// The [`InternalTables`](crate::InternalTables) shape, with the same limits, and it answers one question —
/// [`Sources::dependents`](crate::Sources::dependents). It is not a second catalog: what a host's row says about a name is
/// the host's, and none of it is here.
///
/// Registration is a reconciliation, so this is too. Every funnel that registers a name notes
/// what it reads and every funnel that takes one out forgets it, and [`sync`](crate::register::sync)
/// prunes to the names its `CatalogSpec` holds. That last step is what keeps a table whose
/// registration **failed** answerable — it is noted from the spec, and no deregistration will
/// ever report it — without its entry outliving the def.
///
/// Bounded by what the last pass established: a def no pass has reached is not here, and a view
/// the engine could not create has no scans to record.
#[derive(Clone, Debug, Default)]
pub struct Dependencies(Arc<Mutex<BTreeMap<String, Scanned>>>);

/// What a data source is holding up — [`Sources::dependents`](crate::Sources::dependents)'s answer.
///
/// Two lists, because a caller counting them counts two different things. Both are alphabetical
/// and name each thing once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dependents {
    /// Workspace tables whose def reads its files through this data source. Always empty for a
    /// data source that registers a **catalog**: no def can name one.
    pub tables: Vec<String>,
    /// The views left invalid — those over [`tables`](Self::tables) for an object store, and
    /// those scanning its catalog for a source.
    pub views: Vec<String>,
}

/// One registered name, in its own spelling, and what it reads.
///
/// The spelling is carried because the map is keyed by [`fold_ident`], names being matched the way
/// SQL matches them, while a caller renders the name as it was written.
#[derive(Clone, Debug)]
pub(crate) struct Scanned {
    pub(crate) name: String,
    pub(crate) scans: Scans,
}

/// What one name reads. Two arms and no third: a saved query registers nothing.
#[derive(Clone, Debug)]
pub(crate) enum Scans {
    /// A table, and the data source its files are read through — `None` over local files.
    Table(Option<String>),
    /// A view, and the two lists [`ViewMeta`] records: workspace scans bare, everything else
    /// qualified whole.
    View {
        tables: Vec<String>,
        remote: Vec<String>,
    },
}

impl Dependencies {
    /// The tables read through the data source called `name`, alphabetically.
    ///
    /// Case-insensitive, because a data source's name is a SQL identifier and
    /// [`Sources::resolve`] answers that way — which is also what decides, one level down,
    /// whether the table registered over that store at all.
    pub(crate) fn over(&self, name: &str) -> Vec<String> {
        self.named(|scans| match scans {
            Scans::Table(Some(held)) => held.eq_ignore_ascii_case(name),
            _ => false,
        })
    }

    /// The views scanning any of `tables`, alphabetically and each named once.
    ///
    /// Flat rather than transitive on purpose, and still complete: DataFusion inlines a view it
    /// reads, so a view over a view records the *base* tables of both.
    pub(crate) fn above(&self, tables: &[String]) -> Vec<String> {
        let wanted: BTreeSet<String> = tables.iter().map(|t| fold_ident(t)).collect();
        self.named(|scans| match scans {
            Scans::View { tables, .. } => tables.iter().any(|t| wanted.contains(&fold_ident(t))),
            Scans::Table(_) => false,
        })
    }

    /// The views scanning through the catalog `catalog`, alphabetically and each named once.
    ///
    /// Matched on the qualified name's **first part**, folded: that part is the catalog, which is
    /// what [`ViewMeta`] keeps its two lists apart for.
    pub(crate) fn reading(&self, catalog: &str) -> Vec<String> {
        let wanted = fold_ident(catalog);
        self.named(|scans| match scans {
            Scans::View { remote, .. } => remote
                .iter()
                .filter_map(|dep| dep.split('.').next())
                .any(|part| fold_ident(part) == wanted),
            Scans::Table(_) => false,
        })
    }

    /// Every held name whose scans `wanted` accepts, in its own spelling, alphabetically.
    fn named(&self, wanted: impl Fn(&Scans) -> bool) -> Vec<String> {
        let held = self.0.lock().unwrap();
        let mut found: Vec<String> = held
            .values()
            .filter(|held| wanted(&held.scans))
            .map(|held| held.name.clone())
            .collect();
        found.sort();
        found
    }

    /// Record what registering `name` established about what it reads.
    pub(crate) fn note(&self, name: &str, scans: Scans) {
        self.0.lock().unwrap().insert(
            fold_ident(name),
            Scanned {
                name: name.to_string(),
                scans,
            },
        );
    }

    /// Forget `name` — every funnel that deregisters one.
    pub(crate) fn forget(&self, name: &str) {
        self.0.lock().unwrap().remove(&fold_ident(name));
    }

    /// Keep only the names `wanted` holds — [`sync`](crate::register::sync)'s reconciliation, and the
    /// only thing that can retire an entry no deregistration will ever report.
    pub(crate) fn retain(&self, wanted: &BTreeSet<String>) {
        self.0
            .lock()
            .unwrap()
            .retain(|held, _| wanted.contains(held));
    }
}
