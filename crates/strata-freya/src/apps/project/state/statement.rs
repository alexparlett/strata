//! The **statement settle** (ED-02) — folding one intercepted statement's outcome into the
//! window.
//!
//! An intercepted statement returns a `StoreEffect` rather than leaving its change somewhere to
//! be discovered: the catalog is the `ProjectState` store, never a query, so the engine's answer
//! is *applied* here — the report's own stamp adopted through `catalog_settled`, so every catalog
//! row and every tab's diagnostics re-derive against the catalog the statement left → store upsert
//! on the matching [`ProjChan`] → the def written through the persist funnel at its mutation point
//! → the event log.
//!
//! **The adoption is once, up front, off the report** rather than per arm. A report carries the
//! generation its effect left the catalog at (EA-30), so there is nothing for an arm to remember
//! and nothing a later arm can forget: an effect that moved no generation carries the same number
//! it started at, and adopting it is a no-op.
//!
//! **One fold for every effect**, not one per capability. Each later ED task adds a `StoreEffect`
//! arm and nothing else: no new persist path, no second adoption site, no second place that knows a
//! table row is written on `ProjChan::Tables`.
//!
//! Driven from the tab's request keeper (`views::keeper`) beside history and the log, and for the
//! same reason: the pin observes the settle even while its tab is backgrounded, so a `CREATE
//! TABLE` run in a tab the user has since left still reaches the sidebar and `project.json`. The
//! local `applied` flag is the whole dedup — the pin is keyed by the press's nonce, so there is
//! never a second observer of one settle.
//!
//! **A surface that dispatches its own work folds through the same body**, not a copy of it: the
//! empty-table panel (IT-01) composes a `CREATE TABLE`, calls `Workspace::run` from a press and
//! hands the report to [`settle`] through [`use_settle`]; ⌘S on a view (`editor::actions`) does
//! the same with what `Catalog::create_view` answers. A gesture that ran a statement and stopped
//! there would leave a table the catalog never learns about, and one that folded it itself would
//! be a second body applying one effect.
//!
//! **And the log entry is recorded here, not by [`use_run_logging`](super::log::use_run_logging).**
//! A statement's message claims something durable ("Table 't' created"), and only the fold knows
//! whether the def actually reached `project.json` — the `save_view` lesson: a success row logged
//! over a failed write is the log promising a table the next open loses.

use freya::prelude::{use_consume, use_side_effect, use_state, WritableUtils};
use freya::query::{QueryStateData, UseQuery};
use freya::radio::{use_radio_station, RadioStation};
use strata_engine::{StatementReport, StoreEffect};

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{QueryOutcome, RunQuery};

use super::catalog::{
    catalog_settled, use_catalog, use_catalog_rescan, use_registrations, Catalog, CatalogRescan,
    RegistrationsCtx,
};
use super::hooks::{refresh_table, refresh_table_rows};
use super::log::{log_event, LogLevel};
use super::persist::{persisted_defs, use_report, ReportCtx};
use super::{ProjChan, ProjectState};

/// The window's handles a fold writes through, resolved once at render and passed by value so a
/// press that dispatches its own work can carry them into its task.
#[derive(Clone, Copy)]
pub struct Settle {
    pub project: RadioStation<ProjectState, ProjChan>,
    pub catalog: Catalog,
    /// The window's view of the engine's ledger — read by the fold to ask which views a table
    /// upsert leaves stale. It is not written here: it derives from the stamp
    /// [`catalog_settled`] adopts off the report.
    pub registrations: RegistrationsCtx,
    pub rescan: CatalogRescan,
    /// Both reporting handles — the event log *and* the fault satellite. The log is reached
    /// through here rather than held beside it: `ReportCtx` already carries it, and a second
    /// field resolved from the same context would be one more thing that has to stay the same
    /// handle for a statement's success row and its write failure to land in one place.
    pub report: ReportCtx,
}

/// Gather the fold's handles from the window's stores and context.
///
/// For a surface that has a [`StatementReport`] in hand with **no query behind it** — the
/// empty-table panel (IT-01) dispatches `Workspace::run` from a press rather than from a
/// `QuerySpec`, and then has exactly the same fold to perform. It reaches [`settle`] through
/// this; [`use_statement_settle`] stays the query-driven wrapper over the same body, and there
/// is deliberately no second `apply`, persist path or generation adoption.
pub fn use_settle() -> Settle {
    Settle {
        project: use_radio_station::<ProjectState, ProjChan>(),
        catalog: use_catalog(),
        registrations: use_registrations(),
        rescan: use_catalog_rescan(),
        report: use_report(),
    }
}

