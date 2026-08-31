//! The catalog's **context signals**: the **inspected column** the right-hand inspector is
//! describing, the **profile requests** on relations that have no catalog row to keep one on,
//! **what the engine last answered** for each def, whether a catalog **scan** is in flight, and
//! the **re-scan request** the sidebar's ↻ raises for the window root's scan driver to serve.
//!
//! All are context signals, not Radio stores (state-arch §8, the `LayoutCtx` / `LogCtx` shape):
//! one small value each, written in one place and read in another across the shell. Neither is on
//! [`ProjectState`](super::ProjectState) — that store is the project's durable defs plus what
//! registration learned, and a transient "what am I looking at" pointer would wake every catalog
//! subscriber on each click.
//!
//! The selection's value is a [`ColRef`] — `{ owner, path }`, where the owner is a workspace table
//! or view **or** a relation inside a data source's catalog — because a name alone can't
//! say *which* `city`, the top-level one or the one inside `address`, and the sidebar renders both.

use std::collections::{BTreeMap, BTreeSet};

use freya::prelude::{consume_context, use_provide_context, use_side_effect, State, WritableUtils};
use freya::radio::use_radio;
use strata_engine::{CatalogGen, Registrations};
use strata_model::{ColRef, RemoteRef, SourceDef};

use super::{ProjChan, ProjectState};
use crate::apps::project::contexts::EngineCtx;
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
/// A database answers for itself, so there are no defs and no catalog rows under a database
/// data source (DB-02) — which leaves the "an expensive, opt-in result is freya-query keyed by the
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
/// **Invalidation is a reconciliation, not an event.** A scan describes a relation as the data
/// source last answered for it, so it survives exactly as long as that data source is connected: a
/// Forget takes its def away, and a whole-catalog ↻ re-connects every data source, rebuilding every
/// provider — so both gestures are covered by the one rule, and a single table's Refresh (which
/// touches no data source) leaves a remote scan alone. Nothing here has to notice *which* gesture
/// happened.
pub fn use_init_remote_scans() -> RemoteScans {
    let scans: RemoteScans = use_provide_context(|| State::create(BTreeMap::new()));
    let project = use_radio::<ProjectState, ProjChan>(ProjChan::Sources);
    let registrations = use_registrations();
    use_side_effect(move || {
        let connected = connected_sources(&project.read(), &registrations.read());
        let mut scans = scans;
        if scans
            .peek()
            .keys()
            .any(|relation| !connected.contains(&relation.source))
        {
            scans
                .write()
                .retain(|relation, _| connected.contains(&relation.source));
        }
    });
    scans
}

/// The names of every data source that is currently connected — what a remote relation's
/// [`RemoteRef::source`] is addressed by: the project's own defs, each asked of the engine.
///
/// Every connected one, not only the ones that register a catalog: a name is a name, whether a
/// kind's mode makes relations under it is the registry's answer and this reader has none, and a
/// name that can never key a remote scan prunes nothing. The predicate is "is this still
/// connected", and it answers that exactly.
fn connected_sources(project: &ProjectState, registrations: &Registrations) -> BTreeSet<String> {
    project
        .sources
        .iter()
        .map(SourceDef::named)
        .filter(|name| registrations.sources.is_ready(name))
        .collect()
}

/// This window's remote-scan requests, from context.
pub fn use_remote_scans() -> RemoteScans {
    consume_context::<RemoteScans>()
}

