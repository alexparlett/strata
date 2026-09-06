//! Projects statement effects into the window and reports their persistence outcome.

use freya::radio::{use_radio_station, RadioStation};
use strata_engine::{Persistence, StatementReport, StoreEffect};

use crate::apps::project::contexts::EngineCtx;

use super::catalog::{
    catalog_settled, use_catalog, use_catalog_rescan, use_registrations, Catalog, CatalogRescan,
    RegistrationsCtx,
};
use super::hooks::{refresh_table, refresh_table_rows};
use super::log::{log_event, LogLevel};
use super::persist::{persisted, persisted_defs, use_report, ProjectFile, ReportCtx};
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
/// this; the run capability stays the query-driven wrapper over the same body, and there
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

/// Updates the window from a completed statement and returns whether its definition is durable.
pub fn settle(to: Settle, engine: &EngineCtx, report: &StatementReport) -> bool {
    catalog_settled(to.catalog, report.at);
    let landed = match &report.effect {
        None => true,
        Some(effect) => apply(to, engine, effect, &report.persistence),
    };
    let landed = landed
        && match &report.persistence {
            Persistence::Failed(why) => {
                persisted(to.report, ProjectFile::Defs, || Err(why.clone()))
            }
            Persistence::Saved => persisted(to.report, ProjectFile::Defs, || Ok(())),
            Persistence::Caller => true,
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
/// mutation point is the shared mutation closure's, held **once** rather than spelled out per arm, and adopting
/// the catalog generation is [`settle`]'s, before any of this runs. That is the difference between
/// an invariant and four copies of it — an arm added by a later ED task cannot forget either half,
/// because it never writes either half.
///
/// **The one synchronous read of the ledger is here, and what makes it sound is what it asks.**
/// `RegistrationsCtx` derives from a queued effect (see its own doc), so a read taken in the same
/// breath as [`settle`]'s adoption can still answer the moment before — but `views_to_refresh`
/// asks it only which **views** are failing, while a `TableUpserted` answers for the *table*. The
/// entry this fold moves is never one this read consults. An arm that grows a synchronous read of
/// an entry its own effect moves would have to wait for the derivation instead.
fn apply(to: Settle, engine: &EngineCtx, effect: &StoreEffect, persistence: &Persistence) -> bool {
    let mutate = |chan, write: &dyn Fn(&mut ProjectState)| {
        let mut project = to.project;
        let mut p = project.write_channel(chan);
        write(&mut p);
        match persistence {
            Persistence::Caller => persisted_defs(&p, to.report),
            Persistence::Saved | Persistence::Failed(_) => {
                p.acknowledge(effect);
                true
            }
        }
    };
    match effect {
        StoreEffect::TableUpserted { def, meta } => {
            let landed = mutate(ProjChan::Tables, &|p| {
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
        StoreEffect::TableRemoved { name, .. } => mutate(ProjChan::Tables, &|p| {
            p.remove_table(name);
        }),
        StoreEffect::ViewUpserted { def, meta } => mutate(ProjChan::Views, &|p| {
            p.upsert_view(def.clone());
            p.view_registered(&def.name, meta.clone());
        }),
        StoreEffect::ViewRemoved { name } => mutate(ProjChan::Views, &|p| {
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

    /// Applies a report through the window’s real stores.
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
