//! **Profiling** as a freya-query capability (P3-09): one full scan of one catalog entry,
//! cached by the request that asked for it.
//!
//! ## freya-query *is* the profile cache
//!
//! The Dioxus app hand-rolled this — a `profile` field on the catalog row, a dedup set, a
//! spinner flag. None of that is rebuilt (port plan §4): the cache is the freya-query entry,
//! the dedup is its identity, and the spinner is `query.read().state()`. What the store keeps
//! is the **request** ([`ScanId`] on the row), never the result — the same division the Run
//! trigger makes (`QueryTab::request` holds a spec; the rows live only in the cache).
//!
//! ## A scan is an action, so the key carries a nonce
//!
//! Raw entry identity would be the wrong key in both directions: a settled profile must never
//! re-execute by itself (it is the most expensive thing the app does), and a ↻ re-scan must
//! execute even though nothing about the *name* changed. So the key is `(owner, scan)` where
//! `scan` is minted per request, exactly as [`RunId`](super::RunId) is minted per Run press.
//! Dropping the request is therefore all "invalidate this profile" takes.

use std::time::Duration;

use freya::query::{use_query, Captured, Query, QueryCapability, UseQuery};
use strata_engine::profile::{CatalogProfile, Profiled};
use strata_engine::sql::qualified;
use strata_model::{CatalogKind, RemoteRef};
use uuid::Uuid;

use crate::apps::project::contexts::EngineCtx;

/// One profile request's identity — the cache key that makes a scan an *action*. Fresh per
/// request (a first profile, or a ↻ re-scan); never derived from the entry's name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ScanId(Uuid);

impl ScanId {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// **What a profile is of**, and — because the two are the same question — where its request is
/// kept and how its SQL is written.
///
/// A workspace entry is named and not kinded *for the engine*: tables and views share one
/// namespace, so the name alone resolves it. The `kind` rides along because the **store** does need
/// it — a request lands on the tables channel or the views channel, and `ProjectState` searches one
/// list. A remote relation has no list to search and no row to land on, which is exactly why the
/// two arms exist ([`ProfileActions`](crate::apps::project::views::ProfileActions) reads whichever
/// storage backs the arm it is handed).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum ProfileTarget {
    Workspace {
        kind: CatalogKind,
        name: String,
    },
    Remote {
        /// Whether the server calls it a table or a view. The **same vocabulary** as the workspace
        /// arm, so every surface labels the action by one rule rather than growing a second word
        /// for remote things; the fact behind it is `Relation::is_view`, which DB-02 made the one
        /// place the server's `relkind` letters are read, so the tree and `describe_remote` cannot
        /// disagree about it.
        kind: CatalogKind,
        relation: RemoteRef,
    },
}

impl ProfileTarget {
    /// The name handed to [`Engine::profile`](strata_engine::Engine::profile) — a workspace
    /// entry's own name, or a remote relation's three segments rendered by the case-preserving
    /// renderer, which is the spelling the server resolves.
    ///
    /// **One place**, because it is both what the cache key dispatches with and what a cancel
    /// addresses: two renderings of one relation would be two scans, of which only one could be
    /// stopped.
    pub fn sql_name(&self) -> String {
        match self {
            ProfileTarget::Workspace { name, .. } => name.clone(),
            ProfileTarget::Remote { relation, .. } => qualified([
                relation.connection.as_str(),
                relation.schema.as_str(),
                relation.relation.as_str(),
            ]),
        }
    }

    /// Where this scan's aggregate runs, which is what decides the facts it can compute
    /// ([`Profiled`]).
    pub fn profiled(&self) -> Profiled {
        match self {
            ProfileTarget::Workspace { .. } => Profiled::Workspace,
            ProfileTarget::Remote { .. } => Profiled::Database,
        }
    }

    /// Whether this is a table or a view — what every surface labels the action by.
    pub fn kind(&self) -> CatalogKind {
        match self {
            ProfileTarget::Workspace { kind, .. } | ProfileTarget::Remote { kind, .. } => *kind,
        }
    }

    /// What the entry is called, as a panel or a dialog prints it.
    pub fn label(&self) -> String {
        match self {
            ProfileTarget::Workspace { name, .. } => name.clone(),
            ProfileTarget::Remote { relation, .. } => relation.label(),
        }
    }
}

/// What one scan is of: the entry, and the request that asked for it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ProfileSpec {
    pub target: ProfileTarget,
    pub scan: ScanId,
}

/// The scan capability. The engine handle rides as [`Captured`] — invisible to cache identity.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProfileEntry(pub Captured<EngineCtx>);

impl QueryCapability for ProfileEntry {
    type Ok = CatalogProfile;
    type Err = String;
    type Keys = ProfileSpec;

    async fn run(&self, spec: &ProfileSpec) -> Result<CatalogProfile, String> {
        self.0.profile(spec.target.sql_name()).await
    }
}

