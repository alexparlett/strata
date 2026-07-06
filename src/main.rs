//! Strata — a local, Athena-style parquet query workspace.
//! Dioxus desktop UI + Apache DataFusion engine.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::OnceLock;

use dioxus::prelude::*;

mod action;
mod app;
mod config;
mod ddl;
mod engine;
mod overlays;
mod plan;
mod project;
mod query_error;
mod runs;
mod settings;
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
        Startup::Project(path) => {
            let cfg = window::project_window_config_for(&path);
            let _ = STARTUP_PATH.set(path);
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
    /// A project window for this `.strata` path (recent, or the dev sample).
    Project(String),
    /// The welcome/launcher window (no recent to reopen, non-dev build).
    Launcher,
}

/// Which window the app opens on launch: the most-recent project (when
/// "Reopen last project on startup" is on), else — in dev builds — the bundled
/// sample project, else the launcher.
fn decide_startup() -> Startup {
    let cfg = config::load();
    // "Reopen last project on startup" (System settings) gates reopening recents.
    if cfg.settings.reopen_on_startup {
        if let Some(recent) = cfg.most_recent() {
            return Startup::Project(recent.path.clone());
        }
    }
    #[cfg(feature = "sample-data")]
    {
        Startup::Project(
            concat!(env!("CARGO_MANIFEST_DIR"), "/sample/sample.strata").to_string(),
        )
    }
    #[cfg(not(feature = "sample-data"))]
    {
        Startup::Launcher
    }
}

/// Install a tracing subscriber. Defaults to `warn` for deps + `info` for this
/// crate; override with `RUST_LOG`. `try_init` is a no-op if a subscriber is
/// already installed.
fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,strata=info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
