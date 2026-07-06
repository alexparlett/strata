//! Center pane: query tabs, SQL editor, and the results area. Split across this
//! module — `tabs` (`Tabs`), `editor` (`Editor`), `results` (`Results` state
//! switch + toolbar + pager + placeholders), `grid` (`ResultsGrid` + nested-cell
//! view), and `plan_view` (`PlanView`).
//!
//! Each pane is its own `#[component]` and pulls `AppState` from context, so they
//! have independent reactive scopes — typing in the editor doesn't re-render the
//! results grid, and each pane's local UI signals (the tab menu, the cell view)
//! stay component-local. The per-item render helpers (`grid::render_cell`,
//! `plan_view::plan_node_card`) stay plain fns — a component per grid cell would be
//! all overhead and no benefit.

use dioxus::prelude::*;

use crate::state::AppState;

mod editor;
mod grid;
mod plan_view;
mod results;
mod tabs;

#[component]
pub fn Workspace() -> Element {
    let state = use_context::<Signal<AppState>>();
    let has_ws = !state.read().project.workspaces.is_empty();
    rsx! {
        main { class: "ps-main",
            tabs::Tabs {}
            if has_ws {
                editor::Editor {}
                {crate::action::panel::resize_handle(state, crate::state::ResizeTarget::Editor)}
                results::Results {}
            } else {
                results::EmptyState {}
            }
        }
    }
}