/// Subscribe to one entry's scan.
///
/// **One place, because the whole [`Query`] is the cache key** — stale and clean times
/// included. Two call sites that built it differently would be two entries over one table,
/// and the second would scan it again. The inspector's STATISTICS zone and the catalog row's
/// spinner are both watching the same scan, so they both come through here.
///
/// - `stale_time(MAX)` — a settled scan must never re-execute by itself. Only a new request
///   (a fresh [`ScanId`], hence a new key) scans again.
/// - `clean_time(MAX)` — "cached until the entry changes", which is what the confirm promises.
///   Without it the cache would clear five minutes after the last subscriber unmounted, and the
///   next mount would silently re-run a full scan. The cost is that a superseded entry is never
///   swept either: a re-scan leaves its predecessor in the map with nothing pointing at it, for
///   the window's life. Deliberate, and bounded by how many times one person presses ↻ — a
///   `CatalogProfile` is a `BTreeMap` of a few `Stat`s per column, and the alternative is
///   silently re-reading their whole dataset.
pub fn use_profile(
    engine: &EngineCtx,
    target: &ProfileTarget,
    scan: ScanId,
) -> UseQuery<ProfileEntry> {
    use_query(
        Query::new(
            ProfileSpec {
                target: target.clone(),
                scan,
            },
            ProfileEntry(engine.captured()),
        )
        .stale_time(Duration::MAX)
        .clean_time(Duration::MAX),
    )
}

/// The scan, driven headlessly through the capability layer — the same await `use_query`
/// performs, without a window (`block_on` stands in for the UI executor).
#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use strata_engine::TableSpec;
    use strata_model::{SourceFormat, StatKey};

    use super::*;

    fn workspace(name: &str) -> ProfileTarget {
        ProfileTarget::Workspace {
            kind: CatalogKind::Table,
            name: name.to_string(),
        }
    }

    fn remote(relation: &str) -> ProfileTarget {
        ProfileTarget::Remote {
            kind: CatalogKind::Table,
            relation: RemoteRef {
                connection: "pg".into(),
                schema: "public".into(),
                relation: relation.into(),
            },
        }
    }

    /// A scan of a real table reaches the engine and comes back with the facts a footer can't
    /// carry. `regions.csv` is the CSV case, which is the one profiling exists for: the source
    /// reports nothing at all, so every number here was computed.
    #[test]
    fn a_scan_settles_the_facts_the_source_never_reported() {
        let engine = EngineCtx::default();
        block_on(engine.register(TableSpec {
            name: "regions".into(),
            paths: vec![format!(
                "{}/../strata-core/tests/fixtures/loadfix/regions.csv",
                env!("CARGO_MANIFEST_DIR")
            )],
            format: SourceFormat::from_name("csv"),
            partitions: Vec::new(),
            internal: false,
        }))
        .expect("register");

        let scan = ProfileEntry(engine.captured());
        let spec = ProfileSpec {
            target: workspace("regions"),
            scan: ScanId::new(),
        };
        let profile = block_on(scan.run(&spec)).expect("scan");

        assert_eq!(profile.rows, 5);
        assert_eq!(
            profile.cols["region"]
                .iter()
                .find(|s| s.key == StatKey::Distinct)
                .map(|s| s.text.as_str()),
            Some("2"),
            "the fact a CSV can never report for free"
        );
        assert!(
            profile.sql.contains("FROM regions"),
            "and the query that produced it, for view-as-query: {}",
            profile.sql
        );
    }

    /// Every request is its own cache entry, so a ↻ re-scan actually re-scans — and a settled
    /// one is never re-executed by a remount. The nonce is what buys both.
    #[test]
    fn each_request_is_its_own_cache_key() {
        let scan = ScanId::new();
        let first = ProfileSpec {
            target: workspace("regions"),
            scan,
        };
        assert_eq!(
            first,
            ProfileSpec {
                target: workspace("regions"),
                scan
            },
            "the same request is the same key — a remount reads the cache"
        );
        assert_ne!(
            first,
            ProfileSpec {
                target: workspace("regions"),
                scan: ScanId::new()
            },
            "a re-scan is a new request, so it executes"
        );
        assert_ne!(
            first,
            ProfileSpec {
                target: remote("regions"),
                scan
            },
            "a workspace table and a remote relation of the same name are two entries"
        );
    }

    /// **The two renderers, seen from the one place that picks between them.** A workspace entry
    /// hands the engine its own name, which `run_profile` then folds; a remote relation hands over
    /// three segments quoted on their own account, which is the spelling the server resolves — and
    /// what a cancel has to address, since it names the scan by this same string.
    #[test]
    fn a_targets_engine_name_is_its_own_identity_written_down() {
        assert_eq!(workspace("MyTable").sql_name(), "MyTable");
        assert_eq!(remote("orders").sql_name(), "pg.public.orders");
        assert_eq!(remote("Orders").sql_name(), "pg.public.\"Orders\"");
        assert_eq!(remote("orders").label(), "pg.public.orders");
    }
}