/// The state of the catalog, as **two clocks that move at different moments** — the one a name
/// resolves against, and the one the engine's answers are stamped with.
///
/// *The names clock* is [`generation`](Self::generation), adopted only at a settle. `Scanning` is
/// not merely "busy": a pass applies the catalog **row by row**, so mid-pass it is a real
/// half-applied state — a def that has not registered yet is genuinely not found, and one the
/// pass is about to take out is still there. So this half is a **gate**: while a pass holds it
/// there is no generation to have, nothing validates, and no verdict about a state that never
/// persists is produced rather than produced and retracted. A table being *rebuilt* is not one of
/// those states: `Catalog::register` builds the new provider aside and swaps it in, so a name
/// never stops resolving. (The sidebar header reads the same value to spin and disable its ↻.)
///
/// *The answers clock* is [`answers`](Self::answers), and it moves **per outcome, mid-pass
/// included**. It says only "what the engine has answered has changed", which is a fact about the
/// ledger and not about whether a name resolves, so it is gated on nothing.
/// [`RegistrationsCtx`] derives from it — which is what lets a catalog row settle the moment its
/// own def is answered for, instead of waiting for the last table in the pass.
///
/// Fusing the two is what forced the ledger to be refreshed by hand at three call sites: keyed on
/// the names clock it would have collapsed per-outcome liveness into one update at the end of
/// every pass, and keyed on nothing a fourth registering gesture could silently forget it.
///
/// Both are the **engine's own** [`CatalogGen`], read rather than counted here. Every registry
/// write the engine makes moves it and nothing else does, so a pass that changed nothing leaves
/// every tab's verdict standing, and a change made by another path — a typed statement, a Forget
/// — stales tabs exactly as a pass does. A generation, **not a fingerprint over the rows**,
/// because registration lands row by row: a fingerprint would change N times during one scan and
/// queue N × M dry plans.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CatalogState {
    /// The mount value: **no pass has run at all**, so there is nothing to resolve names against
    /// yet and nothing has been adopted. Distinct from a settled default generation, which is a
    /// project whose pass completed and registered nothing — an empty project resolves names
    /// perfectly well, it just resolves none of them.
    Cold,
    /// A pass is in flight: the answers clock and no names clock, which is the gate.
    Scanning {
        /// The generation of the last engine answer this window adopted.
        answers: CatalogGen,
    },
    /// A pass has completed, or a discrete mutation has landed, and names resolve against
    /// `generation`.
    Settled {
        /// The generation names resolve against.
        generation: CatalogGen,
        /// The generation of the last engine answer this window adopted. Never behind
        /// `generation`: a settle adopts both.
        answers: CatalogGen,
    },
}

impl CatalogState {
    /// The generation to validate against, or `None` when there is nothing to resolve names
    /// against yet — either a pass is in flight, or none has run.
    ///
    /// [`Cold`](Self::Cold) has to be a *claimable* state rather than `Scanning`: [`claim_scan`]
    /// claims from anything but a pass in flight, and seeding one would deadlock the window's one
    /// scan driver at mount and leave every catalog row unanswered forever. So the open-time race
    /// is closed by this answer, not by the initial state — and by value rather than by which side
    /// effect happens to run first.
    pub fn generation(self) -> Option<CatalogGen> {
        match self {
            CatalogState::Cold | CatalogState::Scanning { .. } => None,
            CatalogState::Settled { generation, .. } => Some(generation),
        }
    }

    /// The generation the last engine answer this window adopted was given at — **total across
    /// every variant**, deliberately: an answer lands mid-pass as readily as after one.
    ///
    /// [`Cold`](Self::Cold) answers the seed, which is honest — nothing has been adopted, and the
    /// ledger view is seeded from the engine at mount rather than left empty.
    pub fn answers(self) -> CatalogGen {
        match self {
            CatalogState::Cold => CatalogGen::default(),
            CatalogState::Scanning { answers } | CatalogState::Settled { answers, .. } => answers,
        }
    }

    /// Whether a scan is in flight — the sidebar's spinner / disabled ↻.
    pub fn is_scanning(self) -> bool {
        matches!(self, CatalogState::Scanning { .. })
    }

