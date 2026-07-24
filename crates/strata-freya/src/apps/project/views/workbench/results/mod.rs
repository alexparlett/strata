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
use freya::radio::use_radio;
use strata_core::engine::plan::PlanTab;
use strata_model::SnapshotId;

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
mod sort;
mod status_bar;
mod toolbar;

use chart::ChartView;
use datagrid::{DataGrid, GridData, PageRead};
use find::FindState;
use sort::SortState;
use empty::EmptyState;
use error::ErrorState;
use running::Running;
use status_bar::StatusBar;

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{
    FetchSnapshotPage, PageSpec, QueryOutcome, QuerySpec, RunId, RunQuery,
};
use crate::apps::project::state::{Chan, ResultsView, SessionState, TabId};
use crate::apps::project::views::workbench::results::explain_plan::ExplainPlan;
use crate::apps::project::views::workbench::results::selection::Selection;
use status_bar::{Pager, RunInfo};
pub use cell_view::CellViewThemePreference;
pub use datagrid::DataGridThemePreference;
pub use record_view::RecordViewThemePreference;
pub use explain_plan::ExplainPlanThemePreference;
pub use running::{CancelButtonThemePartial, CancelButtonThemePreference};
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
    /// Rows are available and the tab's view mode is Chart (P2-07) — the chart body.
    Chart,
    /// Explain plan is available.
    ExplainPlan,
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
        // Keyed by the tab, like `EditorTab`: the pane renders in one fixed slot, so without
        // a key a tab switch reuses the scope and the `Selection` context leaks across tabs.
        Self { id, running, key: DiffKey::None }.key(id)
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

        // Subscribes to the tab's Run trigger: a press re-renders the pane with the new spec.
        let id = self.id;
        let radio = use_radio::<SessionState, Chan>(Chan::Request(id));
        let spec = radio.read().request(id).cloned();

        let el: Element = match spec {
            None => shell(EmptyState.into(), StatusBar::new(ResultsState::Empty)),
            Some(spec) => {
                // Keyed by the press's nonce so a new Run remounts the body — the page below
                // resets to 1 and the grid's column widths reseed for the new schema.
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
/// [`QuerySpec`] and derives the body from the query state. `stale_time(MAX)` because a Run
/// is an *action* — a settled entry must never re-execute by itself (SNAPSHOT_SPEC §6); only
/// a new press (fresh nonce → new key) runs again.
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
    fn render(&self) -> impl IntoElement {
        let engine = use_consume::<EngineCtx>();
        let query = use_query(
            Query::new(self.spec.clone(), RunQuery(engine.captured()))
                .stale_time(Duration::MAX),
        );

        // Mirror the run's in-flight-ness into the workbench's `running` slot for the
        // toolbar's Run→Cancel flip (P2-15). The toolbar cannot subscribe this query
        // itself: freya-query re-runs *stale* entries when a subscriber mounts, and an
        // in-flight entry reads as stale — a second enabled subscriber would double-execute
        // the run. So this body, the sole subscriber, resolves the slot: the press's nonce
        // while Pending/Loading, cleared on settle. Unmount (cancel / supersede / tab
        // close) clears it too — nonce-guarded, so if a new press's body mounts before the
        // old one drops, the stale drop can't clobber the newer run's flag.
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
        // The 1-based snapshot page the grid shows and the rows-per-page it's cut into. They
        // live here — beside the status bar that pages them and the grid that reads them — and
        // reset for every press (this component is keyed by the press's nonce). `page_size`
        // starts at the size the Run itself executed with.
        let page = use_state(|| 1usize);
        let run_size = self.spec.page_size;
        let page_size = use_state(move || run_size);
        // The plan view's tree selection (P2-05). Like the page, per-press — a new Run
        // starts back on physical. It lives here so the status bar's active-tab summary
        // reads the same selection the view renders. (The Raw/Tree flag needs no lifting —
        // the toolbar's ToggleButton owns it, mirrored inside the plan view.)
        let plan_tab = use_state(PlanTab::default);

        // The tab's Table/Chart view mode (P2-07) — per-tab (its own `Chan::View` channel),
        // so it survives re-runs and tab switches; the toolbar's toggle writes it.
        let ws = self.spec.tab;
        let view_radio = use_radio::<SessionState, Chan>(Chan::View(ws));
        let results_view = view_radio.read().view(ws);

        // Find-in-results (P2-09): per-press state, like the page number — a new Run starts
        // unfiltered. A query change reshuffles the filtered rows under the page-local
        // selection — the old indices would silently point at *different* cells (the same
        // invariant the pager jump protects) — so it clears the selection.
        let find = FindState::use_new();
        let sel = use_consume::<State<Selection>>();
        // Column sort (P2-13): per-press view intent, like the page — a new Run starts
        // unsorted. Cycling clears the selection and jumps to page 1 itself (see `sort.rs`).
        let sort = SortState::use_new(page, sel);
        use_side_effect(move || {
            let _ = find.query.read();
            let mut sel = sel;
            if *sel.peek() != Selection::None {
                sel.set(Selection::None);
            }
        });

        // The current page's snapshot read (SNAPSHOT_SPEC §6): keyed by [`PageSpec`] and cached
        // forever (`stale_time(MAX)` — reads of an immutable snapshot never go stale), so a
        // revisited page settles straight from the cache. The Run's embedded page 1 short-circuits
        // this read — but only while the page size still matches the Run's own: a page-size change
        // re-cuts the snapshot, so even page 1 must then be a real read. Disabled until the Run
        // settles rows — the placeholder id of a disabled read never reaches the engine.
        // The sort intent resolves to the engine's `(column name, ascending)` here — the one
        // boundary that knows the settled schema (the intent itself is index-keyed; an index
        // the schema can't resolve falls back to unsorted rather than erroring the read).
        let (snapshot, sort_key) = match &*query.read().state() {
            QueryStateData::Settled { res: Ok(QueryOutcome::Rows(rows)), .. } => (
                rows.output.snapshot,
                (*sort.by.read()).and_then(|(ci, asc)| {
                    rows.output.columns.get(ci).map(|c| (c.name.clone(), asc))
                }),
            ),
            _ => (None, None),
        };
        let cur_page = *page.read();
        let cur_size = *page_size.read();
        // A sorted read is never the Run's own page 1 — the snapshot re-orders under it.
        let native_page1 = cur_page == 1 && cur_size == run_size && sort_key.is_none();
        let fetch = use_query(
            Query::new(
                PageSpec {
                    snapshot: snapshot.unwrap_or(SnapshotId(0)),
                    page: cur_page,
                    page_size: cur_size,
                    sort: sort_key,
                },
                FetchSnapshotPage(engine.captured()),
            )
            .stale_time(Duration::MAX)
            .enable(snapshot.is_some() && !native_page1),
        );

        // Cancel = abort engine-side (S14: tag-guarded, a stale press can't kill a newer run)
        // + clear this tab's Run trigger, unmounting this body back to the empty state. The
        // query entry settles `Err("cancelled")` unobserved — a new press is a fresh nonce
        // anyway.
        let session = use_radio::<SessionState, Chan>(Chan::Request(ws));
        let cancel = {
            let engine = engine.clone();
            let run = self.spec.run;
            let mut session = session;
            move |()| {
                engine.cancel(ws.into(), run.into());
                session.write_channel(Chan::Request(ws)).clear_request(ws);
            }
        };

        let reader = query.read();
        let (body, bar): (Element, StatusBar) = match &*reader.state() {
            QueryStateData::Pending | QueryStateData::Loading { .. } => {
                (Running::new(cancel).into(), StatusBar::new(ResultsState::Running))
            }
            // Chart mode (P2-07): the placeholder body under the shared toolbar. The pager
            // and selection aggregate are grid concerns, so the bar keeps only the run
            // readouts; the page/find/sort state above stays put for the switch back.
            QueryStateData::Settled {
                res: Ok(QueryOutcome::Rows(rows)),
                settlement_instant,
            } if results_view == ResultsView::Chart => (
                ChartView::new(ws, find).into(),
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
                // Resolve the page both consumers share: the grid renders it, the status bar
                // aggregates the selection over it (see `PageRead`).
                let run_grid = Rc::new(GridData::from_run(&rows.output, &rows.batch));
                let view = if native_page1 {
                    PageRead::Ready(run_grid.clone())
                } else {
                    match &*fetch.read().state() {
                        QueryStateData::Settled { res: Ok(fetched), .. } => {
                            PageRead::Ready(Rc::new(GridData::from_page(
                                rows.output.columns.clone(),
                                fetched.rows.clone(),
                                fetched.batch.clone(),
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
                // The find filter narrows the *resolved* page (page-bounded — see `find`):
                // the grid and the status bar's selection aggregate both see the filtered
                // rows; the pager still walks the unfiltered snapshot.
                let row_base = (cur_page - 1) * cur_size;
                let needle = find.needle();
                let (view, row_nums) = match &view {
                    PageRead::Ready(data) => {
                        let fv = find::filter_page(needle.as_deref(), data, row_base);
                        (PageRead::Ready(fv.data), fv.row_nums)
                    }
                    other => (other.clone(), None),
                };
                let bar = StatusBar::new(ResultsState::Grid)
                    .pager(Pager { page, page_size, total: rows.output.total })
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
                        .into(),
                    bar,
                )
            }
            // The settled EXPLAIN (P2-05): the three-tier plan card tree. The status bar's
            // summary counts the *shown* tree — the same effective tab the view resolves.
            QueryStateData::Settled { res: Ok(QueryOutcome::Plan(plan)), .. } => {
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
