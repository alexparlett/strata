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
use crate::state::{AppState, LogTab, PlanTab, RemoveKind, ResizeTarget, SettingsCat};

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
    ConfirmConfig,
    RequestRemove { kind: RemoveKind, name: String },
    CancelRemove,
    ConfirmRemove,
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
    ToggleCmdk,
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
    OpenExport,
    RunExport,
    OpenSettings,
    // Settings prefs (persist to app config) + the modal's category nav.
    SetTheme(String),
    SetSettingsCat(SettingsCat),
    ToggleSyncOs,
    SetDensity(bool),
    ToggleZebra,
    SetRowLimit(usize),
    ToggleReopenStartup,
    SetDefaultProjectDir(String),
    SetOpenPref(String),
    ToggleConfirmClose,
    OpenCellPopover { name: String, type_label: String, json: String },

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
            | ConfirmConfig
            | ConfirmRemove
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
        ConfirmConfig => catalog::confirm_config(state),
        RequestRemove { kind, name } => catalog::request_remove(state, kind, name),
        CancelRemove => catalog::cancel_remove(state),
        ConfirmRemove => catalog::confirm_remove(state),
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
        ToggleCmdk => overlay::toggle_cmdk(state),
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
        OpenExport => overlay::open_export(state),
        RunExport => query::run_export(state),
        OpenSettings => overlay::open_settings(state),
        SetTheme(id) => overlay::set_theme(state, id),
        SetSettingsCat(cat) => overlay::set_settings_cat(state, cat),
        ToggleSyncOs => overlay::toggle_sync_os(state),
        SetDensity(v) => overlay::set_density(state, v),
        ToggleZebra => overlay::toggle_zebra(state),
        SetRowLimit(v) => overlay::set_row_limit(state, v),
        ToggleReopenStartup => overlay::toggle_reopen_startup(state),
        SetDefaultProjectDir(v) => overlay::set_default_project_dir(state, v),
        SetOpenPref(v) => overlay::set_open_pref(state, v),
        ToggleConfirmClose => overlay::toggle_confirm_close(state),
        OpenCellPopover {
            name,
            type_label,
            json,
        } => overlay::open_cell(state, name, type_label, json),

        // project (open/recent spawn new windows; close closes this window)
        OpenProject => projects::open_dir(state),
        OpenRecent(path) => projects::open_recent(path),
        SaveProject => projects::save(state),
        CloseProject => projects::close(state),
    }
}
