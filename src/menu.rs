//! Native application menu (the macOS menu bar) — S11.
//!
//! Dioxus installs a default menu bar (App / Edit / Window + a debug dev-tools
//! Help menu); `Config::with_menu` **replaces** it wholesale. So we rebuild the
//! standard **App / Edit / Window** submenus ourselves — the Edit one from
//! `PredefinedMenuItem`s so system copy / paste / undo keep working — drop the
//! debug Help menu, and add a **File** menu (Open / Recent / Close / Save All /
//! Settings + New Query).
//!
//! The macOS menu is **app-global** and its events carry only the item id, so a
//! project window handles a File command *only when it is the focused window*
//! (`window::is_focused_window`); the id is relayed into a signal a `use_effect`
//! consumes, so the async open-folder dialog runs with a reactive scope. See
//! `app.rs`.
//!
//! Menu ids are a [`MenuCmd`] enum rather than loose strings: `MenuCmd: Into<MenuId>`
//! builds the item id and [`MenuCmd::parse`] recovers it from the event, so the
//! build-time and handle-time sides can't drift.

use std::cell::RefCell;

use dioxus::desktop::muda::{
    accelerator::Accelerator, IsMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem,
    Submenu,
};
use dioxus::prelude::*;

use crate::action::{dispatch, Action};

thread_local! {
    /// Handle to the live Edit-menu Select All item, so focus changes can grey / enable it.
    /// The macOS menu bar is app-global (the last-created window installs it — see
    /// [`app_menu`]); this holds that item. Main-thread only, like all menu ops.
    static SELECT_ALL_ITEM: RefCell<Option<MenuItem>> = RefCell::new(None);
}

/// Grey / enable the Edit-menu **Select All** item. Who can answer ⌘A now lives in
/// `keymap`'s responder registry (RustRover-style: enabled only in a scope that can answer
/// it); `keymap::claim_focus`/`release_focus` call this so the native item — which can't
/// reactively read a per-window signal — tracks it. The one keymap→menu edge.
pub fn set_select_all_enabled(enabled: bool) {
    SELECT_ALL_ITEM.with(|c| {
        if let Some(item) = c.borrow().as_ref() {
            item.set_enabled(enabled);
        }
    });
}

/// A command the native File menu can emit. Serialized to a [`MenuId`] when the
/// item is built and parsed back from the event id when it fires — one source of
/// truth for the id ↔ command mapping.
#[derive(Clone, PartialEq)]
pub enum MenuCmd {
    NewQuery,
    OpenProject,
    CloseProject,
    SaveAll,
    Settings,
    /// Select All (⌘A). A custom item — not `PredefinedMenuItem::select_all` — so the
    /// accelerator is intercepted at the AppKit level (before the webview swallows it) and
    /// routed by [`run_project_command`]: to grid cells when the results grid is focused,
    /// otherwise to the native web text selection.
    SelectAll,
    /// Copy (⌘C). Custom for the same reason as [`SelectAll`](MenuCmd::SelectAll): when the
    /// results grid is focused we copy the grid *selection* (Rz4, TSV), otherwise we re-emit
    /// the native `copy:` so text fields copy their own selection.
    Copy,
    /// Open a specific recent project (payload = its `.strata` path).
    OpenRecent(String),
}

const RECENT_PREFIX: &str = "file.recent:";

impl MenuCmd {
    /// The stable menu-id string for this command.
    fn id(&self) -> String {
        match self {
            MenuCmd::NewQuery => "file.new_query".into(),
            MenuCmd::OpenProject => "file.open_project".into(),
            MenuCmd::CloseProject => "file.close_project".into(),
            MenuCmd::SaveAll => "file.save_all".into(),
            MenuCmd::Settings => "file.settings".into(),
            MenuCmd::SelectAll => "edit.select_all".into(),
            MenuCmd::Copy => "edit.copy".into(),
            MenuCmd::OpenRecent(path) => format!("{RECENT_PREFIX}{path}"),
        }
    }

    /// Recover a command from a `MenuEvent` id, or `None` if it isn't ours (e.g. a
    /// predefined Edit / Window item, which muda handles itself).
    pub fn parse(id: &str) -> Option<MenuCmd> {
        Some(match id {
            "file.new_query" => MenuCmd::NewQuery,
            "file.open_project" => MenuCmd::OpenProject,
            "file.close_project" => MenuCmd::CloseProject,
            "file.save_all" => MenuCmd::SaveAll,
            "file.settings" => MenuCmd::Settings,
            "edit.select_all" => MenuCmd::SelectAll,
            "edit.copy" => MenuCmd::Copy,
            other => MenuCmd::OpenRecent(other.strip_prefix(RECENT_PREFIX)?.to_string()),
        })
    }
}

impl From<MenuCmd> for MenuId {
    fn from(c: MenuCmd) -> MenuId {
        MenuId(c.id())
    }
}

fn accel(s: &str) -> Option<Accelerator> {
    s.parse::<Accelerator>().ok()
}

