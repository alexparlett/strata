//! The catalog's **context signals**: the **inspected column** the right-hand inspector is
//! describing, the **profile requests** on relations that have no catalog row to keep one on,
//! whether a catalog **scan** is in flight, and the **re-scan request** the sidebar's ↻ raises for
//! the window root's scan driver to serve.
//!
//! All are context signals, not Radio stores (state-arch §8, the `LayoutCtx` / `LogCtx` shape):
//! one small value each, written in one place and read in another across the shell. Neither is on
//! [`ProjectState`](super::ProjectState) — that store is the project's durable defs plus what
//! registration learned, and a transient "what am I looking at" pointer would wake every catalog
//! subscriber on each click.
//!
//! The selection's value is a [`ColRef`] — `{ owner, path }`, where the owner is a workspace table
//! or view **or** a relation inside a database connection's catalog — because a name alone can't
//! say *which* `city`, the top-level one or the one inside `address`, and the sidebar renders both.

use std::collections::{BTreeMap, BTreeSet};

use freya::prelude::{consume_context, use_provide_context, use_side_effect, State, WritableUtils};
use freya::radio::use_radio;
use strata_model::{ColRef, Provider, RemoteRef};

use super::{ProjChan, ProjectState, Reg};
use crate::apps::project::query::ScanId;

/// The selected column, or `None` when nothing is inspected. `State` is `Copy`, so consumers hold
/// it by value.
pub type CatalogSelection = State<Option<ColRef>>;

/// Provide this window's inspected-column slot. Call once in the window root, above the shell.
pub fn use_init_catalog_selection() -> CatalogSelection {
    use_provide_context(|| State::create(None::<ColRef>))
}

/// This window's inspected-column slot, from context.
pub fn use_catalog_selection() -> CatalogSelection {
    consume_context::<CatalogSelection>()
}

/// **The profile scan asked for on each remote relation** — the same request a workspace entry
/// keeps on its own catalog row, for the relations that have no row to keep it on.
///
/// A database answers for itself, so there are no defs and no `Reg` rows under a database
/// connection (DB-02) — which leaves the "an expensive, opt-in result is freya-query keyed by the
/// request; the store holds the request" rule with nowhere to put the request. The rule generalizes
/// rather than being excepted: **whoever owns the surface holds the request**. For a workspace
/// entry that is `ProjectState`; for a remote relation it is the window, here. Nothing is minted
/// into the store, and the numbers still live only in the freya-query entry the [`ScanId`] keys.
///
/// A map on a `State` rather than a store channel because it is not project data and does not
/// persist: it is dropped with the window, exactly like the selection above it.
pub type RemoteScans = State<BTreeMap<RemoteRef, ScanId>>;

/// Provide this window's remote-scan requests, and keep them true. Call once in the window root.
///
/// **Invalidation is a reconciliation, not an event.** A scan describes a relation as the
/// connection last answered for it, so it survives exactly as long as that connection is connected:
/// a Forget takes its row away, and a whole-catalog ↻ drops every connection row to `Loading` before
/// re-connecting ([`ProjectState::reload_connections`]), which rebuilds every provider — so both
/// gestures are covered by the one rule, and a single table's Refresh (which touches no connection)
/// leaves a remote scan alone. Nothing here has to notice *which* gesture happened.
pub fn use_init_remote_scans() -> RemoteScans {
    let scans: RemoteScans = use_provide_context(|| State::create(BTreeMap::new()));
    let project = use_radio::<ProjectState, ProjChan>(ProjChan::Connections);
    use_side_effect(move || {
        let connected = connected_catalogs(&project.read());
        let mut scans = scans;
        if scans
            .peek()
            .keys()
            .any(|relation| !connected.contains(&relation.connection))
        {
            scans
                .write()
                .retain(|relation, _| connected.contains(&relation.connection));
        }
    });
    scans
}

