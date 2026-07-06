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
mod plan;
mod project;
mod query_error;
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
    let startup = decide_startup();
    // The window chrome (transparent macOS titlebar, child-window webview, dark
    // background) + the project's saved size/position live in
    // `window::project_window_config_for`. See wry#1056 for `with_as_child_window`.
    let cfg = window::project_window_config_for(&startup);
    let _ = STARTUP_PATH.set(startup);
    LaunchBuilder::new().with_cfg(cfg).launch(root_entry);
}

/// Root of the first window: a project window carrying [`STARTUP_PATH`].
fn root_entry() -> Element {
    let open_path = STARTUP_PATH.get().cloned().unwrap_or_default();
    rsx! { app::ProjectRoot { open_path } }
}

/// Which project the app opens on launch: the most-recent one, else (dev builds)
/// the bundled sample, else an empty untitled project. The launcher is never the
/// startup window — it only appears when "Close project" closes the last window.
fn decide_startup() -> String {
    let cfg = config::load();
    // "Reopen last project on startup" (System settings) gates reopening recents.
    if cfg.reopen_on_startup {
        if let Some(recent) = cfg.most_recent() {
            return recent.path.clone();
        }
    }
    #[cfg(feature = "sample-data")]
    {
        concat!(env!("CARGO_MANIFEST_DIR"), "/sample/sample.strata").to_string()
    }
    #[cfg(not(feature = "sample-data"))]
    {
        String::new()
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