/// Fold a press's statement outcome into the window, once, when it settles. Call once per
/// `RequestPin` (`views::keeper`).
pub fn use_statement_settle(query: UseQuery<RunQuery>) {
    let to = use_settle();
    let engine = use_consume::<EngineCtx>();
    let mut applied = use_state(|| false);
    use_side_effect(move || {
        if *applied.peek() {
            return;
        }
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
        let _ = settle(to, &engine, &report);
    });
}

/// Adopt the generation `report` left the catalog at, apply its effect, then say so — in that
/// order, because whether the statement is worth announcing is the write's answer, not the
/// engine's, and because the report's stamp is a fact whether or not the def reached disk.
///
/// The engine rides beside the `Copy` handles rather than on [`Settle`], exactly as `drop_row`
/// takes it: `EngineCtx` is an `Arc` and would cost the struct its `Copy`, for one arm.
///
/// `pub` for the one caller that has a report without a query — see [`use_settle`]. There is no
/// dedup hazard in that: the `applied` flag above guards a *pin's* re-render, and a press that
/// dispatched its own run has no pin.
///
/// **It answers whether the change is durable**, which is the same question [`apply`] answers and
/// for the same reason: a surface that closes itself on success has to be able to tell a create
/// that reached `project.json` from one that did not. The query-driven caller ignores it — the
/// results pane has nothing to close and `persisted_defs` has already reported the cause — but
/// the Configure window's Save does not (`configure::views::footer`).
pub fn settle(to: Settle, engine: &EngineCtx, report: &StatementReport) -> bool {
    catalog_settled(to.catalog, report.at);
    let landed = match &report.effect {
        None => true,
        Some(effect) => apply(to, engine, effect),
    };
    if landed {
        log_event(to.report.log, LogLevel::Ok, report.message.clone());
    }
    landed
}

/// Fold one effect into the stores. Returns whether the change is durable — `false` only when a
/// def mutation could not be written, which [`persisted_defs`] has already reported through the
/// faults funnel.
///
/// The def-mutating arms name a channel and a mutation and nothing else: persisting at the
/// mutation point is [`mutated`]'s, held **once** rather than spelled out per arm, and adopting
/// the catalog generation is [`settle`]'s, before any of this runs. That is the difference between
/// an invariant and four copies of it — an arm added by a later ED task cannot forget either half,
/// because it never writes either half.
fn apply(to: Settle, engine: &EngineCtx, effect: &StoreEffect) -> bool {
    match effect {
        StoreEffect::TableUpserted { def, meta } => {
            let landed = mutated(to, ProjChan::Tables, |p| {
                p.upsert_table(def.clone());
                p.table_registered(&def.name, meta.clone());
            });
            let stale = to
                .project
                .peek()
                .views_to_refresh(&def.name, &to.registrations.peek());
            if !stale.is_empty() {
                refresh_table(to.rescan, def.name.clone());
            }
            landed
        }
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
        StoreEffect::RescanTable { name } => {
            refresh_table_rows(engine.clone(), to.project, name.clone());
            true
        }
        StoreEffect::FunctionsChanged
        | StoreEffect::PreparedChanged
        | StoreEffect::RemoteRelationsChanged => true,
    }
}

/// One def mutation, whole: take the guard on `chan`, apply `write`, and persist at the mutation
/// point. Answers whether the defs actually reached `project.json`.
///
/// The catalog generation is adopted by [`settle`] before any arm runs, and on **either** arm of
/// this write, deliberately — validation resolves against the engine, not the project file, so a
/// mutation whose write failed has moved what every tab's diagnostics should say just as much as
/// one whose write landed (`save_view` settled the same point).
fn mutated(to: Settle, chan: ProjChan, write: impl FnOnce(&mut ProjectState)) -> bool {
    let mut project = to.project;
    let mut p = project.write_channel(chan);
    write(&mut p);
    persisted_defs(&p, to.report)
}

/// Statement-fold tests — the arm that has no def to write and therefore no store mutation to
/// assert on: an `INSERT`'s row-count refresh, which leaves the fold, goes to the engine and
/// comes back on a task.
///
/// Driven over a **real** engine and a real project folder, because that round trip is the whole
/// deliverable: every link either side of it is unit-tested (`Catalog::table_meta` in
/// `strata-engine`, `ProjectState::table_reread` next door), and what nothing else covers is that
/// the arm dispatches at all and its spawned task lands.
#[cfg(test)]
mod tests {
    use std::thread::sleep;
    use std::time::Duration;