/// The catalog names of every database connection that is currently connected — what a remote
/// relation's [`RemoteRef::connection`] is addressed by, from the defs that decide it.
fn connected_catalogs(project: &ProjectState) -> BTreeSet<String> {
    project
        .connections
        .iter()
        .filter(|row| matches!(row.reg, Reg::Ready(())))
        .filter_map(|row| match &row.def.provider {
            Provider::Postgres(pg) => Some(pg.catalog.trim().to_string()),
            _ => None,
        })
        .collect()
}

/// This window's remote-scan requests, from context.
pub fn use_remote_scans() -> RemoteScans {
    consume_context::<RemoteScans>()
}

/// The state of the catalog **as a thing to resolve against** — one value answering both
/// questions its two readers ask: *can I use it right now*, and *has it changed since I last
/// looked*.
///
/// `Scanning` is not merely "busy". `Engine::register` **deregisters before it re-infers**, so
/// mid-pass `table_exist` is false for every table being rebuilt — a validation pass then would
/// report "not found" for tables sitting right there. So this is a **gate**: while it is
/// `Scanning`, nothing validates, and nothing false is ever produced rather than produced and
/// retracted. (The sidebar header reads the same value to spin and disable its ↻.)
///
/// `Settled(epoch)` carries a counter bumped once per completed pass and once per discrete
/// catalog mutation ([`catalog_settled`]). It is what makes a tab's verdict stale when the user
/// fixes a source path: an **epoch, not a fingerprint over the rows**, because registration
/// lands row by row — a fingerprint would change N times during one scan and queue N × M dry
/// plans, where the epoch is silent mid-pass for free, having nothing to bump until the pass
/// finishes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CatalogState {
    Scanning,
    Settled(u64),
}

impl CatalogState {
    /// The epoch to validate against, or `None` when there is nothing to resolve names against
    /// yet — either a pass is in flight, or **no pass has completed at all**.
    ///
    /// `Settled(0)` is the seed, and it means "registration hasn't run". It has to be a settled
    /// value rather than `Scanning`, because [`claim_scan`] claims *from* settled: seeding
    /// `Scanning` deadlocks the window's one scan driver at mount and leaves every catalog row
    /// in `Reg::Loading` forever. So the open-time race is closed by the **epoch**, not by the
    /// initial state — and by value rather than by which side effect happens to run first.
    pub fn epoch(self) -> Option<u64> {
        match self {
            CatalogState::Scanning => None,
            CatalogState::Settled(0) => None,
            CatalogState::Settled(epoch) => Some(epoch),
        }
    }

    /// Whether a scan is in flight — the sidebar's spinner / disabled ↻.
    pub fn is_scanning(self) -> bool {
        matches!(self, CatalogState::Scanning)
    }
}

/// This window's catalog state. See [`CatalogState`].
pub type Catalog = State<CatalogState>;

/// Provide this window's catalog state. Called by `use_init_project`, which owns the scan driver
/// that claims it.
///
/// Starts at `Settled(0)` — **settled**, so the project-open pass can claim it, and **epoch 0**,
/// so nothing validates until that pass has actually completed. See [`CatalogState::epoch`].
pub fn use_init_catalog() -> Catalog {
    use_provide_context(|| State::create(CatalogState::Settled(0)))
}

/// A catalog mutation that is **not** a scan has landed on the engine — a `CREATE OR REPLACE
/// VIEW` from ⌘S, a drop's deregister. Bump the epoch so every tab's verdict is re-derived
/// against what the engine now holds.
///
/// A no-op while `Scanning`: the pass in flight will bump on its way out, and bumping twice
/// would only re-validate everything twice.
pub fn catalog_settled(mut catalog: Catalog) {
    let CatalogState::Settled(epoch) = *catalog.peek() else {
        return;
    };
    catalog.set(CatalogState::Settled(epoch + 1));
}