/// Build the application menu: **App · File · Edit · Window**. Rebuilt at each
/// window creation (the macOS bar is global — last one wins; identical apart from
/// the recents snapshot).
pub fn app_menu() -> Menu {
    let menu = Menu::new();

    // App menu — macOS titles the first submenu with the app name automatically.
    let app = Submenu::new("Strata", true);
    let _ = app.append_items(&[
        &PredefinedMenuItem::about(Some("About Strata"), None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::services(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::hide(None),
        &PredefinedMenuItem::hide_others(None),
        &PredefinedMenuItem::show_all(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::quit(None),
    ]);

    // File — the S11 additions. No accelerators on items the webview already binds
    // (⌘T new-tab, ⌘W close-tab, ⌘, settings, ⌘S save-query) so those keep flowing
    // to `handle_key`; only the genuinely-new ⌘O / ⌥⌘S are bound here.
    let new_query = MenuItem::with_id(MenuCmd::NewQuery, "New Query", true, None);
    let open = MenuItem::with_id(
        MenuCmd::OpenProject,
        "Open Project…",
        true,
        accel("CmdOrCtrl+O"),
    );
    let recent = recent_submenu();
    let close = MenuItem::with_id(MenuCmd::CloseProject, "Close Project", true, None);
    let save_all = MenuItem::with_id(MenuCmd::SaveAll, "Save All", true, accel("Alt+CmdOrCtrl+S"));
    let settings = MenuItem::with_id(MenuCmd::Settings, "Settings…", true, None);
    let file = Submenu::new("File", true);
    let _ = file.append_items(&[
        &new_query,
        &PredefinedMenuItem::separator(),
        &open,
        &recent,
        &PredefinedMenuItem::separator(),
        &close,
        &save_all,
        &PredefinedMenuItem::separator(),
        &settings,
    ]);

    // Edit — rebuilt from predefined items so copy / paste / undo survive replacing
    // the default menu. Select All is a *custom* item (not the predefined one) so its ⌘A
    // is intercepted here before the webview and routed by `run_project_command` — the
    // grid claims it when focused, otherwise it falls back to the native text select-all.
    // Built disabled; the text surfaces' `onfocusin` handlers enable it while in scope
    // (RustRover-style). Stash the handle so those focus changes can toggle it.
    let select_all = MenuItem::with_id(MenuCmd::SelectAll, "Select All", false, accel("CmdOrCtrl+A"));
    SELECT_ALL_ITEM.with(|c| *c.borrow_mut() = Some(select_all.clone()));
    // Copy is likewise custom (not `PredefinedMenuItem::copy`) so ⌘C is intercepted before the
    // webview and routed by `run_project_command`: the grid copies its selection, text fields
    // re-emit native `copy:`. Always enabled (copy is valid in any focus).
    let copy = MenuItem::with_id(MenuCmd::Copy, "Copy", true, accel("CmdOrCtrl+C"));
    let edit = Submenu::new("Edit", true);
    let _ = edit.append_items(&[
        &PredefinedMenuItem::undo(None),
        &PredefinedMenuItem::redo(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::cut(None),
        &copy,
        &PredefinedMenuItem::paste(None),
        &select_all,
    ]);

    // Window — minimal standard set. We deliberately omit the predefined Close Window
    // (which carries ⌘W): RustRover-style, ⌘W closes the active *tab* (`CloseActiveTab`,
    // an OS global hotkey), so leaving Close Window here would steal the chord. The window
    // is still closable via the traffic-light button and File → Close Project.
    let window = Submenu::new("Window", true);
    let _ = window.append_items(&[
        &PredefinedMenuItem::minimize(None),
        &PredefinedMenuItem::maximize(None),
    ]);

    // On macOS the root menu accepts only submenus.
    let _ = menu.append_items(&[&app, &file, &edit, &window]);

    // Help — dev-only: a single "Component Gallery" entry (S28/S29 preview).
    // Compiled out of release builds, so the menu simply has no Help submenu there.
    #[cfg(debug_assertions)]
    {
        let help = Submenu::new("Help", true);
        let _ = menu.append_items(&[&help]);
    }

    menu
}

/// The **Open Recent** submenu, from the app config's recents (capped at 10).
fn recent_submenu() -> Submenu {
    let recent = Submenu::new("Open Recent", true);
    // Recents come straight from the app config, read at build time. The native menu
    // is imperative and can't live-update anyway (muda app-global — see F8).
    let recents = crate::config::load().recent_projects;
    if recents.is_empty() {
        let _ = recent.append(&MenuItem::new("No Recent Projects", false, None));
        return recent;
    }
    let items: Vec<MenuItem> = recents
        .iter()
        .take(10)
        .map(|r| {
            MenuItem::with_id(
                MenuCmd::OpenRecent(r.path.clone()),
                r.name.as_str(),
                true,
                None,
            )
        })
        .collect();
    let refs: Vec<&dyn IsMenuItem> = items.iter().map(|i| i as &dyn IsMenuItem).collect();
    let _ = recent.append_items(&refs);
    recent
}

/// Run a File-menu command in the focused project window. Called from a
/// `use_effect` (reactive scope present, so the open-folder dialog can spawn).
pub fn run_project_command(cmd: &MenuCmd) {
    match cmd {
        MenuCmd::NewQuery => dispatch(Action::NewTab),
        MenuCmd::OpenProject => dispatch(Action::OpenProject),
        MenuCmd::CloseProject => dispatch(Action::CloseProject),
        MenuCmd::SaveAll => dispatch(Action::SaveProject),
        MenuCmd::Settings => crate::window::spawn_settings_window(),
        // ⌘A / ⌘C route on the current focus scope, but that decision is the action layer's
        // (`query::select_all` / `query::menu_copy`) — the menu is a dumb adapter, like every
        // other arm here.
        MenuCmd::SelectAll => dispatch(Action::SelectAll),
        MenuCmd::Copy => dispatch(Action::Copy),
        MenuCmd::OpenRecent(path) => dispatch(Action::OpenRecent(path.clone())),
    }
}
