//! Project-management action handlers.
//!
//! In the multi-window model each project lives in its own window (`ProjectRoot`
//! → own engine), so *opening* a project spawns a **new window**; only a window's
//! own startup loads a project in place ([`load_current`]). "Close project"
//! closes the window, reopening the launcher only if it was the last one.

use std::path::PathBuf;

// Glob for the `Readable`/`Writable` traits (`.read()`/`.write()`); this module
// doesn't reference `Action`, so there's no prelude collision.
use dioxus::prelude::*;

use dioxus::desktop::tao::window::WindowId;

use crate::config;
use crate::engine::{Command, TableSpec};
use crate::project::Project;
use crate::state::{AppState, LogKind};

/// Load a project into *this* window (startup / freshly-spawned window). A
/// missing file becomes a new project scaffolded from the folder name.
pub fn load_current(mut state: Signal<AppState>, path: PathBuf) {
    let project = if path.exists() {
        match Project::load(&path) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("open project {}: {e}", path.display());
                state
                    .write()
                    .set_status(LogKind::Error, format!("Couldn't open project: {e}"));
                return;
            }
        }
    } else {
        let mut p = Project::empty();
        p.name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_string());
        if let Err(e) = p.save(&path) {
            tracing::error!("create project {}: {e}", path.display());
        }
        p
    };
    install(state, project, path);
}

/// `Action::OpenProject` — pick a folder and open its project, honouring the
/// **Opening a project** preference (System settings): *This window* loads it in
/// place, *New window* / *Ask* spawn a window. The picker starts in the
/// configured **default project directory** when one is set. Async, because a
/// blocking `rfd` dialog would re-enter the renderer and panic.
pub fn open_dir(state: Signal<AppState>) {
    let (open_pref, default_dir) = {
        let s = state.read();
        (s.open_pref.clone(), s.default_project_dir.clone())
    };
    spawn(async move {
        let mut dialog = rfd::AsyncFileDialog::new();
        if let Some(dir) = expand_tilde(default_dir.trim()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(handle) = dialog.pick_folder().await {
            let path = crate::window::resolve_project_file(handle.path());
            if open_pref == "this" {
                open_in_current(state, path);
            } else {
                crate::window::spawn_project_window(path.to_string_lossy().into_owned());
            }
        }
    });
}

/// `Action::OpenRecent` — open a recent project in a **new window**.
pub fn open_recent(path: String) {
    crate::window::spawn_project_window(path);
}

/// Open `path` in *this* window, replacing the project in place ("This window"
/// open preference): save the outgoing project, clear its catalog from this
/// window's engine, then load the new one.
pub fn open_in_current(state: Signal<AppState>, path: PathBuf) {
    save(state);
    let (tx, tables, views) = {
        let s = state.read();
        (
            s.cmd_tx.clone(),
            s.project.tables.iter().map(|t| t.name.clone()).collect::<Vec<String>>(),
            s.project.views.iter().map(|v| v.name.clone()).collect::<Vec<String>>(),
        )
    };
    if let Some(tx) = &tx {
        for name in views {
            let _ = tx.send(Command::DropView { name });
        }
        for table in tables {
            let _ = tx.send(Command::Deregister { table });
        }
    }
    load_current(state, path);
}

/// Expand a leading `~` to `$HOME`; `None` for an empty path.
fn expand_tilde(p: &str) -> Option<PathBuf> {
    if p.is_empty() {
        return None;
    }
    if let Some(rest) = p.strip_prefix('~') {
        if let Some(home) = std::env::var_os("HOME") {
            return Some(PathBuf::from(home).join(rest.trim_start_matches('/')));
        }
    }
    Some(PathBuf::from(p))
}

/// Save the current project to its file (no-op if it isn't backed by one yet).
/// Captures the window's current size + position first so the project reopens
/// where it was left.
pub fn save(mut state: Signal<AppState>) {
    if let Some(geom) = crate::window::current_window_geom() {
        state.write().project.window = Some(geom);
    }
    write_project(state);
}

/// Save from the window-close handler (`CloseRequested`), where the dioxus scope
/// isn't available — geometry is read from the window registry *by id* rather
/// than `window()`. Does not open the launcher (an OS close never does).
pub fn save_on_close(mut state: Signal<AppState>, win_id: WindowId) {
    if let Some(geom) = crate::window::window_geom_by_id(win_id) {
        state.write().project.window = Some(geom);
    }
    write_project(state);
}

/// Write the project to its file (no geometry capture — the caller sets it).
fn write_project(state: Signal<AppState>) {
    let (path, result) = {
        let s = state.read();
        match &s.project_path {
            Some(p) => (Some(p.clone()), s.project.save(p)),
            None => (None, Ok(())),
        }
    };
    if let (Some(path), Err(e)) = (path, result) {
        tracing::error!("save project {}: {e}", path.display());
    }
}

/// Autosave after a durable change (only when a project is open on disk).
pub fn autosave(state: Signal<AppState>) {
    if state.read().project_path.is_some() {
        save(state);
    }
}

/// `Action::CloseProject` — save, then close this window. If it's the last
/// project window, open the launcher; otherwise focus a sibling. An OS
/// close-button doesn't route here, so it never opens the launcher.
pub fn close(state: Signal<AppState>) {
    save(state);
    if crate::window::project_window_count() <= 1 {
        crate::window::open_launcher_window();
    } else {
        crate::window::focus_another_window();
    }
    dioxus::desktop::window().close();
}

// ---- internals ----

/// Install `project` into this window's state and register its catalog with the
/// window's (fresh) engine, then record it in recents.
fn install(mut state: Signal<AppState>, project: Project, path: PathBuf) {
    let tx = state.read().cmd_tx.clone();

    // Registration commands for the incoming catalog (built before the move).
    // Stored sources may be relative to the project dir; resolve to absolute for
    // the engine.
    let base = path.parent().map(|p| p.to_path_buf());
    let specs: Vec<TableSpec> = project
        .tables
        .iter()
        .map(|t| TableSpec {
            name: t.name.clone(),
            paths: t
                .sources
                .iter()
                .map(|s| crate::action::catalog::resolve_source(base.as_deref(), s))
                .collect(),
            format: t.format.clone(),
            // Persisted `(name, type)` partition spec — types survive reload.
            partitions: t.partition_cols.clone(),
        })
        .collect();
    let views: Vec<(String, String)> = project
        .views
        .iter()
        .map(|v| (v.name.clone(), v.sql.clone()))
        .collect();
    let name = project.name.clone();

    {
        let mut s = state.write();
        s.project = project;
        s.project_path = Some(path.clone());
        s.result = None;
        s.set_status(LogKind::Ok, format!("Opened project '{name}'"));
    }

    if let Some(tx) = &tx {
        for spec in specs {
            let _ = tx.send(Command::Register(spec));
        }
        for (view_name, sql) in views {
            let _ = tx.send(Command::CreateView { name: view_name, sql });
        }
    }

    // Record it in recents (per-machine app config). The file already exists on
    // disk; later durable edits autosave through `dispatch`.
    let mut cfg = config::load();
    cfg.push_recent(&name, &path.to_string_lossy());
    config::save(&cfg);
    state.write().recent_projects = cfg.recent_projects;
}
