//! The query output surface below the editor. The body is **freya-query off the tab's SQL**
//! (state-arch §6): the pane reads the workbench's Run trigger and derives its state from
//! that query's own lifecycle — no press for this tab → **empty**; `Pending`/`Loading` →
//! **running**; settled rows → **grid**; a settled plan → **explain**; a settled `Err` →
//! **error**. Every state sits over the same **status bar** footer (the results-pane footer,
//! themed by `status_bar`).

use std::time::Duration;

use freya::prelude::*;
use freya::query::{use_query, Query, QueryStateData};

mod datagrid;
mod empty;
mod error;
mod explain_plan;
mod running;
mod selection;
mod status_bar;
mod toolbar;

use datagrid::DataGrid;
use empty::EmptyState;
use error::ErrorState;
use running::Running;
use status_bar::StatusBar;

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{QueryOutcome, QuerySpec, RunQuery};
use crate::apps::project::state::TabId;
use crate::apps::project::views::workbench::results::explain_plan::ExplainPlan;
use crate::apps::project::views::workbench::results::selection::Selection;
use status_bar::Pager;
pub use datagrid::DataGridThemePreference;
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
            None => shell(EmptyState.into(), ResultsState::Empty, None),
            Some(spec) => {
                // Keyed by the press's nonce so a new Run remounts the body — the page below
                // resets to 1 and the grid's column widths reseed for the new schema.
                let run = spec.run;
                ResultsBody { spec, key: DiffKey::None }.key(run).into()
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
        // The 1-based snapshot page the grid shows. It lives here — beside the status bar that
        // pages it and the grid that reads it — and starts at 1 for every press (this component
        // is keyed by the press's nonce).
        let page = use_state(|| 1usize);

        let reader = query.read();
        let (body, state, pager): (Element, ResultsState, Option<Pager>) = match &*reader.state() {
            QueryStateData::Pending | QueryStateData::Loading { .. } => {
                (Running.into(), ResultsState::Running, None)
            }
            QueryStateData::Settled { res: Ok(QueryOutcome::Rows(rows)), .. } => (
                DataGrid::new(&rows.output, page).into(),
                ResultsState::Grid,
                Some(Pager {
                    page,
                    total: rows.output.total,
                    page_size: rows.output.page_size,
                }),
            ),
            // The plan body is a placeholder — P2-05 renders the settled `QueryPlan`.
            QueryStateData::Settled { res: Ok(QueryOutcome::Plan(_)), .. } => {
                (ExplainPlan.into(), ResultsState::ExplainPlan, None)
            }
            QueryStateData::Settled { res: Err(err), .. } => {
                (ErrorState::new(err.clone()).into(), ResultsState::Error, None)
            }
        };

        shell(body, state, pager)
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The pane frame every state shares. The state body flexes to fill the panel; the status bar
/// keeps its fixed 40px, so it stays pinned at the bottom no matter how tall the grid's content
/// is. Wrapping the body in an explicit `flex(1)` box (rather than leaning on each body to flex
/// itself) is what actually bounds the grid — otherwise its scroll view would grow to its
/// content and shove the footer off. The pager rides only with the grid state.
fn shell(body: Element, state: ResultsState, pager: Option<Pager>) -> Element {
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
        .child(StatusBar::new(state).pager(pager))
        .into()
}
