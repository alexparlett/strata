//! Strata — a local, Athena-style parquet query workspace.
//! Dioxus desktop UI + Apache DataFusion engine.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use dioxus::prelude::*;

mod action;
mod app;
mod config;
mod ddl;
mod diagnostics;
mod engine;
mod engine_config;
mod hotkeys;
mod keymap;
mod menu;
mod overlays;
mod plan;
mod project;
mod query_error;
mod runs;
mod serialize;
mod session;
mod settings;
mod sql;
mod state;
mod theme;
mod ui;
mod util;
mod window;
// Syntax highlighting is provided by `dioxus-code` (Code / CodeEditor) — the
// former hand-rolled `highlight.rs` is no longer used.

/// The Strata design system, injected as a <style> in each window root.
pub const CSS: &str = include_str!("../assets/main.css");

/// The project the first window opens with — set once before launch, read by
/// [`root_entry`]. Subsequent windows carry their path as a component prop.
static STARTUP_PATH: OnceLock<String> = OnceLock::new();

/// The *additional* projects to reopen on startup (beyond the first window). The
/// first `ProjectRoot` spawns a window for each, once — see [`spawn_startup_rest`].
static STARTUP_REST: OnceLock<Vec<String>> = OnceLock::new();
static REST_SPAWNED: AtomicBool = AtomicBool::new(false);

/// Spawn windows for the additional startup projects (beyond the first), exactly
/// once. Called from the first `ProjectRoot` after it mounts (creating windows
/// needs a running desktop context, which `main` doesn't have yet).
pub fn spawn_startup_rest() {
    if REST_SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(rest) = STARTUP_REST.get() {
        for path in rest {
            window::spawn_project_window(path.clone());
        }
    }
}

fn main() {
    init_logging();
    // Clear any snapshots left over from a previous run — once, before any engine
    // window exists. At runtime each window's engine only cleans its own scope.
    engine::purge_snapshot_root();
    match decide_startup() {
        // The window chrome (transparent macOS titlebar, child-window webview,
        // dark background) + the project's saved size/position live in
        // `window::project_window_config_for`. See wry#1056 for
        // `with_as_child_window`.
        Startup::Projects(mut paths) => {
            let first = paths.remove(0);
            let cfg = window::project_window_config_for(&first);
            let _ = STARTUP_PATH.set(first);
            let _ = STARTUP_REST.set(paths);
            LaunchBuilder::new().with_cfg(cfg).launch(root_entry);
        }
        // Nothing to reopen → open the launcher as the first window (same window
        // shown when "Close project" closes the last project window).
        Startup::Launcher => {
            let cfg = window::launcher_window_config();
            LaunchBuilder::new()
                .with_cfg(cfg)
                .launch(ui::launcher::LauncherRoot);
        }
    }
}

/// Root of the first window: a project window carrying [`STARTUP_PATH`].
fn root_entry() -> Element {
    let open_path = STARTUP_PATH.get().cloned().unwrap_or_default();
    rsx! { app::ProjectRoot { open_path } }
}

/// What the app opens on launch.
enum Startup {
    /// Reopen these `.strata` project paths (the first as the launch window, the
    /// rest spawned by it) — the set that had windows open at the last quit.
    Projects(Vec<String>),
    /// The welcome/launcher window (nothing to reopen).
    Launcher,
}

/// Which window(s) the app opens on launch: when "Reopen projects on startup" is
/// on, reopen every project that had a window open at the last quit (filtered to
/// ones that still exist on disk); otherwise the launcher.
fn decide_startup() -> Startup {
    let cfg = config::load();
    if cfg.settings.reopen_on_startup {
        let paths: Vec<String> = cfg
            .open_projects
            .iter()
            .filter(|p| project::Project::exists_at(std::path::Path::new(p)))
            .cloned()
            .collect();
        if !paths.is_empty() {
            return Startup::Projects(paths);
        }
    }
    Startup::Launcher
}

/// Install a tracing subscriber. Defaults to `warn` for deps + `info` for this
/// crate; override with `RUST_LOG`. `try_init` is a no-op if a subscriber is
/// already installed.
fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,strata=info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
