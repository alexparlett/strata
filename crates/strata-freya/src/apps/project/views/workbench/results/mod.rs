//! The query output surface below the editor. The body is **freya-query off the tab's SQL**
//! (state-arch §6): the pane reads the workbench's Run trigger and derives its state from
//! that query's own lifecycle — no press for this tab → **empty**; `Pending`/`Loading` →
//! **running**; settled rows → **grid**; a settled plan → **explain**; a settled `Err` →
//! **error**. Every state sits over the same **status bar** footer (the results-pane footer,
//! themed by `status_bar`).

use std::rc::Rc;
use std::time::Duration;

use freya::prelude::*;
use freya::query::{use_query, Query, QueryStateData};
use strata_model::SnapshotId;

mod datagrid;
mod empty;
mod error;
mod explain_plan;
mod running;
mod selection;
mod status_bar;
mod toolbar;

use datagrid::{DataGrid, GridData, PageRead};
use empty::EmptyState;
use error::ErrorState;
use running::Running;
use status_bar::StatusBar;

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{
    FetchSnapshotPage, PageSpec, QueryOutcome, QuerySpec, RunQuery,
};
use crate::apps::project::state::TabId;
use crate::apps::project::views::workbench::results::explain_plan::ExplainPlan;
use crate::apps::project::views::workbench::results::selection::Selection;
use status_bar::{Pager, RunInfo};
pub use datagrid::DataGridThemePreference;
pub use running::CancelButtonThemePreference;
pub use status_bar::StatusBarThemePreference;

/// Which of the state bodies the results pane shows — the status bar's coarse view state.
#[derive(PartialEq, Clone, Copy)]
pub enum ResultsState {
    /// No query has produced rows yet.
    Empty,
    /// A query is executing.
    Running,
    /// Rows are available — the grid.
    Grid,
    /// Explain plan is available.
    ExplainPlan,
    /// The last run settled `Err`.
    Error,
}

/// The results pane for one tab. Reads the workbench's Run trigger and mounts the
/// query-driven body when the latest press belongs to *this* tab — otherwise the empty
/// state. Revisiting a tab whose press is still current re-serves the settled outcome
/// from the freya-query cache (keyed by the press's [`QuerySpec`]) with zero engine traffic.
#[derive(PartialEq)]
pub struct Results {
    id: TabId,
    request: State<Option<QuerySpec>>,
}

impl Results {
    pub fn new(id: TabId, request: State<Option<QuerySpec>>) -> Self {
        Self { id, request }
    }
}

impl Component for Results {
    fn render(&self) -> impl IntoElement {
        use_provide_context(|| State::create(Selection::None));

        // Subscribes to the Run trigger: a press re-renders the pane with the new spec.
        let id = self.id;
        let spec = self.request.read().as_ref().filter(|spec| spec.tab == id).cloned();

        let el: Element = match spec {
            None => shell(EmptyState.into(), StatusBar::new(ResultsState::Empty)),
            Some(spec) => {
                // Keyed by the press's nonce so a new Run remounts the body — the page below
                // resets to 1 and the grid's column widths reseed for the new schema.
                let run = spec.run;
                ResultsBody { spec, request: self.request, key: DiffKey::None }.key(run).into()
            }
        };
        el
    }
}

/// The pane once its tab owns the current press: subscribes `use_query` on the press's
/// [`QuerySpec`] and derives the body from the query state. `stale_time(MAX)` because a Run
/// is an *action* — a settled entry must never re-execute by itself (SNAPSHOT_SPEC §6); only
/// a new press (fresh nonce → new key) runs again.
#[derive(PartialEq)]
struct ResultsBody {
    spec: QuerySpec,
    /// The workbench's Run trigger — Cancel clears it, returning the pane to empty.
    request: State<Option<QuerySpec>>,
    key: DiffKey,
}

