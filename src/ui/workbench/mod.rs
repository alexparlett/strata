//! The **workbench** — the center working area: the tab strip (`Tabs`) plus the
//! active tab's `Workspace` (SQL editor + results). The parent `Workbench` renders
//! the strip and the active tab's content (or the no-tabs empty state).
//!
//! Panes below are each their own `#[component]` pulling `AppState` from context,
//! so they have independent reactive scopes — typing in the editor doesn't
//! re-render the results grid — and their transient UI signals (the tab menu, the
//! cell view, the rename draft) stay component-local. Submodules: `tabs` (`Tabs`),
//! `workspace` (`Workspace` = editor + results), `editor` (`Editor`), `results`
//! (`Results` switch + toolbar + pager + placeholders), `grid` (`ResultsGrid` +
//! nested-cell view), `plan_view` (`PlanView`). The per-item render helpers
//! (`grid::render_cell`, `plan_view::plan_node_card`) stay plain fns.
//!
//! Note the two `Workspace`s: `crate::state::Workspace` is a *tab's data*; the
//! `workspace::Workspace` component here is that tab's *view*.

use dioxus::prelude::*;

use crate::state::AppState;

mod editor;
mod grid;
mod plan_view;
mod results;
mod tabs;
mod workspace;

/// The center working area: the tab strip plus the active tab's `Workspace` (or
/// the no-tabs empty state).
#[component]
pub fn Workbench() -> Element {
    let state = use_context::<Signal<AppState>>();
    let has_ws = !state.read().project.workspaces.is_empty();
    rsx! {
        main { class: "ps-main",
            tabs::Tabs {}
            if has_ws {
                workspace::Workspace {}
            } else {
                results::EmptyState {}
            }
        }
    }
}