    /// This state with an engine answer given at `at` adopted: the answers clock always, and the
    /// names clock unless a pass holds the gate.
    ///
    /// **Neither clock goes backwards.** Two gestures can settle out of order — each folds after
    /// its own await — and a window that adopted the older stamp last would describe a catalog the
    /// engine has already moved past. Taking the later of the two is what reading the engine's
    /// *current* generation at every fold used to give for free.
    fn adopting(self, at: CatalogGen) -> CatalogState {
        match self {
            CatalogState::Cold => CatalogState::Settled {
                generation: at,
                answers: at,
            },
            CatalogState::Scanning { answers } => CatalogState::Scanning {
                answers: answers.max(at),
            },
            CatalogState::Settled {
                generation,
                answers,
            } => CatalogState::Settled {
                generation: generation.max(at),
                answers: answers.max(at),
            },
        }
    }
}

/// **This window's view of the engine's registration ledger** — the right-hand side of every
/// catalog row's join, held once so that a walk over the rows costs no engine call.
///
/// The value is the engine's own answer ([`Catalog::registrations`](strata_engine::Catalog)),
/// copied wholesale and never edited: whether a def registered is the engine's decision, and
/// this window renders it. What is held here rather than re-read per surface is *the moment* —
/// one read means every row on screen describes the same instant.
///
/// It is not a mirror of the store's rows. The rows are the defs, which are the store's; this is
/// what happened when the engine was asked to register them, keyed by name and stamped with the
/// generation each answer was given at.
///
/// **Why it has to exist at all**, rather than each surface re-reading the engine on whatever
/// already wakes it: a **data source** has no payload. A table's answer lands `TableMeta` on its
/// row and a view's lands `ViewMeta`, so those surfaces are woken by the store write that carries
/// it — but connecting learns nothing a row could carry, so a source outcome writes nothing and
/// there is no channel to wake the tree. This is that channel, and once it exists it serves the
/// rest, which is also what keeps every row on screen describing one instant.
///
/// **It is derived, not maintained** ([`use_init_registrations`]): the one thing that moves it is
/// [`CatalogState::answers`], so a gesture that makes the engine register something keeps this
/// true by adopting the stamp its answer already carries. Nothing refreshes it by hand, and
/// nothing can forget to.
///
/// A `State` rather than a `Memo` because a surface's tests seed it with answers no engine holds
/// — the ledger a window renders is a value, and a test about how a refusal draws must be able to
/// hand one over.
pub type RegistrationsCtx = State<Registrations>;

/// Provide this window's ledger view, seeded from the engine and kept true from `catalog`. Called
/// by `use_init_project`, which owns the pass that moves the stamp.
///
/// Seeded rather than left empty because a re-rooted window mounts onto an engine that may
/// already hold a catalog, and an empty seed would draw every row as unanswered until the first
/// pass landed.
///
/// A whole read rather than a per-def write, because a def is not the only thing an answer moves:
/// a data source going down takes every table over it with it, and the engine has already worked
/// that out. `set_if_modified`, so a stamp that moved without changing what the engine answers —
/// a `SET`, a created function — wakes nobody joined against this.
///
/// The read is of the engine's ledger **now**, not of the moment `at` names, which is what makes
/// [`CatalogState::adopting`]'s refusal to rewind safe: adopting the later of two out-of-order
/// stamps skips a re-derivation whose answer this read has already taken.
pub fn use_init_registrations(engine: &EngineCtx, catalog: Catalog) -> RegistrationsCtx {
    let seed = engine.clone();
    let ledger: RegistrationsCtx =
        use_provide_context(move || State::create(seed.catalog().registrations()));
    let engine = engine.clone();
    use_side_effect(move || {
        let _ = catalog.read().answers();
        let mut ledger = ledger;
        ledger.set_if_modified(engine.catalog().registrations());
    });
    ledger
}

/// This window's ledger view, from context.
pub fn use_registrations() -> RegistrationsCtx {
    consume_context::<RegistrationsCtx>()
}

/// This window's catalog state. See [`CatalogState`].
pub type Catalog = State<CatalogState>;

/// Provide this window's catalog state. Called by `use_init_project`, which owns the scan driver
/// that claims it.
///
/// Starts [`Cold`](CatalogState::Cold) — claimable, so the project-open pass can take it, and
/// unresolvable, so nothing validates until that pass has actually completed.
pub fn use_init_catalog() -> Catalog {
    use_provide_context(|| State::create(CatalogState::Cold))
}

