//! The query output surface below the editor. The body is **freya-query off the tab's SQL**
//! (state-arch §6): the pane reads the workbench's Run trigger and derives its state from
//! that query's own lifecycle — no press for this tab → **empty**; `Pending`/`Loading` →
//! **running**; settled rows → **grid**; a settled plan → **explain**; a settled statement
//! report → **statement**; a settled `Err` → **error**. Every state sits over the same **status
//! bar** footer (the results-pane footer, themed by `status_bar`).

use std::rc::Rc;

use freya::prelude::*;
use freya::query::{use_query, QueryStateData};
use freya::radio::use_radio;
use strata_arrow::plan::PlanTab;
use strata_model::{ResultsView, SnapshotId, TabId};

mod cell_view;
mod chart;
mod copy;
mod datagrid;
mod empty;
mod error;
mod explain_plan;
mod find;
mod record_view;
mod running;
mod selection;
mod shape;
mod sort;
mod statement;
mod status_bar;
mod toolbar;
mod value_tree;

use chart::ChartView;
use datagrid::{DataGrid, GridData, PageRead};
use empty::EmptyState;
use error::ErrorState;
use find::{FindState, PageKey};
use running::Running;
use sort::SortState;
use statement::StatementState;
use status_bar::StatusBar;

use crate::apps::export::{ExportLaunch, ExportTarget};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{PageSpec, QueryOutcome, QueryPage, QuerySpec, RunId};
use crate::apps::project::state::{Chan, LogCtx, SessionState};
use crate::apps::project::views::workbench::editor::actions;
use crate::apps::project::views::workbench::results::explain_plan::ExplainPlan;
use crate::apps::project::views::workbench::results::selection::Selection;
use crate::platform::Subtree;
use crate::state::AppCtx;
pub use cell_view::CellViewThemePreference;
pub use chart::ChartThemePreference;
pub use datagrid::DataGridThemePreference;
pub use explain_plan::ExplainPlanThemePreference;
pub use record_view::RecordViewThemePreference;
pub use running::{CancelButtonThemePartial, CancelButtonThemePreference};
pub use shape::{ShapeDialog, ShapeTarget};
pub use status_bar::StatusBarThemePreference;
use status_bar::{Pager, RunInfo};

/// Which of the state bodies the results pane shows — the status bar's coarse view state.
#[derive(PartialEq, Clone, Copy)]
pub enum ResultsState {
    /// No query has produced rows yet.
    Empty,
    /// A query is executing.
    Running,
    /// Rows are available — the grid.
    Grid,
    /// Rows are available and the tab's view mode is Chart (P2-07) — the chart body.
    Chart,
    /// Explain plan is available.
    ExplainPlan,
    /// An intercepted statement ran — a status row, no rows (ED-02).
    Statement,
    /// The last run settled `Err`.
    Error,
}

/// The results pane for one tab. Reads the tab's own Run trigger (`QueryTab::request`, on
/// `Chan::Request(id)` — so keystrokes never wake this pane) and mounts the query-driven
/// body when the tab has one — otherwise the empty state. Revisiting a tab whose request
/// is still current re-serves the settled outcome from the freya-query cache (keyed by the
/// request's [`QuerySpec`]) with zero engine traffic.
#[derive(PartialEq)]
pub struct Results {
    id: TabId,
    running: State<Option<RunId>>,
    key: DiffKey,
}

impl Results {
    pub fn new(id: TabId, running: State<Option<RunId>>) -> Self {
        Self {
            id,
            running,
            key: DiffKey::None,
        }
        .key(id)
    }
}

