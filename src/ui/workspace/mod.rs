//! Center pane: query tabs, SQL editor, and the results area. Split across this
//! module — `tabs` (the tab strip + context menu), `editor` (the SQL editor),
//! `results` (the state switch: running / error / plan / grid / empty, plus the
//! toolbar and pager), `grid` (the results grid + nested-cell view), and
//! `plan_view` (the EXPLAIN plan tree).
//!
//! `state` is read once in `Workspace` and threaded into the render helpers, so
//! no hooks are called inside loops/branches (Dioxus rules of hooks).

use dioxus::prelude::*;

use crate::state::AppState;
use crate::ui::components::Point;

mod editor;
mod grid;
mod plan_view;
mod results;
mod tabs;

/// A nested-cell view target (struct/list/map cell), shown in a `Dialog`. Built by
/// `grid::render_cell` and rendered by `grid::cell_dialog`; threaded through the
/// results helpers as a workspace-local signal.
#[derive(Clone)]
pub(crate) struct CellView {
    pub(crate) name: String,
    pub(crate) type_label: String,
    pub(crate) json: String,
}

#[component]
pub fn Workspace() -> Element {
    let state = use_context::<Signal<AppState>>();
    // Self-contained: the tab context menu lives here, not in `AppState`.
    let tab_menu = use_signal(|| None::<(usize, Point)>);
    // The nested-cell view is likewise workspace-local, opened from a grid cell.
    let cell_view = use_signal(|| None::<CellView>);
    let has_ws = !state.read().project.workspaces.is_empty();
    rsx! {
        main { class: "ps-main",
            {tabs::tabs(state, tab_menu)}
            if has_ws {
                {editor::editor(state)}
                {crate::action::panel::resize_handle(state, crate::state::ResizeTarget::Editor)}
                {results::results_area(state, cell_view)}
            } else {
                {results::empty_state(state)}
            }
            if let Some(c) = cell_view() {
                {grid::cell_dialog(cell_view, c)}
            }
        }
    }
}
