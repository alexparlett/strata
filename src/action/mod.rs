//! The single UI-intent funnel. Every UI event handler emits an [`Action`] and
//! calls [`dispatch`], which routes it — via one exhaustive match — to the
//! handler in the matching domain module (`query`, `tab`, `catalog`, `panel`,
//! `overlay`). Mirrors the way the engine takes `engine::Command`s.
//!
//! One `Action` per intent: a tab's `×` and the tab-menu "Close" both emit the
//! *same* `CloseTab(idx)` — menus map their rows to concrete actions, they don't
//! re-route through a wrapper.

use crate::model::{LogTab, RemoveKind};
// The dioxus prelude is deliberately *not* glob-imported here — it also exports
// an `Action`, which would collide with our enum and break `use Action::*` in
// `dispatch`.
use crate::plan::PlanTab;

// Domain handler modules. `panel` (the `Resizer` handle + window-fill toggle),
// `projects` (window startup), and `catalog` (the modal's source scan,
// `scan_sources`) are reached from outside the action layer; the rest are
// private — reachable only through `dispatch`.
pub mod catalog;
pub mod overlay;
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
    /// Edit-menu ⌘A — routed by focus scope to the grid page or the focused text field.
    /// The menu is a dumb adapter; `query::select_all` decides where it lands.
    SelectAll,
    /// Edit-menu ⌘C — routed by focus scope to the grid copy or the focused text field.
    Copy,
    /// Copy the current grid selection to the clipboard in the given format (Rz4).
    CopySelection(crate::engine::serialize::TextFormat),
    /// Copy a single record — all columns of the page-local filtered row index — to the
    /// clipboard in the given format (Rz5, the record view's `⋯` menu).
    CopyRecord(usize, crate::engine::serialize::TextFormat),
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
    RegisterTable(crate::model::ConfigForm),
    ConfirmRemove {
        kind: RemoveKind,
        name: String,
    },
    EditView(String),
    ToggleTableOpen(usize),
    ToggleViewOpen(usize),
    /// Inspect a column — see [`crate::model::ColRef`] for why it takes a kind +
    /// owner + path rather than a name.
    SelectColumn(crate::model::ColRef),
    /// Re-infer catalog table schemas (the sidebar refresh button).
    RescanCatalog,
    /// Full-scan profile of a table (D4), no confirm — the PROFILE zone's ↻ re-scan,
    /// which re-runs something the user already chose.
    ProfileTable(String),
    /// Ask first — every *first* profile of a table goes through here, from either
    /// entry point (the inspector's button, the table context menu).
    AskProfileTable(String),
    /// The cost-confirm's "Profile".
    ConfirmProfileTable(String),
    /// Abort an in-flight profile — a full scan can run for minutes.
    CancelProfileTable(String),
    /// Open the profile's own query in a tab ("view as query").
    OpenProfileSql(String),

    // ── tab drag-to-reorder (T1) ──
    /// Commit a tab reorder: move workspace `id` to the post-removal slot `insert`.
    /// The live drag (ghost + drop slot) is component-local to `Tabs` via HTML5 drag
    /// events; only this durable commit crosses the action layer, so the new order
    /// autosaves.
    MoveTab {
        id: crate::session::WorkspaceId,
        insert: usize,
    },
    ToggleSidebar,
    CloseInspector,
    ToggleWindowFill,

    // ── bottom drawer (History + Events tabs) ──
    OpenHistory,
    OpenEvents,
    OpenProblems,
    SetLogTab(LogTab),
    OpenHistoryQuery(String),
    RunHistoryQuery(String),
    ToggleLog,
    ClearDrawer,
    ToggleLogRow(u64),
    RunExport(crate::model::ExportForm),
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
pub fn dispatch(action: Action) {
    let durable = is_durable(&action);
    let defs = durable && touches_defs(&action);
    run(action);
    if durable {
        if defs {
            projects::autosave();
        } else {
            projects::autosave_session();
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
            | MoveTab { .. }
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

fn run(action: Action) {
    use Action::*;
    match action {
        // query & results
        RunQuery => query::run(),
        RunExplain(analyze) => query::run_explain(analyze),
        ClearResults => query::clear_results(),
        SetResultsFind { ws, open } => query::set_results_find(ws, open),
        CancelQuery => query::cancel(),
        FetchPage(page) => query::fetch_page(page),
        SortColumn(ci) => query::sort_column(ci),
        SelectAll => query::select_all(),
        Copy => query::menu_copy(),
        CopySelection(fmt) => query::copy_selection(fmt),
        CopyRecord(row, fmt) => query::copy_record(row, fmt),
        LoadSelectStar(name) => query::select_star(&name),
        FormatSql => query::format(),
        ClearSql => query::clear(),
        SaveAsView => query::save_as_view(),
        SaveQuery => query::save(),
        OpenSavedQuery(name) => query::open_saved(&name),
        DeleteSavedQuery(name) => query::delete_saved(&name),
        SetResultSearch(q) => query::set_result_search(q),
        DismissQueryError => query::dismiss_error(),
        SetPlanTab(tab) => query::set_plan_tab(tab),
        TogglePlanRaw => query::toggle_plan_raw(),
        SetPageSize(sz) => query::set_page_size(sz),
        SetResultsView(v) => query::set_results_view(v),

        // tabs
        NewTab => tab::add(),
        SwitchTab(id) => tab::switch(id),
        CloseTab(id) => tab::close(id),
        CloseTabForce(id) => tab::close_force(id),
        CloseOtherTabs(id) => tab::close_others(id),
        CloseTabsRight(id) => tab::close_right(id),
        CloseAllTabs => tab::close_all(),
        ReopenTab => tab::reopen(),
        RenameTab(id, name) => tab::rename_tab(id, name),
        DuplicateTab(id) => tab::duplicate(id),

        // catalog
        OpenConfigNew => catalog::open_config_new(),
        OpenConfigEdit(name) => catalog::open_config_edit(&name),
        RegisterTable(draft) => catalog::register_table(draft),
        ConfirmRemove { kind, name } => catalog::confirm_remove(kind, name),
        EditView(name) => catalog::edit_view(&name),
        ToggleTableOpen(i) => crate::project::toggle_table_open(i),
        ToggleViewOpen(i) => crate::project::toggle_view_open(i),
        SelectColumn(col) => catalog::select_column(col),
        RescanCatalog => catalog::refresh(),
        ProfileTable(name) => catalog::profile(name),
        AskProfileTable(name) => crate::overlays::open_profile_confirm(name),
        ConfirmProfileTable(name) => catalog::confirm_profile(name),
        CancelProfileTable(name) => catalog::cancel_profile(name),
        OpenProfileSql(name) => catalog::open_profile_sql(name),

        MoveTab { id, insert } => tab::move_tab(id, insert),
        ToggleSidebar => crate::layout::toggle_sidebar(),
        CloseInspector => crate::layout::set_inspector_open(false),
        ToggleWindowFill => panel::toggle_window_fill(),

        // bottom drawer
        OpenHistory => overlay::open_history(),
        OpenEvents => overlay::open_events(),
        OpenProblems => overlay::open_problems(),
        SetLogTab(tab) => overlay::set_log_tab(tab),
        OpenHistoryQuery(sql) => overlay::open_history_query(sql),
        RunHistoryQuery(sql) => {
            overlay::open_history_query(sql);
            query::run();
        }
        ToggleLog => overlay::toggle_log(),
        ClearDrawer => overlay::clear_drawer(),
        ToggleLogRow(id) => overlay::toggle_log_row(id),
        RunExport(opts) => query::run_export(opts),

        // project (open/recent spawn new windows; close closes this window)
        OpenProject => projects::open_dir(),
        OpenRecent(path) => projects::open_recent(path),
        OpenChosen {
            new_window,
            remember,
        } => projects::choose_open(new_window, remember),
        SaveProject => projects::save(),
        CloseProject => projects::close(),
        CloseWindowForce => projects::close_now(),
    }
}
