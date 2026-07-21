//! The query output surface below the editor. One of three state bodies — **empty** (no rows yet),
//! **running** (a query is executing), **grid** (rows are available) — each its own component,
//! under a single **status bar** footer present in every state (the results-pane footer, themed by
//! `status_bar`). The active state is driven by the query runtime (freya-query + the runs store)
//! once that lands; until then it defaults to `Empty`.

use freya::prelude::*;

mod datagrid;
mod empty;
mod running;
mod selection;
mod status_bar;
mod toolbar;
mod explain_plan;

use datagrid::DataGrid;
use empty::EmptyState;
use running::Running;
use status_bar::StatusBar;

use crate::apps::project::views::workbench::results::explain_plan::ExplainPlan;
use crate::apps::project::views::workbench::results::selection::Selection;
pub use datagrid::DataGridThemePreference;
pub use status_bar::StatusBarThemePreference;

/// Which of the three views the results pane shows.
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
}

/// The results pane. Switches between the empty / running / grid views.
#[derive(PartialEq)]
pub struct Results {
    state: ResultsState,
}

impl Results {
    pub fn new() -> Self {
        // SPIKE: force the grid so the VirtualScrollView spike is visible. Reverts to `Empty` (derived
        // from the runs store / query state) once the runtime layer is wired.
        Self {
            state: ResultsState::Grid,
        }
    }

    pub fn state(mut self, state: ResultsState) -> Self {
        self.state = state;
        self
    }
}

impl Component for Results {
    fn render(&self) -> impl IntoElement {
        use_provide_context(|| State::create(Selection::None));

        let body: Element = match self.state {
            ResultsState::Empty => EmptyState.into(),
            ResultsState::Running => Running.into(),
            ResultsState::Grid => DataGrid::new().into(),
            ResultsState::ExplainPlan => ExplainPlan.into(),
        };

        // The state body flexes to fill the panel; the status bar keeps its fixed 40px, so it stays
        // pinned at the bottom no matter how tall the grid's content is. Wrapping the body in an
        // explicit `flex(1)` box (rather than leaning on each body to flex itself) is what actually
        // bounds the grid — otherwise its scroll view would grow to its content and shove the footer off.
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
            .child(StatusBar::new(self.state))
    }
}