impl KeyExt for Results {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for Results {
    fn render(&self) -> impl IntoElement {
        use_provide_context(|| State::create(Selection::None));

        let id = self.id;
        let radio = use_radio::<SessionState, Chan>(Chan::Request(id));
        let spec = radio.read().request(id).cloned();

        let el: Element = match spec {
            None => shell(EmptyState.into(), StatusBar::new(ResultsState::Empty)),
            Some(spec) => {
                let run = spec.run;
                ResultsBody {
                    spec,
                    running: self.running,
                    key: DiffKey::None,
                }
                .key(run)
                .into()
            }
        };
        el
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The pane once its tab owns the current press: subscribes `use_query` on the press's
/// [`QuerySpec`] (through [`QuerySpec::query`] — the settings are cache identity) and
/// derives the body from the query state. `stale_time(MAX)` because a Run is an *action* —
/// a settled entry must never re-execute by itself (`SNAPSHOT_SPEC` §6); only a new press
/// (fresh nonce → new key) runs again. The tab's keeper (`views::keeper`) subscribes
/// the same entry for as long as the press stays current, so unmounting this body on a tab
/// switch never starts the entry aging out.
#[derive(PartialEq)]
struct ResultsBody {
    spec: QuerySpec,
    /// The workbench's in-flight mirror — this body (the query's sole subscriber) resolves
    /// it to the press's nonce while Pending/Loading so the toolbar can flip Run→Cancel.
    running: State<Option<RunId>>,
    key: DiffKey,
}

impl KeyExt for ResultsBody {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ResultsBody {
    #[allow(clippy::too_many_lines)]
    fn render(&self) -> impl IntoElement {
        let engine = use_consume::<EngineCtx>();
        let query = use_query(self.spec.query(&engine));

        let run = self.spec.run;
        let mut running = self.running;
        use_side_effect(move || {
            let in_flight = matches!(
                &*query.read().state(),
                QueryStateData::Pending | QueryStateData::Loading { .. }
            );
            let mirrored = *running.peek() == Some(run);
            if in_flight && !mirrored {
                running.set(Some(run));
            } else if !in_flight && mirrored {
                running.set(None);
            }
        });
        use_drop(move || {
            if *running.peek() == Some(run) {
                running.set(None);
            }
        });

        let page = use_state(|| 1usize);
        let run_size = self.spec.page_size;
        let page_size = use_state(move || run_size);
        let plan_tab = use_state(PlanTab::default);

        let ws = self.spec.tab;
        let view_radio = use_radio::<SessionState, Chan>(Chan::View(ws));
        let results_view = view_radio.read().view(ws);

        let find = FindState::use_new();
        let pages = find::use_page_memo();
        let sel = use_consume::<State<Selection>>();
        let sort = SortState::use_new(page, sel);
        use_side_effect(move || {
            let _ = find.query.read();
            let mut sel = sel;
            if *sel.peek() != Selection::None {
                sel.set(Selection::None);
            }
        });

        let (snapshot, sort_key) = match &*query.read().state() {
            QueryStateData::Settled {
                res: Ok(QueryOutcome::Rows(rows)),
                ..
            } => (
                rows.output.snapshot,
                (*sort.by.read()).and_then(|(ci, asc)| {
                    rows.output.columns.get(ci).map(|c| (c.name.clone(), asc))
                }),
            ),
            _ => (None, None),
        };
        let cur_page = *page.read();
        let cur_size = *page_size.read();
        let native_page1 = cur_page == 1 && cur_size == run_size && sort_key.is_none();
        let page_spec = PageSpec {
            snapshot: snapshot.unwrap_or(SnapshotId(0)),
            page: cur_page,
            page_size: cur_size,
            sort: sort_key,
        };
        let fetch = use_query(page_spec.query(&engine, snapshot.is_some() && !native_page1));

        let session = use_radio::<SessionState, Chan>(Chan::Request(ws));
        let log = use_consume::<LogCtx>();
        let cancel = {
            let engine = engine.clone();
            let run = self.spec.run;
            move |()| actions::cancel_run(&engine, session, log, ws, run)
        };

        let tab_name = session.read().name(ws);
        let export_sort = page_spec.sort.clone();
        let export_app = use_consume::<AppCtx>();
        let export_log = use_consume::<LogCtx>();
        let export_engine = engine;
        let export_subtree = use_consume::<Subtree>();
        let export_target = |rows: &QueryPage| -> Option<ExportLaunch> {
            rows.output.snapshot.map(|snapshot| ExportLaunch {
                target: ExportTarget {
                    snapshot,
                    columns: rows.output.columns.clone(),
                    total: rows.output.total,
                    sort: export_sort.clone(),
                    page: cur_page,
                    page_size: cur_size,
                    label: tab_name.clone(),
                    sample: rows.output.rows.clone(),
                },
                engine: export_engine.clone(),
                app: export_app.clone(),
                log: export_log,
                subtree: export_subtree.clone(),
            })
        };

        let shape_sql = self.spec.sql.clone();
        let shape_target = |rows: &QueryPage| -> Option<ShapeTarget> {
            rows.output.snapshot.map(|_| ShapeTarget {
                tab: ws,
                sql: shape_sql.clone(),
                columns: rows.output.columns.clone(),
                seed: None,
            })
        };

        let reader = query.read();
        let (body, bar): (Element, StatusBar) = match &*reader.state() {
            QueryStateData::Pending | QueryStateData::Loading { .. } => (
                Running::new(cancel).into(),
                StatusBar::new(ResultsState::Running),
            ),
            QueryStateData::Settled {
                res: Ok(QueryOutcome::Rows(rows)),
                settlement_instant,
            } if results_view == ResultsView::Chart => (
                ChartView::new(
                    ws,
                    find,
                    export_target(rows),
                    rows.output.snapshot,
                    rows.output.columns.clone(),
                )
                .shape(shape_target(rows))
                .into(),
                StatusBar::new(ResultsState::Chart).info(RunInfo {
                    total: rows.output.total,
                    elapsed_ms: rows.output.elapsed_ms,
                    settled: *settlement_instant,
                }),
            ),
            QueryStateData::Settled {
                res: Ok(QueryOutcome::Rows(rows)),
                settlement_instant,
            } => {
                let run_grid =
                    pages.run_page(|| Rc::new(GridData::from_run(&rows.output, &rows.batch)));
                let row_base = (cur_page - 1) * cur_size;
                let needle = find.needle();
                let (view, row_nums) = if native_page1 {
                    let fv = pages.view(
                        PageKey::Run,
                        || run_grid.clone(),
                        needle.as_deref(),
                        row_base,
                    );
                    (PageRead::Ready(fv.data), fv.row_nums)
                } else {
                    match &*fetch.read().state() {
                        QueryStateData::Settled {
                            res: Ok(fetched), ..
                        } => {
                            let fv = pages.view(
                                PageKey::Snapshot(page_spec),
                                || {
                                    Rc::new(GridData::from_page(
                                        rows.output.columns.clone(),
                                        fetched.rows.clone(),
                                        fetched.batch.clone(),
                                    ))
                                },
                                needle.as_deref(),
                                row_base,
                            );
                            (PageRead::Ready(fv.data), fv.row_nums)
                        }
                        QueryStateData::Settled { res: Err(err), .. } => {
                            (PageRead::Failed(err.clone()), None)
                        }
                        QueryStateData::Pending | QueryStateData::Loading { .. } => {
                            (PageRead::Loading, None)
                        }
                    }
                };
                let bar = StatusBar::new(ResultsState::Grid)
                    .pager(Pager {
                        page,
                        page_size,
                        total: rows.output.total,
                    })
                    .info(RunInfo {
                        total: rows.output.total,
                        elapsed_ms: rows.output.elapsed_ms,
                        settled: *settlement_instant,
                    })
                    .view(view.clone());
                (
                    DataGrid::new(run_grid, view, row_base, self.spec.tab, find, sort)
                        .row_nums(row_nums)
                        .total(rows.output.total)
                        .export(export_target(rows))
                        .shape(shape_target(rows))
                        .into(),
                    bar,
                )
            }
            QueryStateData::Settled {
                res: Ok(QueryOutcome::Plan(plan)),
                ..
            } => {
                let tab = explain_plan::effective_tab(plan, *plan_tab.read());
                let ops = match tab {
                    PlanTab::Physical => plan.physical.len(),
                    PlanTab::Logical => plan.logical.len(),
                };
                (
                    ExplainPlan::new(plan.clone(), plan_tab).into(),
                    StatusBar::new(ResultsState::ExplainPlan).plan(ops, tab),
                )
            }
            QueryStateData::Settled {
                res: Ok(QueryOutcome::Statement(report)),
                ..
            } => (
                StatementState::new(report.kind, report.message.clone()).into(),
                StatusBar::new(ResultsState::Statement).statement(report.kind, report.elapsed_ms),
            ),
            QueryStateData::Settled { res: Err(err), .. } => (
                ErrorState::new(err.clone(), self.spec.tab).into(),
                StatusBar::new(ResultsState::Error),
            ),
        };

        shell(body, bar)
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The pane frame every state shares. The state body flexes to fill the panel; the status bar
/// keeps its fixed 40px, so it stays pinned at the bottom no matter how tall the grid's content
/// is. Wrapping the body in an explicit `flex(1)` box (rather than leaning on each body to flex
/// itself) is what actually bounds the grid — otherwise its scroll view would grow to its
/// content and shove the footer off. The caller builds the bar (pager / info / aggregate ride
/// only with the grid state).
fn shell(body: Element, bar: StatusBar) -> Element {
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .content(Content::Flex)
        .child(
            rect()
                .width(Size::fill())
                .height(Size::flex(1.))
                .overflow(Overflow::Clip)
                .child(body),
        )
        .child(bar)
        .into()
}
