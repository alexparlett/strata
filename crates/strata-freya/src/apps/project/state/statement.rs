//! The **statement settle** (ED-02) — folding one intercepted statement's outcome into the
//! window.
//!
//! An intercepted statement returns a `StoreEffect` rather than leaving its change somewhere to
//! be discovered: the catalog is the `ProjectState` store, never a query, so the engine's answer
//! is *applied* here exactly as `save_view` applies `Engine::create_view`'s — store upsert on the
//! matching [`ProjChan`] → the def written through the persist funnel at its mutation point →
//! `catalog_settled`, so every tab's diagnostics re-derive against the catalog the engine now
//! holds → the event log.
//!
//! **One fold for every effect**, not one per capability. Each later ED task adds a `StoreEffect`
//! arm and nothing else: no new persist path, no new epoch bump, no second place that knows a
//! table row is written on `ProjChan::Tables`.
//!
//! Driven from the tab's request keeper (`views::keeper`) beside history and the log, and for the
//! same reason: the pin observes the settle even while its tab is backgrounded, so a `CREATE
//! TABLE` run in a tab the user has since left still reaches the sidebar and `project.json`. The
//! local `applied` flag is the whole dedup — the pin is keyed by the press's nonce, so there is
//! never a second observer of one settle.
//!
//! **And the log entry is recorded here, not by [`use_run_logging`](super::log::use_run_logging).**
//! A statement's message claims something durable ("Table 't' created"), and only the fold knows
//! whether the def actually reached `project.json` — the `save_view` lesson: a success row logged
//! over a failed write is the log promising a table the next open loses.

use freya::prelude::{use_consume, use_side_effect, use_state, WritableUtils};
use freya::query::{QueryStateData, UseQuery};
use freya::radio::{use_radio_station, RadioStation};
use strata_core::engine::{StatementReport, StoreEffect};

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{QueryOutcome, RunQuery};

use super::catalog::{catalog_settled, use_catalog, use_catalog_rescan, Catalog, CatalogRescan};
use super::hooks::{refresh_table, refresh_table_rows};
use super::log::{log_event, LogLevel};
use super::persist::{persisted_defs, use_report, ReportCtx};
use super::{ProjChan, ProjectState};

/// The window's handles a fold writes through — resolved once at render, passed by value, like
/// every other observer here (`save_view` takes the same set as arguments).
#[derive(Clone, Copy)]
struct Settle {
    project: RadioStation<ProjectState, ProjChan>,
    catalog: Catalog,
    rescan: CatalogRescan,
    /// Both reporting handles — the event log *and* the fault satellite. The log is reached
    /// through here rather than held beside it: `ReportCtx` already carries it, and a second
    /// field resolved from the same context would be one more thing that has to stay the same
    /// handle for a statement's success row and its write failure to land in one place.
    report: ReportCtx,
}

/// Fold a press's statement outcome into the window, once, when it settles. Call once per
/// `RequestPin` (`views::keeper`).
pub fn use_statement_settle(query: UseQuery<RunQuery>) {
    let to = Settle {
        project: use_radio_station::<ProjectState, ProjChan>(),
        catalog: use_catalog(),
        rescan: use_catalog_rescan(),
        report: use_report(),
    };
    // Not on `Settle`: an `Arc` handle would cost that struct its `Copy` for the sake of one arm.
    let engine = use_consume::<EngineCtx>();
    let mut applied = use_state(|| false);
    use_side_effect(move || {
        if *applied.peek() {
            return;
        }
        // Cloned out while the query's borrow is held and released before anything writes — the
        // shape `use_history_recording` uses, and here it is load-bearing: the fold takes write
        // guards on stores this component's own render read from.
        let settled = match &*query.read().state() {
            QueryStateData::Settled {
                res: Ok(QueryOutcome::Statement(report)),
                ..
            } => Some(report.clone()),
            _ => None,
        };
        let Some(report) = settled else {
            return;
        };
        applied.set(true);
        settle(to, &engine, &report);
    });
}

/// Apply `report`'s effect, then say so — in that order, because whether the statement is worth
/// announcing is the write's answer, not the engine's.
///
/// The engine rides beside the `Copy` handles rather than on [`Settle`], exactly as `drop_row`
/// takes it: `EngineCtx` is an `Arc` and would cost the struct its `Copy`, for one arm.
fn settle(to: Settle, engine: &EngineCtx, report: &StatementReport) {
    // No effect is not a failure: `SET`, `PREPARE` and `DEALLOCATE` change the session and
    // nothing the catalog holds, so there is nothing to persist and the report stands on its own.
    let landed = match &report.effect {
        None => true,
        Some(effect) => apply(to, engine, effect),
    };
    if landed {
        log_event(to.report.log, LogLevel::Ok, report.message.clone());
    }
}