/// A **request** for a re-scan — the sidebar's ↻ bumps the count, and the window root's scan
/// driver ([`use_init_project`](super::hooks::use_init_project)) runs the pass.
///
/// The button deliberately can't spawn the pass itself. Freya's `spawn` binds a task to the
/// *current scope*, which inside an event handler is the handler's own element — here, deep
/// inside the sidebar subtree. Collapsing the sidebar mid-scan drops that scope and cancels the
/// task, which would abandon every row in `Reg::Loading` with no pass left to answer them. A
/// counter crosses the scope boundary instead, so the pass always belongs to the window root
/// (which is also where `ProjectState`, the thing it writes, lives).
///
/// A count, not a flag: a request that lands while a pass is in flight is dropped (the button is
/// disabled then anyway), but the count still makes each press a *distinct* value, so two presses
/// either side of a scan can't be folded into one no-op by change detection.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct ScanRequest {
    /// Bumped per request. `0` is the mount value, which is what tells the driver that its first
    /// run *is* the project-open pass rather than a ↻.
    pub seq: u64,
    /// How much of the catalog this request covers.
    pub scope: ScanScope,
}

/// What a scan covers. The pass itself is the same either way — re-register from the defs — so
/// this only decides the **work list**, which is also the set of rows that drop to `Loading`.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub enum ScanScope {
    /// Every def in the catalog: project open, and the sidebar's ↻.
    #[default]
    All,
    /// One table, plus the views a refresh of it would otherwise leave reading the provider it
    /// replaced (P3-06's row Refresh). Every other row keeps the verdict it already has.
    Table(String),
}

/// Ask the window root's driver for a pass over `scope`.
///
/// `peek` then `set`, rather than a mutating `write`: the request is a whole new value, and the
/// read must not subscribe whoever is asking — a menu item or a toolbar button has no business
/// re-rendering when the counter it bumps changes.
pub fn request_scan(mut rescan: CatalogRescan, scope: ScanScope) {
    let seq = rescan.peek().seq + 1;
    rescan.set(ScanRequest { seq, scope });
}

/// This window's re-scan request counter. See [`ScanRequest`].
pub type CatalogRescan = State<ScanRequest>;

/// Provide this window's re-scan request counter. Called by `use_init_project`, which owns the
/// driver that watches it.
pub fn use_init_catalog_rescan() -> CatalogRescan {
    use_provide_context(|| State::create(ScanRequest::default()))
}

/// Claim the flag for a scan: `Some(guard)` when this caller won it, `None` when a pass already
/// holds it. The flag stays set for exactly as long as the returned [`ScanGuard`] lives.
///
/// Test-and-set in **one** synchronous step, deliberately: the executor is cooperative and
/// single-threaded, so nothing can interleave between the peek and the set — whereas checking
/// here and setting inside the spawned pass leaves the flag clear across a poll boundary, and
/// two dispatches both pass the check and scan the same catalog concurrently.
pub fn claim_scan(mut catalog: Catalog) -> Option<ScanGuard> {
    let CatalogState::Settled(epoch) = *catalog.peek() else {
        return None;
    };
    catalog.set(CatalogState::Scanning);
    Some(ScanGuard { catalog, epoch })
}

/// Holds the [`Catalog`] in [`CatalogState::Scanning`] for as long as it is alive, and releases
/// it into the next epoch on `Drop` — whether the
/// pass it belongs to *finished* or was *cancelled*.
///
/// That distinction is the whole point. A `set(false)` after the pass's last `.await` never runs
/// when Freya drops the task (a scope unmounting, or the window closing), and the flag would then
/// latch `true` for the rest of the window's life: ↻ disabled forever, every catalog row stranded
/// in `Reg::Loading`. Same bug, same shape and same reasoning as freya-query's own `RunningGuard`.
///
/// The guard is moved *into* the scan future, so the release is tied to that future's storage
/// rather than to its completion. Freya drops a scope's tasks before it drops the scope's state
/// storage, and the driver spawns into the same scope that owns this `State` — so the write below
/// always lands on a live signal.
pub struct ScanGuard {
    catalog: Catalog,
    /// The epoch this pass claimed at; releasing lands on the next one.
    epoch: u64,
}

impl Drop for ScanGuard {
    fn drop(&mut self) {
        self.catalog.set(CatalogState::Settled(self.epoch + 1));
    }
}

/// This window's catalog state, from context.
pub fn use_catalog() -> Catalog {
    consume_context::<Catalog>()
}