    use freya::prelude::*;
    use freya::radio::RadioStation;
    use freya_testing::TestingRunner;
    use futures::executor::block_on;
    use strata_core::project::{save_defs, ProjectDefs};
    use strata_core::theme::load;
    use strata_engine::{Registrations, RunOutcome, RunTag, StoreEffect, TableMeta, WsId};

    use crate::apps::project::state::{CatalogState, Log, PersistFaults, ScanRequest};
    use crate::theme::strata_theme;

    use super::*;

    /// `use_statement_settle`'s body with the query replaced by a report handed in — the fold is
    /// what is under test, and a real `UseQuery` would only add the press that produced one.
    #[derive(PartialEq)]
    struct Fold {
        report: StatementReport,
    }

    impl Component for Fold {
        fn render(&self) -> impl IntoElement {
            let to = Settle {
                project: use_radio_station::<ProjectState, ProjChan>(),
                catalog: use_catalog(),
                registrations: use_registrations(),
                rescan: use_catalog_rescan(),
                report: use_report(),
            };
            let engine = use_consume::<EngineCtx>();
            let report = self.report.clone();
            use_hook(move || {
                let _ = settle(to, &engine, &report);
            });
            rect()
        }
    }

    /// Run one statement on `engine` and take its report.
    fn statement(engine: &EngineCtx, sql: &str) -> StatementReport {
        match block_on(engine.ws(WsId(1)).run(RunTag(1), sql.into(), 10)).expect("ran") {
            RunOutcome::Statement(report) => report,
            RunOutcome::Rows(..) => panic!("{sql} ran as a query"),
        }
    }

    /// **An `INSERT`'s row count reaches the sidebar.** The arm leaves the fold entirely — no def
    /// to write, no channel to mutate — so nothing about the store proves it ran; the row only
    /// moves if `refresh_table_rows` dispatched, its `spawn_forever` was polled, the engine
    /// answered and `table_reread` landed it.
    ///
    /// Before ED-05 this was a scan-pass request, which the driver serialises and a store
    /// assertion could see. It is now a bare engine round trip, which is exactly why it needs a
    /// test that waits for one.
    #[test]
    fn an_inserts_row_count_reaches_the_row() {
        let root = std::env::temp_dir().join(format!("strata-settle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        save_defs(&root, &ProjectDefs::default()).expect("scaffolded");

        let engine = EngineCtx::default();
        engine.set_data_dir(&root);
        let created = statement(
            &engine,
            "CREATE TABLE t AS SELECT * FROM (VALUES (1)) AS v(n)",
        );
        let Some(StoreEffect::TableUpserted { def, .. }) = created.effect.clone() else {
            panic!("{:?}", created.effect);
        };
        let inserted = statement(&engine, "INSERT INTO t VALUES (2)");
        assert_eq!(inserted.count, Some(1), "the statement itself landed");

        let (mut runner, project) = {
            let root = root.clone();
            TestingRunner::new(
                move || {
                    use_init_theme(|| strata_theme(&load("midnight")));
                    let report = use_consume::<State<StatementReport>>();
                    rect()
                        .expanded()
                        .child(Fold {
                            report: report.read().clone(),
                        })
                        .into_element()
                },
                (400., 300.).into(),
                move |r| {
                    r.provide_root_context(|| engine.clone());
                    r.provide_root_context(|| State::create(CatalogState::Cold));
                    r.provide_root_context(|| State::create(Registrations::default()));
                    r.provide_root_context(|| State::create(ScanRequest::default()));
                    r.provide_root_context(|| State::create(Log::default()));
                    r.provide_root_context(|| State::create(PersistFaults::default()));
                    r.provide_root_context(|| State::create(inserted.clone()));
                    r.provide_root_context(|| {
                        let mut p = ProjectState::from_defs(
                            ProjectDefs {
                                tables: vec![def.clone()],
                                ..Default::default()
                            },
                            root.clone(),
                        );
                        p.table_registered(
                            &def.name,
                            TableMeta {
                                columns: Vec::new(),
                                rows: Some(1),
                            },
                        );
                        RadioStation::<ProjectState, ProjChan>::create(p)
                    })
                },
                1.,
            )
        };

        let rows = |p: &RadioStation<ProjectState, ProjChan>| {
            p.peek().tables[0].meta.as_ref().and_then(|m| m.rows)
        };
        assert_eq!(rows(&project), Some(1), "the row before the fold answers");

        for _ in 0..200 {
            runner.sync_and_update();
            if rows(&project) == Some(2) {
                break;
            }
            sleep(Duration::from_millis(10));
        }
        assert_eq!(
            rows(&project),
            Some(2),
            "the appended row reached the sidebar"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
