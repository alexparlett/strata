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
use strata_core::engine::profile::CatalogProfile;
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

/// What one scan is of: the catalog entry, and the request that asked for it.
///
/// The entry is named, not kinded: tables and views share **one** namespace (the engine
/// resolves either from the name alone, and `ProjectState::name_in_use` stops two rows folding
/// together), so a second field would be a second source of truth for the same identity.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ProfileSpec {
    pub owner: String,
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
        self.0.profile(spec.owner.clone()).await
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
pub fn use_profile(engine: &EngineCtx, owner: &str, scan: ScanId) -> UseQuery<ProfileEntry> {
    use_query(
        Query::new(
            ProfileSpec {
                owner: owner.to_string(),
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
    use strata_core::engine::TableSpec;
    use strata_model::{SourceFormat, StatKey};

    use super::*;

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
            owner: "regions".into(),
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
        let owner = "regions".to_string();
        let scan = ScanId::new();
        let first = ProfileSpec {
            owner: owner.clone(),
            scan,
        };
        assert_eq!(
            first,
            ProfileSpec {
                owner: owner.clone(),
                scan
            },
            "the same request is the same key — a remount reads the cache"
        );
        assert_ne!(
            first,
            ProfileSpec {
                owner,
                scan: ScanId::new()
            },
            "a re-scan is a new request, so it executes"
        );
    }
}
