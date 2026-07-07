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
    let project = if Project::exists_at(&path) {
        match Project::load_from_dir(&path) {
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
        // Scaffold a new project named after the containing folder. Workspaces
        // live in the reactive session store (not on `Project`), so seed it with a
        // single blank workspace here — the load path does this inside
        // `Project::load_from_dir`, the scaffold path must do it explicitly.
        crate::session::reset_blank();
        let mut p = Project::empty();
        p.name = path
            .parent()
            .and_then(|f| f.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_string());
        if let Err(e) = p.save_all(&path) {
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
        let store = crate::settings::SETTINGS.resolve();
        let s = store.peek();
        (s.open_pref, s.default_project_dir.clone())
    };
    spawn(async move {
        let mut dialog = rfd::AsyncFileDialog::new();
        if let Some(dir) = expand_tilde(default_dir.trim()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(handle) = dialog.pick_folder().await {
            let path = crate::window::resolve_project_dir(handle.path());
            open_with_pref(state, path, open_pref);
        }
    });
}

/// `Action::OpenRecent` — open a recent project, honouring the open preference
/// (This window / New window / Ask), like [`open_dir`].
pub fn open_recent(state: Signal<AppState>, path: String) {
    let pref = crate::settings::SETTINGS.resolve().peek().open_pref;
    open_with_pref(state, PathBuf::from(path), pref);
}

/// Route an open to the current window, a new window, or the "ask" prompt (B10),
/// per the resolved open preference.
fn open_with_pref(state: Signal<AppState>, path: PathBuf, pref: crate::config::OpenPref) {
    use crate::config::OpenPref;
    match pref {
        OpenPref::This => open_in_current(state, path),
        OpenPref::New => crate::window::spawn_project_window(path.to_string_lossy().into_owned()),
        OpenPref::Ask => crate::overlays::open_open_prompt(path),
    }
}

/// `Action::OpenChosen` — resolve the open-target prompt (B10): open the pending
/// project here or in a new window, optionally remembering the choice as the pref.
pub fn choose_open(state: Signal<AppState>, new_window: bool, remember: bool) {
    let path = crate::overlays::OVERLAYS.resolve().read().open_prompt.clone();
    crate::overlays::close_open_prompt();
    let Some(path) = path else {
        return;
    };
    let pref = if new_window {
        crate::config::OpenPref::New
    } else {
        crate::config::OpenPref::This
    };
    if remember {
        crate::settings::set_open_pref(pref);
    }
    if new_window {
        crate::window::spawn_project_window(path.to_string_lossy().into_owned());
    } else {
        open_in_current(state, path);
    }
}

/// Open `path` in *this* window, replacing the project in place ("This window"
/// open preference): save the outgoing project, clear its catalog from this
/// window's engine, then load the new one.
pub fn open_in_current(state: Signal<AppState>, path: PathBuf) {
    save(state);
    // The current window is being repurposed for `path`; mark the old project
    // closed (`install` marks the new one open).
    if let Some(old) = state.read().project_path.clone() {
        crate::config::mark_closed(&old.to_string_lossy());
    }
    let (tx, tables, views) = {
        let s = state.read();
        (
            s.cmd_tx.clone(),
            s.project
                .tables
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<String>>(),
            s.project
                .views
                .iter()
                .map(|v| v.name.clone())
                .collect::<Vec<String>>(),
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
    write_files(state, true);
}

/// Save from the window-close handler (`CloseRequested`), where the dioxus scope
/// isn't available — geometry is read from the window registry *by id* rather
/// than `window()`. Does not open the launcher (an OS close never does).
pub fn save_on_close(mut state: Signal<AppState>, win_id: WindowId) {
    if let Some(geom) = crate::window::window_geom_by_id(win_id) {
        state.write().project.window = Some(geom);
    }
    write_files(state, true);
    if let Some(dir) = state.read().project_path.clone() {
        crate::config::mark_closed(&dir.to_string_lossy());
    }
}

/// Write the project to its `.strata/` dir: both files when `defs` (a definition
/// changed), else only `session.json` (a tab/history/geometry change — keeps the
/// committed `project.json` quiet). No-op if no project is backed on disk.
fn write_files(state: Signal<AppState>, defs: bool) {
    let (path, result) = {
        let s = state.read();
        match &s.project_path {
            Some(dir) => (
                Some(dir.clone()),
                if defs {
                    s.project.save_all(dir)
                } else {
                    s.project.save_session(dir)
                },
            ),
            None => (None, Ok(())),
        }
    };
    if let (Some(path), Err(e)) = (path, result) {
        tracing::error!("save project {}: {e}", path.display());
    }
}

/// Autosave after a durable change that touched **definitions** — both files.
pub fn autosave(mut state: Signal<AppState>) {
    if state.read().project_path.is_none() {
        return;
    }
    if let Some(geom) = crate::window::current_window_geom() {
        state.write().project.window = Some(geom);
    }
    write_files(state, true);
}

/// Autosave after a **session-only** durable change (tabs / history) — writes just
/// `session.json`, leaving the committed `project.json` untouched.
pub fn autosave_session(mut state: Signal<AppState>) {
    if state.read().project_path.is_none() {
        return;
    }
    if let Some(geom) = crate::window::current_window_geom() {
        state.write().project.window = Some(geom);
    }
    write_files(state, false);
}

/// Persist live editor edits to `session.json`. The controlled `CodeEditor` writes
/// its workspace's `sql` lens directly (bypassing `dispatch`, so no autosave
/// fires), so this effect subscribes to the reactive [`crate::session`] store and
/// writes a (debounced) `session.json` whenever it changes — including a keystroke.
/// Mounted once in the root project component (`ProjectRoot`).
pub fn use_persist_session(state: Signal<AppState>) {
    // A generation counter so a burst of edits collapses into one write: each
    // change bumps it, the spawned task writes only if it's still the latest.
    let mut gen = use_signal(|| 0u64);
    use_effect(move || {
        // Subscribe to this window's session store (structural + per-field lens
        // writes both re-run this effect). Bind the store first so the read guard
        // doesn't outlive a temporary.
        let store = crate::session::store();
        let _sub = store.read();
        // No-op until the project is backed on disk.
        if state.peek().project_path.is_none() {
            return;
        }
        let g = {
            let mut w = gen.write();
            *w += 1;
            *w
        };
        spawn(async move {
            // Debounce: coalesce a run of keystrokes into a single write.
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            if *gen.peek() != g {
                return; // superseded by a newer edit
            }
            write_files(state, false);
        });
    });
}

/// `Action::CloseProject` — save, then close this window. If it's the last
/// project window, open the launcher; otherwise focus a sibling. An OS
/// close-button doesn't route here, so it never opens the launcher.
pub fn close(state: Signal<AppState>) {
    save(state);
    if let Some(dir) = state.read().project_path.clone() {
        crate::config::mark_closed(&dir.to_string_lossy());
    }
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
        // The workspaces for the incoming project were already loaded into the
        // reactive session store (`load_from_dir` / `reset_blank`); drop the
        // previous project's runs so a reused id can't inherit stale results.
        crate::runs::clear();
        s.set_status(LogKind::Ok, format!("Opened project '{name}'"));
    }

    if let Some(tx) = &tx {
        for spec in specs {
            let _ = tx.send(Command::Register(spec));
        }
        for (view_name, sql) in views {
            let _ = tx.send(Command::CreateView {
                name: view_name,
                sql,
            });
        }
    }

    // Record it in recents (per-machine app config). The file already exists on
    // disk; later durable edits autosave through `dispatch`.
    let mut cfg = config::load();
    cfg.push_recent(&name, &path.to_string_lossy());
    cfg.add_open(&path.to_string_lossy());
    config::save(&cfg);
    state.write().recent_projects = cfg.recent_projects;
}