/// **The engine answered at `at`** — adopt it, so every row joined against the ledger re-derives
/// and (unless a pass holds the gate) every tab's verdict re-derives too.
///
/// The window's **one** adoption funnel. `at` is never read from the engine here: it arrives on
/// the answer being folded — a [`StatementReport`](strata_engine::StatementReport)'s stamp, a
/// registration outcome's, or the generation a direct gesture like
/// [`Sources::disconnect`](strata_engine::Sources::disconnect) answers with. That is what makes
/// forgetting impossible rather than merely unlikely: a fold has the stamp because it has the
/// content, and the engine calls that move the ledger hand one back `must_use`.
///
/// **The two clocks have different rules, which is why this is one call.** The names clock is a
/// no-op while a pass is in flight — that pass adopts on its way out, and adopting twice would
/// re-validate everything twice — while the answers clock moves either way, because a statement
/// that landed mid-pass answered for a def and the row showing it is gated on nothing.
pub fn catalog_settled(mut catalog: Catalog, at: CatalogGen) {
    let adopted = catalog.peek().adopting(at);
    catalog.set(adopted);
}

/// A **request** for a re-scan — the sidebar's ↻ bumps the count, and the window root's scan
/// driver ([`use_init_project`](super::hooks::use_init_project)) runs the pass.
///
/// The button deliberately can't spawn the pass itself. Freya's `spawn` binds a task to the
/// *current scope*, which inside an event handler is the handler's own element — here, deep
/// inside the sidebar subtree. Collapsing the sidebar mid-scan drops that scope and cancels the
/// task, which would abandon every row unanswered with no pass left to answer them. A
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

/// What a scan covers, which is also what decides the engine call it makes
/// (`state::hooks::ScanWork`) and the set of rows that drop to `Loading`.
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

/// Claim the catalog for a scan: `Some(guard)` when this caller won it, `None` when a pass
/// already holds it. It stays [`Scanning`](CatalogState::Scanning) for exactly as long as the
/// returned [`ScanGuard`] lives.
///
/// **A UI concurrency claim, not a correctness mechanism.** What it buys is one pass per window
/// (two concurrent ones would re-register the same defs and fight over every row) and the
/// Scanning affordance the sidebar header spins on. What it does *not* decide is what a name
/// resolves to: that is the engine's, which builds each provider aside and swaps it in, and mints
/// the generation the release below adopts.
///
/// The claim carries the answers clock across, untouched: a pass's own outcomes move it while the
/// gate is shut, which is what settles each catalog row as its def is answered for.
///
/// Test-and-set in **one** synchronous step, deliberately: the executor is cooperative and
/// single-threaded, so nothing can interleave between the peek and the set — whereas checking
/// here and setting inside the spawned pass leaves the claim open across a poll boundary, and
/// two dispatches both pass the check and scan the same catalog concurrently.
pub fn claim_scan(mut catalog: Catalog, engine: &EngineCtx) -> Option<ScanGuard> {
    let held = *catalog.peek();
    if held.is_scanning() {
        return None;
    }
    catalog.set(CatalogState::Scanning {
        answers: held.answers(),
    });
    Some(ScanGuard {
        catalog,
        engine: engine.clone(),
    })
}