impl KeyExt for ResultsBody {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ResultsBody {
    fn render(&self) -> impl IntoElement {
        let engine = use_consume::<EngineCtx>();
        let query = use_query(
            Query::new(self.spec.clone(), RunQuery(engine.captured()))
                .stale_time(Duration::MAX),
        );
        // The 1-based snapshot page the grid shows and the rows-per-page it's cut into. They
        // live here — beside the status bar that pages them and the grid that reads them — and
        // reset for every press (this component is keyed by the press's nonce). `page_size`
        // starts at the size the Run itself executed with.
        let page = use_state(|| 1usize);
        let run_size = self.spec.page_size;
        let page_size = use_state(move || run_size);

        // The current page's snapshot read (SNAPSHOT_SPEC §6): keyed by [`PageSpec`] and cached
        // forever (`stale_time(MAX)` — reads of an immutable snapshot never go stale), so a
        // revisited page settles straight from the cache. The Run's embedded page 1 short-circuits
        // this read — but only while the page size still matches the Run's own: a page-size change
        // re-cuts the snapshot, so even page 1 must then be a real read. Disabled until the Run
        // settles rows — the placeholder id of a disabled read never reaches the engine.
        let snapshot = match &*query.read().state() {
            QueryStateData::Settled { res: Ok(QueryOutcome::Rows(rows)), .. } => {
                rows.output.snapshot
            }
            _ => None,
        };
        let cur_page = *page.read();
        let cur_size = *page_size.read();
        let native_page1 = cur_page == 1 && cur_size == run_size;
        let fetch = use_query(
            Query::new(
                PageSpec {
                    snapshot: snapshot.unwrap_or(SnapshotId(0)),
                    page: cur_page,
                    page_size: cur_size,
                    sort: None,
                },
                FetchSnapshotPage(engine.captured()),
            )
            .stale_time(Duration::MAX)
            .enable(snapshot.is_some() && !native_page1),
        );

        // Cancel = abort engine-side (S14: tag-guarded, a stale press can't kill a newer run)
        // + clear the Run trigger, unmounting this body back to the empty state. The query
        // entry settles `Err("cancelled")` unobserved — a new press is a fresh nonce anyway.
        let cancel = {
            let engine = engine.clone();
            let ws = self.spec.tab;
            let run = self.spec.run;
            let mut request = self.request;
            move |()| {
                engine.cancel(ws.into(), run.into());
                request.set(None);
            }
        };

        let reader = query.read();
        let (body, bar): (Element, StatusBar) = match &*reader.state() {
            QueryStateData::Pending | QueryStateData::Loading { .. } => {
                (Running::new(cancel).into(), StatusBar::new(ResultsState::Running))
            }
            QueryStateData::Settled {
                res: Ok(QueryOutcome::Rows(rows)),
                settlement_instant,
            } => {
                // Resolve the page both consumers share: the grid renders it, the status bar
                // aggregates the selection over it (see `PageRead`).
                let run_grid = Rc::new(GridData::from_run(&rows.output));
                let view = if native_page1 {
                    PageRead::Ready(run_grid.clone())
                } else {
                    match &*fetch.read().state() {
                        QueryStateData::Settled { res: Ok(fetched), .. } => {
                            PageRead::Ready(Rc::new(GridData::from_page(
                                rows.output.columns.clone(),
                                fetched.rows.clone(),
                            )))
                        }
                        QueryStateData::Settled { res: Err(err), .. } => {
                            PageRead::Failed(err.clone())
                        }
                        QueryStateData::Pending | QueryStateData::Loading { .. } => {
                            PageRead::Loading
                        }
                    }
                };
                let bar = StatusBar::new(ResultsState::Grid)
                    .pager(Pager { page, page_size, total: rows.output.total })
                    .info(RunInfo {
                        total: rows.output.total,
                        elapsed_ms: rows.output.elapsed_ms,
                        settled: *settlement_instant,
                    })
                    .view(view.clone());
                let row_base = (cur_page - 1) * cur_size;
                (DataGrid::new(run_grid, view, row_base, self.request).into(), bar)
            }
            // The plan body is a placeholder — P2-05 renders the settled `QueryPlan`.
            QueryStateData::Settled { res: Ok(QueryOutcome::Plan(plan)), .. } => (
                ExplainPlan.into(),
                StatusBar::new(ResultsState::ExplainPlan).plan_ops(plan.physical.len()),
            ),
            QueryStateData::Settled { res: Err(err), .. } => {
                (ErrorState::new(err.clone()).into(), StatusBar::new(ResultsState::Error))
            }
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
                .child(body),
        )
        .child(bar)
        .into()
}