/// This window's re-scan request counter, from context.
pub fn use_catalog_rescan() -> CatalogRescan {
    consume_context::<CatalogRescan>()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A signal without a component around it — `create_global` is the one constructor that
    /// needs no scope, which is what lets the claim be tested as the plain state machine it is.
    fn catalog() -> Catalog {
        State::create_global(CatalogState::Settled(1))
    }

    /// **The seed must be claimable.** `Settled(0)` is what the window mounts with, and the
    /// project-open registration pass claims *from* settled — seeding `Scanning` instead
    /// deadlocks the window's one scan driver at mount and strands every catalog row in
    /// `Reg::Loading` for the life of the window. Nothing validates at epoch 0 either, so the
    /// open-time race is closed without breaking the claim.
    #[test]
    fn the_seed_is_claimable_but_nothing_validates_against_it() {
        let catalog = State::create_global(CatalogState::Settled(0));

        assert!(catalog.peek().epoch().is_none(), "no pass has run yet");
        let guard = claim_scan(catalog).expect("the project-open pass must be able to claim");
        drop(guard);
        assert_eq!(
            catalog.peek().epoch(),
            Some(1),
            "…and now there is a catalog"
        );
    }

    /// Only one pass at a time: the second caller is told to stay out, and the value the sidebar
    /// header reads says a scan is running.
    #[test]
    fn one_claim_at_a_time() {
        let catalog = catalog();
        let guard = claim_scan(catalog).expect("the first claim wins");
        assert!(catalog.peek().is_scanning(), "the header sees a scan");
        assert!(catalog.peek().epoch().is_none(), "and nothing validates");
        assert!(claim_scan(catalog).is_none(), "a second claim can't get in");

        drop(guard);

        assert_eq!(catalog.peek().epoch(), Some(2), "released one epoch on");
        assert!(claim_scan(catalog).is_some(), "the next scan can claim it");
    }

    /// A pass releases into a **new** epoch, which is what makes every tab's verdict stale and
    /// so re-derived against the catalog the pass just built. Without the bump, fixing a source
    /// path would leave "table not found" on screen until the user typed.
    #[test]
    fn each_pass_releases_into_a_new_epoch() {
        let catalog = catalog();
        for expected in 2..=4 {
            drop(claim_scan(catalog).expect("claim"));
            assert_eq!(catalog.peek().epoch(), Some(expected));
        }
    }

    /// A discrete catalog mutation — ⌘S creating a view, a drop deregistering a table — bumps
    /// too, because validation resolves against the *engine*, not the defs. It is a no-op
    /// mid-pass: the pass in flight bumps on its way out, and bumping twice would re-validate
    /// everything twice.
    #[test]
    fn a_settled_mutation_bumps_but_a_scanning_one_does_not() {
        let catalog = catalog();
        catalog_settled(catalog);
        assert_eq!(catalog.peek().epoch(), Some(2));

        let guard = claim_scan(catalog).expect("claim");
        catalog_settled(catalog);
        assert!(catalog.peek().is_scanning(), "still gated, still no epoch");
        drop(guard);
        assert_eq!(catalog.peek().epoch(), Some(3), "one bump, from the pass");
    }

    /// **The D8 regression.** A pass that is *cancelled* rather than finished must still release
    /// the flag. Freya cancels a task by dropping its future, so the guard the future owns is
    /// dropped without the pass's body ever reaching its end — which is exactly what a
    /// `set(false)` written at the end of the pass would miss, latching ↻ disabled and every
    /// catalog row in `Reg::Loading` for the window's whole life.
    #[test]
    fn a_cancelled_pass_still_releases_the_claim() {
        let scan = catalog();
        let guard = claim_scan(scan).expect("the first claim wins");
        let pass = async move {
            let _scan = guard;
            std::future::pending::<()>().await;
        };

        drop(pass);

        assert_eq!(
            scan.peek().epoch(),
            Some(2),
            "the claim is released by the drop, into the next epoch"
        );
        assert!(claim_scan(scan).is_some(), "…so ↻ works again");
    }
}
