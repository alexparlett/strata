//! The single UI-intent funnel. Every UI event handler emits an [`Action`] and
//! calls [`dispatch`], which routes it — via one exhaustive match — to the
//! handler in the matching domain module (`query`, `tab`, `catalog`, `panel`,
//! `overlay`). Mirrors the way the engine takes `engine::Command`s.
//!
//! One `Action` per intent: a tab's `×` and the tab-menu "Close" both emit the
//! *same* `CloseTab(idx)` — menus map their rows to concrete actions, they don't
//! re-route through a wrapper.

// Import `Signal` explicitly rather than glob-importing the dioxus prelude —
// the prelude also exports an `Action`, which would collide with our enum and
// break `use Action::*` in `dispatch`.
use dioxus::prelude::Signal;
use dioxus::signals::WritableExt;
use crate::state::{AppState, LogTab, PlanTab, RemoveKind, ResizeTarget};

// Domain handler modules. `panel` (the shared `resize_handle` component),
// `projects` (window startup), and `catalog` (the modal's source scan,
// `scan_sources`) are reached from outside the action layer; the rest are
// private — reachable only through `dispatch`.
pub(crate) mod catalog;
pub(crate) mod overlay;
pub mod panel;
// `projects` is public so a window's `ProjectRoot` startup can load its assigned
// project in place (`load_current`), outside the dispatch funnel.
pub mod projects;
mod query;
mod tab;

/// Every discrete UI intent. `Clone` so menu components can stash an action per
/// row and dispatch a copy on click.
#[derive(Clone)]
pub enum Action {
    // ── query & results ──
    RunQuery,
    FetchPage(usize),
    LoadSelectStar(String),
    FormatSql,
    ClearSql,
    SaveAsView,
    SaveQuery,
    OpenSavedQuery(String),
    DeleteSavedQuery(String),
    SetResultSearch(String),
    DismissQueryError,
    SetPlanTab(PlanTab),
    TogglePlanRaw,
    TogglePageSizeMenu,
    SetPageSize(usize),

    // ── tabs ──
    NewTab,
    SwitchTab(usize),
    CloseTab(usize),
    CloseOtherTabs(usize),
    CloseTabsRight(usize),
    CloseAllTabs,
    ReopenTab,
    StartRename(usize),
    RenameInput(String),
    CommitRename,
    CancelRename,

    // ── catalog ──
    OpenConfigNew,
    OpenConfigEdit(String),
    RegisterTable(crate::state::ConfigForm),
    ConfirmRemove { kind: RemoveKind, name: String },
    EditView(String),
    SetFilter(String),
    ToggleTableOpen(usize),
    ToggleViewOpen(usize),
    SelectColumn { table: String, column: String },

    // ── panels ──
    StartResize { target: ResizeTarget, origin: f64, start: f64 },
    ResizeMove { x: f64, y: f64 },
    EndResize,
    ToggleSidebar,
    CloseInspector,
    ToggleWindowFill,

    // ── overlays ──
    CloseOverlays,
    // Bottom drawer (History + Events tabs).
    OpenHistory,
    OpenEvents,
    SetLogTab(LogTab),
    OpenHistoryQuery(String),
    RunHistoryQuery(String),
    ToggleLog,
    ClearDrawer,
    ToggleLogRow(u64),
    ToggleLogHeight,
    RunExport(crate::state::ExportForm),
    // Settings prefs now write the `crate::settings` store directly from the
    // Settings modal — they are no longer dispatched through here.

    // ── project ──
    // RustRover-style: "Open" picks a directory and opens its `.psproj` or
    // creates one — there is no separate "New".
    OpenProject,
    OpenRecent(String),
    SaveProject,
    CloseProject,
}

/// Execute an [`Action`]. Durable actions (those that mutate the project's
/// definitions/tabs) trigger a project autosave afterward; editor-affecting
/// actions bump `editor_epoch` so the SQL editor remounts onto the new content.
pub fn dispatch(mut state: Signal<AppState>, action: Action) {
    let durable = is_durable(&action);
    let editor = affects_editor(&action);
    run(state, action);
    if editor {
        state.write().editor_epoch += 1;
    }
    if durable {
        projects::autosave(state);
    }
}

/// Whether an action changes the active tab's SQL for a reason *other* than the
/// user typing, so the SQL editor must remount (see `AppState::editor_epoch`).
fn affects_editor(a: &Action) -> bool {
    use Action::*;
    matches!(
        a,
        SwitchTab(_)
            | NewTab
            | CloseTab(_)
            | CloseOtherTabs(_)
            | CloseTabsRight(_)
            | CloseAllTabs
            | ReopenTab
            | FormatSql
            | ClearSql
            | LoadSelectStar(_)
            | OpenSavedQuery(_)
            | EditView(_)
            | OpenHistoryQuery(_)
            | RunHistoryQuery(_)
    )
}