/// Holds the [`Catalog`] in [`CatalogState::Scanning`] for as long as it is alive, and releases
/// it onto the generation the engine is at — whether the pass it belongs to *finished* or was
/// *cancelled*.
///
/// That distinction is the whole point. A write after the pass's last `.await` never runs when
/// Freya drops the task (a scope unmounting, or the window closing), and the claim would then
/// latch for the rest of the window's life: ↻ disabled forever, every catalog row stranded in
/// unanswered. Same bug, same shape and same reasoning as freya-query's own `RunningGuard`.
///
/// The generation is **read at the release**, not carried from the claim, which is what makes a
/// cancelled pass honest: whatever it managed to register before it was dropped is what the
/// engine is holding, and whatever it did not is not stale — it never happened. It is the one
/// place this window reads the engine's clock rather than adopting a stamp handed to it, because
/// there is no answer here to carry one: a drop is not an outcome.
///
/// The guard is moved *into* the scan future, so the release is tied to that future's storage
/// rather than to its completion. Freya drops a scope's tasks before it drops the scope's state
/// storage, and the driver spawns into the same scope that owns this `State` — so the write below
/// always lands on a live signal.
pub struct ScanGuard {
    catalog: Catalog,
    /// Read for its generation on release. The `EngineCtx` is an `Arc`, which is also what keeps
    /// this engine alive long enough to answer.
    engine: EngineCtx,
}

