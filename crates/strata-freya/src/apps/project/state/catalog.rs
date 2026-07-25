//! The catalog's **context signals**: the **inspected column** the right-hand inspector is
//! describing, whether a catalog **scan** is in flight, and the **re-scan request** the sidebar's
//! ↻ raises for the window root's scan driver to serve.
//!
//! All are context signals, not Radio stores (state-arch §8, the `LayoutCtx` / `LogCtx` shape):
//! one small value each, written in one place and read in another across the shell. Neither is on
//! [`ProjectState`](super::ProjectState) — that store is the project's durable defs plus what
//! registration learned, and a transient "what am I looking at" pointer would wake every catalog
//! subscriber on each click.
//!
//! The selection's value is a [`ColRef`] — `{ kind, owner, path }` — because a name alone can't say
//! *which* `city`, the top-level one or the one inside `address`, and the sidebar renders both.

use freya::prelude::{consume_context, use_provide_context, State, WritableUtils};
use strata_model::ColRef;

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

/// Whether a catalog scan is in flight (P3-03) — the registration pass at project open, or a
/// press of the sidebar's ↻. Set by [`claim_scan`] for exactly as long as the winner's
/// [`ScanGuard`] lives; read by the sidebar header, which spins its refresh button and disables
/// it for the duration.
///
/// This is about the *act of scanning*, not about any row — every row already carries its own
/// `Reg::Loading`, and a bool that flips twice per pass has no business waking catalog
/// subscribers. The initial pass claims it too, so ↻ can't start a second scan on top of the first.
pub type CatalogScan = State<bool>;

/// Provide this window's scan flag. Called by `use_init_project`, which owns the scan driver
/// that claims it.
pub fn use_init_catalog_scan() -> CatalogScan {
    use_provide_context(|| State::create(false))
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
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct ScanRequest(pub u64);

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
pub fn claim_scan(mut scan: CatalogScan) -> Option<ScanGuard> {
    if *scan.peek() {
        return None;
    }
    scan.set(true);
    Some(ScanGuard(scan))
}

/// Holds [`CatalogScan`] set for as long as it is alive, and clears it on `Drop` — whether the
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
pub struct ScanGuard(CatalogScan);

impl Drop for ScanGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// This window's scan flag, from context.
pub fn use_catalog_scan() -> CatalogScan {
    consume_context::<CatalogScan>()
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
    fn flag() -> CatalogScan {
        State::create_global(false)
    }

    /// Only one pass at a time: the second caller is told to stay out, and the flag the sidebar
    /// header reads says a scan is running.
    #[test]
    fn one_claim_at_a_time() {
        let scan = flag();
        let guard = claim_scan(scan).expect("the first claim wins");
        assert!(*scan.peek(), "the header sees a scan in flight");
        assert!(claim_scan(scan).is_none(), "a second claim can't get in");

        drop(guard);

        assert!(!*scan.peek());
        assert!(claim_scan(scan).is_some(), "the next scan can claim it");
    }

    /// **The D8 regression.** A pass that is *cancelled* rather than finished must still release
    /// the flag. Freya cancels a task by dropping its future, so the guard the future owns is
    /// dropped without the pass's body ever reaching its end — which is exactly what a
    /// `set(false)` written at the end of the pass would miss, latching ↻ disabled and every
    /// catalog row in `Reg::Loading` for the window's whole life.
    #[test]
    fn a_cancelled_pass_still_releases_the_claim() {
        let scan = flag();
        let guard = claim_scan(scan).expect("the first claim wins");
        // The shape `scan_catalog` has: the guard rides in the future's own state, and the
        // work never gets to run.
        let pass = async move {
            let _scan = guard;
            std::future::pending::<()>().await;
        };

        drop(pass);

        assert!(!*scan.peek(), "the claim is released by the drop");
        assert!(claim_scan(scan).is_some(), "…so ↻ works again");
    }
}