/// Whether an action mutates the durable project → should autosave.
fn is_durable(a: &Action) -> bool {
    use Action::*;
    matches!(
        a,
        RunQuery
            | FormatSql
            | ClearSql
            | SaveAsView
            | SaveQuery
            | OpenSavedQuery(_)
            | DeleteSavedQuery(_)
            | LoadSelectStar(_)
            | NewTab
            | CloseTab(_)
            | CloseOtherTabs(_)
            | CloseTabsRight(_)
            | CloseAllTabs
            | ReopenTab
            | CommitRename
            | RegisterTable(_)
            | ConfirmRemove { .. }
            | EditView(_)
            | OpenHistoryQuery(_)
            | RunHistoryQuery(_)
    )
}

fn run(state: Signal<AppState>, action: Action) {
    use Action::*;
    match action {
        // query & results
        RunQuery => query::run(state),
        FetchPage(page) => query::fetch_page(state, page),
        LoadSelectStar(name) => query::select_star(state, &name),
        FormatSql => query::format(state),
        ClearSql => query::clear(state),
        SaveAsView => query::save_as_view(state),
        SaveQuery => query::save(state),
        OpenSavedQuery(name) => query::open_saved(state, &name),
        DeleteSavedQuery(name) => query::delete_saved(state, &name),
        SetResultSearch(q) => query::set_result_search(state, q),
        DismissQueryError => query::dismiss_error(state),
        SetPlanTab(tab) => query::set_plan_tab(state, tab),
        TogglePlanRaw => query::toggle_plan_raw(state),
        TogglePageSizeMenu => query::toggle_page_size_menu(state),
        SetPageSize(sz) => query::set_page_size(state, sz),

        // tabs
        NewTab => tab::add(state),
        SwitchTab(idx) => tab::switch(state, idx),
        CloseTab(idx) => tab::close(state, idx),
        CloseOtherTabs(idx) => tab::close_others(state, idx),
        CloseTabsRight(idx) => tab::close_right(state, idx),
        CloseAllTabs => tab::close_all(state),
        ReopenTab => tab::reopen(state),
        StartRename(idx) => tab::start_rename(state, idx),
        RenameInput(val) => tab::rename_input(state, val),
        CommitRename => tab::commit_rename(state),
        CancelRename => tab::cancel_rename(state),

        // catalog
        OpenConfigNew => catalog::open_config_new(state),
        OpenConfigEdit(name) => catalog::open_config_edit(state, &name),
        RegisterTable(draft) => catalog::register_table(state, draft),
        ConfirmRemove { kind, name } => catalog::confirm_remove(state, kind, name),
        EditView(name) => catalog::edit_view(state, &name),
        SetFilter(f) => catalog::set_filter(state, f),
        ToggleTableOpen(i) => catalog::toggle_table_open(state, i),
        ToggleViewOpen(i) => catalog::toggle_view_open(state, i),
        SelectColumn { table, column } => catalog::select_column(state, table, column),

        // panels
        StartResize {
            target,
            origin,
            start,
        } => panel::start_resize(state, target, origin, start),
        ResizeMove { x, y } => panel::resize_move(state, x, y),
        EndResize => panel::end_resize(state),
        ToggleSidebar => panel::toggle_sidebar(state),
        CloseInspector => panel::close_inspector(state),
        ToggleWindowFill => panel::toggle_window_fill(state),

        // overlays
        CloseOverlays => overlay::close_all(state),
        OpenHistory => overlay::open_history(state),
        OpenEvents => overlay::open_events(state),
        SetLogTab(tab) => overlay::set_log_tab(state, tab),
        OpenHistoryQuery(sql) => overlay::open_history_query(state, sql),
        RunHistoryQuery(sql) => {
            overlay::open_history_query(state, sql);
            query::run(state);
        }
        ToggleLog => overlay::toggle_log(state),
        ClearDrawer => overlay::clear_drawer(state),
        ToggleLogRow(id) => overlay::toggle_log_row(state, id),
        ToggleLogHeight => overlay::toggle_log_height(state),
        RunExport(opts) => query::run_export(state, opts),

        // project (open/recent spawn new windows; close closes this window)
        OpenProject => projects::open_dir(state),
        OpenRecent(path) => projects::open_recent(path),
        SaveProject => projects::save(state),
        CloseProject => projects::close(state),
    }
}
