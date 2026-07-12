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
use crate::plan::PlanTab;
use crate::state::{AppState, LogTab, RemoveKind, ResizeTarget};
use dioxus::prelude::Signal;

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
    /// Cancel the active tab's in-flight query / explain (S14).
    CancelQuery,
    /// Run an `EXPLAIN [ANALYZE]` of the active tab's SQL **without** mutating the
    /// editor buffer (E4) — wraps engine-side only, like Save-as-view. `true` = ANALYZE.
    RunExplain(bool),
    /// Clear the active tab's results back to the empty state (Rz8).
    ClearResults,
    /// Open/close a tab's results find popover (U6). Closing clears its find query.
    SetResultsFind {
        ws: crate::session::WorkspaceId,
        open: bool,
    },
    FetchPage(usize),
    /// Cycle sort on results column `usize` (asc → desc → clear); re-fetches page 1 (Rz6).
    SortColumn(usize),
    /// Copy the current grid selection to the clipboard in the given format (Rz4).
    CopySelection(crate::serialize::TextFormat),
    /// Copy a single record — all columns of the page-local filtered row index — to the
    /// clipboard in the given format (Rz5, the record view's `⋯` menu).
    CopyRecord(usize, crate::serialize::TextFormat),
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
    SetResultsView(crate::runs::ResultsView),

    // ── tabs ──
    NewTab,
    SwitchTab(crate::session::WorkspaceId),
    CloseTab(crate::session::WorkspaceId),
    /// Close a tab unconditionally (from the discard-confirm dialog). `CloseTab`
    /// routes here after the user confirms discarding an unsaved tab.
    CloseTabForce(crate::session::WorkspaceId),
    CloseOtherTabs(crate::session::WorkspaceId),
    CloseTabsRight(crate::session::WorkspaceId),
    CloseAllTabs,
    ReopenTab,
    /// Commit an inline tab rename. Start / draft / cancel are transient UI state
    /// owned by the `Tabs` component; only the commit (durable) is an action.
    RenameTab(crate::session::WorkspaceId, String),
    /// Duplicate a tab: clone its SQL into a new "<name> copy" tab to its right.
    DuplicateTab(crate::session::WorkspaceId),

    // ── catalog ──
    OpenConfigNew,
    OpenConfigEdit(String),
    RegisterTable(crate::state::ConfigForm),
    ConfirmRemove {
        kind: RemoveKind,
        name: String,
    },
    EditView(String),
    SetFilter(String),
    ToggleTableOpen(usize),
    ToggleViewOpen(usize),
    SelectColumn {
        table: String,
        column: String,
    },

    // ── panels ──
    StartResize {
        target: ResizeTarget,
        origin: f64,
        start: f64,
    },
    ResizeMove {
        x: f64,
        y: f64,
    },
    EndResize,
    // ── tab drag-to-reorder (T1) ──
    StartTabDrag {
        id: crate::session::WorkspaceId,
        from: usize,
        name: String,
        off_x: f64,
        off_y: f64,
        x: f64,
        y: f64,
    },
    TabDragMove {
        x: f64,
        y: f64,
    },
    TabDragOver(usize),
    EndTabDrag,
    ToggleSidebar,
    CloseInspector,
    ToggleWindowFill,

    // ── overlays ──
    CloseOverlays,
    // Bottom drawer (History + Events tabs).
    OpenHistory,
    OpenEvents,
    OpenProblems,
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
    /// Resolve the open-target prompt (B10): open the pending project here or in a
    /// new window, optionally remembering the choice as the open preference.
    OpenChosen {
        new_window: bool,
        remember: bool,
    },
    SaveProject,
    CloseProject,
    /// Close the window unconditionally — from the running-query close confirm (S14).
    CloseWindowForce,
}

/// Execute an [`Action`]. Durable actions (those that mutate the project's
/// definitions or the working session) trigger an autosave afterward. The SQL
/// editor is a *controlled* input bound to its workspace's `sql` lens, so no
/// remount / epoch bump is needed — programmatic edits flow straight to the store.
pub fn dispatch(state: Signal<AppState>, action: Action) {
    let durable = is_durable(&action);
    let defs = durable && touches_defs(&action);
    run(state, action);
    if durable {
        if defs {
            projects::autosave(state);
        } else {
            projects::autosave_session(state);
        }
    }
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
            | CloseTabForce(_)
            | CloseOtherTabs(_)
            | CloseTabsRight(_)
            | CloseAllTabs
            | ReopenTab
            | RenameTab(..)
            | DuplicateTab(_)
            | RegisterTable(_)
            | ConfirmRemove { .. }
            | EditView(_)
            | OpenHistoryQuery(_)
            | RunHistoryQuery(_)
    )
}

/// Whether a durable action changed the committed **definitions** (catalog / views
/// / saved queries) → write `project.json`. Otherwise it's session-only (tabs /
/// history) → `session.json` only.
fn touches_defs(a: &Action) -> bool {
    use Action::*;
    matches!(
        a,
        SaveAsView | SaveQuery | DeleteSavedQuery(_) | RegisterTable(_) | ConfirmRemove { .. }
    )
}

fn run(state: Signal<AppState>, action: Action) {
    use Action::*;
    match action {
        // query & results
        RunQuery => query::run(state),
        RunExplain(analyze) => query::run_explain(state, analyze),
        ClearResults => query::clear_results(state),
        SetResultsFind { ws, open } => query::set_results_find(ws, open),
        CancelQuery => query::cancel(state),
        FetchPage(page) => query::fetch_page(state, page),
        SortColumn(ci) => query::sort_column(state, ci),
        CopySelection(fmt) => query::copy_selection(state, fmt),
        CopyRecord(row, fmt) => query::copy_record(state, row, fmt),
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
        SetResultsView(v) => query::set_results_view(v),

        // tabs
        NewTab => tab::add(state),
        SwitchTab(id) => tab::switch(state, id),
        CloseTab(id) => tab::close(state, id),
        CloseTabForce(id) => tab::close_force(state, id),
        CloseOtherTabs(id) => tab::close_others(state, id),
        CloseTabsRight(id) => tab::close_right(state, id),
        CloseAllTabs => tab::close_all(state),
        ReopenTab => tab::reopen(state),
        RenameTab(id, name) => tab::rename_tab(state, id, name),
        DuplicateTab(id) => tab::duplicate(state, id),

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
        StartTabDrag {
            id,
            from,
            name,
            off_x,
            off_y,
            x,
            y,
        } => tab::start_drag(state, id, from, name, off_x, off_y, x, y),
        TabDragMove { x, y } => tab::drag_move(state, x, y),
        TabDragOver(over) => tab::drag_over(state, over),
        EndTabDrag => tab::end_drag(state),
        ToggleSidebar => panel::toggle_sidebar(state),
        CloseInspector => panel::close_inspector(state),
        ToggleWindowFill => panel::toggle_window_fill(state),

        // overlays
        CloseOverlays => overlay::close_all(state),
        OpenHistory => overlay::open_history(state),
        OpenEvents => overlay::open_events(state),
        OpenProblems => overlay::open_problems(state),
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
        OpenRecent(path) => projects::open_recent(state, path),
        OpenChosen {
            new_window,
            remember,
        } => projects::choose_open(state, new_window, remember),
        SaveProject => projects::save(state),
        CloseProject => projects::close(state),
        CloseWindowForce => projects::close_now(state),
    }
}