/// Fold one effect into the stores. Returns whether the change is durable — `false` only when a
/// def mutation could not be written, which [`persisted_defs`] has already reported through the
/// faults funnel.
///
/// The def-mutating arms name a channel and a mutation and nothing else: persisting at the
/// mutation point and bumping the catalog epoch are [`mutated`]'s, held **once** rather than
/// spelled out per arm. That is the difference between an invariant and four copies of it — an
/// arm added by a later ED task cannot forget either half, because it never writes either half.
fn apply(to: Settle, engine: &EngineCtx, effect: &StoreEffect) -> bool {
    match effect {
        // A registered table: the def and the answer land together, so the sidebar row goes
        // straight to `Reg::Ready` rather than flashing `Loading` for a registration that is
        // already done.
        //
        // **The views over it are a second question, and it is the scan driver's.** Re-creating
        // a table does not re-plan the views above it — their plans captured the old provider by
        // `Arc` (D10/D11) — so a `CREATE OR REPLACE TABLE` that changes the schema leaves every
        // view over it executing the old one against the new files, and the user gets a raw
        // Arrow "column types must match schema types" from a view they did not touch. The same
        // request also serves the other direction: a `CREATE TABLE` that finally provides the
        // name a failing view was missing brings that view back, which is exactly what
        // `views_to_refresh` widens to. Asked through `refresh_table` rather than re-derived
        // here — one funnel decides which views a table's arrival disturbs, and the row Refresh
        // already is it.
        StoreEffect::TableUpserted { def, meta } => {
            let landed = mutated(to, ProjChan::Tables, |p| {
                p.upsert_table(def.clone());
                p.table_registered(&def.name, meta.clone());
            });
            // Only when there is something to re-create: the pass would otherwise flip this
            // table's own row back to `Loading` for a registration that just answered, on the
            // ordinary CTAS into a project with no views at all.
            if !to.project.peek().views_to_refresh(&def.name).is_empty() {
                refresh_table(to.rescan, def.name.clone());
            }
            landed
        }
        // `dependents` are named in the report's own sentence and deliberately **not** cascaded:
        // a `ViewTable`'s inlined plan goes on executing until reload, so nothing here is stale
        // yet — and the epoch bump makes every tab's diagnostics re-derive at once, which is the
        // surface that actually tells the user.
        StoreEffect::TableRemoved { name, .. } => mutated(to, ProjChan::Tables, |p| {
            p.remove_table(name);
        }),
        StoreEffect::ViewUpserted { def, meta } => mutated(to, ProjChan::Views, |p| {
            p.upsert_view(def.clone());
            p.view_registered(&def.name, meta.clone());
        }),
        StoreEffect::ViewRemoved { name } => mutated(to, ProjChan::Views, |p| {
            p.remove_view(name);
        }),
        // The data moved, the def did not — and **only** the data. Still read from the files,
        // never added up from what the statement claimed, but read without re-registering: a
        // re-scan replaces the provider, which is the one thing that makes the views above it
        // stale (D10/D11) and the only reason a table Refresh re-creates them. An append cannot
        // change the shape they captured, so `refresh_table_rows` asks the engine for this row's
        // count and lands it, and every view is left alone.
        StoreEffect::RescanTable { name } => {
            refresh_table_rows(engine.clone(), to.project, name.clone());
            true
        }
        // Nothing persists (functions are session-scoped) and no row changes, but names that
        // did not resolve a moment ago now do — and diagnostics resolve against the engine.
        StoreEffect::FunctionsChanged => {
            catalog_settled(to.catalog);
            true
        }
    }
}

/// One def mutation, whole: take the guard on `chan`, apply `write`, persist at the mutation
/// point, and bump the catalog epoch. Answers whether the defs actually reached `project.json`.
///
/// The epoch is bumped on **either** arm, deliberately — validation resolves against the engine,
/// not the project file, so a mutation whose write failed has moved what every tab's diagnostics
/// should say just as much as one whose write landed (`save_view` settled the same point).
fn mutated(to: Settle, chan: ProjChan, write: impl FnOnce(&mut ProjectState)) -> bool {
    let mut project = to.project;
    let persisted = {
        let mut p = project.write_channel(chan);
        write(&mut p);
        persisted_defs(&p, to.report)
    };
    catalog_settled(to.catalog);
    persisted
}