impl Drop for ScanGuard {
    fn drop(&mut self) {
        let at = self.engine.catalog().generation();
        let answers = self.catalog.peek().answers().max(at);
        self.catalog.set(CatalogState::Settled {
            generation: at,
            answers,
        });
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

/// The claim's own behavior, which is **the affordance and nothing else**: who may start a pass,
/// and that the Scanning state always ends — plus the two clocks, whose whole point is that they
/// move at different moments. What a name resolves to, and when a verdict goes stale, are the
/// engine's generation, tested where it is minted (`strata_engine::generation`).
#[cfg(test)]
mod tests {
    use super::*;

    /// A signal without a component around it — `create_global` is the one constructor that
    /// needs no scope, which is what lets the claim be tested as the plain state machine it is.
    fn catalog(state: CatalogState) -> Catalog {
        State::create_global(state)
    }

    /// **The mount value must be claimable.** [`CatalogState::Cold`] is what the window mounts
    /// with, and the project-open registration pass claims from it — seeding `Scanning` instead
    /// deadlocks the window's one scan driver at mount and strands every catalog row unanswered
    /// for the life of the window. Nothing validates against it either, so the open-time race is
    /// closed without breaking the claim.
    #[test]
    fn the_mount_value_is_claimable_but_nothing_validates_against_it() {
        let engine = EngineCtx::default();
        let catalog = catalog(CatalogState::Cold);

        assert!(catalog.peek().generation().is_none(), "no pass has run yet");
        let guard =
            claim_scan(catalog, &engine).expect("the project-open pass must be able to claim");
        drop(guard);
        assert_eq!(
            catalog.peek().generation(),
            Some(engine.catalog().generation()),
            "…and now the window is looking at the catalog the engine holds"
        );
    }

    /// Only one pass at a time: the second caller is told to stay out, and the value the sidebar
    /// header reads says a scan is running.
    #[test]
    fn one_claim_at_a_time() {
        let engine = EngineCtx::default();
        let catalog = catalog(CatalogState::Settled {
            generation: engine.catalog().generation(),
            answers: engine.catalog().generation(),
        });
        let guard = claim_scan(catalog, &engine).expect("the first claim wins");
        assert!(catalog.peek().is_scanning(), "the header sees a scan");
        assert!(
            catalog.peek().generation().is_none(),
            "and nothing validates"
        );
        assert!(
            claim_scan(catalog, &engine).is_none(),
            "a second claim can't get in"
        );

        drop(guard);

        assert!(!catalog.peek().is_scanning(), "released");
        assert!(
            claim_scan(catalog, &engine).is_some(),
            "the next scan can claim it"
        );
    }

    /// A pass releases onto **the engine's** generation, which is what makes every tab's verdict
    /// stale exactly when the catalog moved and not otherwise. Without adopting it, fixing a
    /// source path would leave "table not found" on screen until the user typed.
    #[test]
    fn a_pass_releases_onto_the_generation_the_engine_reached() {
        let engine = EngineCtx::default();
        let catalog = catalog(CatalogState::Cold);
        let guard = claim_scan(catalog, &engine).expect("claim");
        let _ = engine.catalog().deregister("nothing_of_the_sort");

        drop(guard);

        assert_eq!(
            catalog.peek().generation(),
            Some(engine.catalog().generation()),
            "the window is at the number the engine minted, not one it counted"
        );
    }

    /// A discrete catalog mutation — ⌘S creating a view, a drop deregistering a table — adopts
    /// the stamp its answer carried too, because validation resolves against the *engine*, not
    /// the defs. Its names half is a no-op mid-pass: the pass in flight adopts on its way out,
    /// and adopting twice would re-validate everything twice.
    #[test]
    fn a_settled_mutation_adopts_but_a_scanning_one_does_not() {
        let engine = EngineCtx::default();
        let catalog = catalog(CatalogState::Cold);

        catalog_settled(catalog, engine.catalog().deregister("nothing_of_the_sort"));
        let after = engine.catalog().generation();
        assert_eq!(catalog.peek().generation(), Some(after));

        let guard = claim_scan(catalog, &engine).expect("claim");
        catalog_settled(catalog, engine.catalog().deregister("nor_this"));
        assert!(catalog.peek().is_scanning(), "still gated, still no answer");
        drop(guard);
        assert_eq!(
            catalog.peek().generation(),
            Some(engine.catalog().generation()),
            "one adoption, from the pass"
        );
    }

    /// **The answers clock is not gated, and the names clock is.** This is the whole of EA-30: a
    /// pass's per-outcome answers have to reach the ledger while the pass is still running — that
    /// is what settles catalog rows one at a time — while nothing may validate against a catalog
    /// that is half applied.
    #[test]
    fn an_answer_lands_mid_pass_and_a_verdict_does_not() {
        let engine = EngineCtx::default();
        let catalog = catalog(CatalogState::Cold);
        let _guard = claim_scan(catalog, &engine).expect("claim");
        let before = catalog.peek().answers();

        catalog_settled(catalog, engine.catalog().deregister("one"));
        let after_first = catalog.peek().answers();
        catalog_settled(catalog, engine.catalog().deregister("two"));

        assert!(after_first > before, "the first outcome moved the ledger");
        assert!(
            catalog.peek().answers() > after_first,
            "and so did the second, rather than both waiting for the pass to end"
        );
        assert!(
            catalog.peek().generation().is_none(),
            "while nothing resolves against a catalog that is still being applied"
        );
    }

    /// **Neither clock goes backwards.** Two gestures settle from their own tasks, so the older
    /// stamp can be adopted last — and a window that took it would be describing a catalog the
    /// engine has already moved past.
    #[test]
    fn an_out_of_order_settle_never_rewinds_the_window() {
        let engine = EngineCtx::default();
        let first = engine.catalog().deregister("first");
        let second = engine.catalog().deregister("second");
        let catalog = catalog(CatalogState::Cold);

        catalog_settled(catalog, second);
        catalog_settled(catalog, first);

        assert_eq!(catalog.peek().generation(), Some(second));
        assert_eq!(catalog.peek().answers(), second);
    }

    /// **The D8 regression.** A pass that is *cancelled* rather than finished must still release
    /// the claim. Freya cancels a task by dropping its future, so the guard the future owns is
    /// dropped without the pass's body ever reaching its end — which is exactly what a write at
    /// the end of the pass would miss, latching ↻ disabled and every catalog row unanswered for
    /// the window's whole life.
    #[test]
    fn a_cancelled_pass_still_releases_the_claim() {
        let engine = EngineCtx::default();
        let scan = catalog(CatalogState::Cold);
        let guard = claim_scan(scan, &engine).expect("the first claim wins");
        let pass = async move {
            let _scan = guard;
            std::future::pending::<()>().await;
        };

        drop(pass);

        assert!(
            !scan.peek().is_scanning(),
            "the claim is released by the drop"
        );
        assert!(
            claim_scan(scan, &engine).is_some(),
            "…so ↻ works again"
        );
    }
}
