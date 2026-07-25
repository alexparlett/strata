//! Strata — the Freya (Skia / native) frontend. The Freya-port target; rides the
//! shared `strata-core` alongside the transitional `strata-dioxus` app. See
//! `docs/FREYA_PORT_PLAN.md` (§3 for this crate's internal layout).
//!
//! Layout grows per phase: `apps/<window>/` holds one self-contained OS window each
//! (the project window and the launcher today), `platform/` the window model that spawns
//! and focuses between them. Top-level `state/` (global singletons), `components/` (DS
//! widgets) and `theme.rs` are shared by every window.
//!
//! No Tokio runtime here on purpose: the engine facade owns a private runtime, and the
//! UI just awaits its methods (`JoinHandle`s are executor-agnostic) — see
//! `strata_core::engine` and `docs/SNAPSHOT_SPEC.md` §7.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use apps::launcher::LauncherApp;
use apps::project::ProjectApp;
use freya::prelude::*;
use strata_core::config::AppConfig;
use strata_core::engine::purge_snapshot_root;
use strata_core::project as project_io;

use crate::platform::create_global_windows;
use crate::state::{create_global_config, AppCtx};
use crate::theme::ThemesCtx;

mod apps;
pub mod components;
mod keymap;
mod menu;
mod platform;
mod state;
mod theme;

fn main() {
    // Clear snapshot leftovers from a previous crashed run (each live engine only ever
    // cleans its own subdirectory — safe only here, before any engine exists).
    purge_snapshot_root();
    // Discover the theme registry once (built-ins + the user themes dir) — every window
    // shares this one handle via context.
    let themes = ThemesCtx::discover();
    // The app-global **reactive config**: the whole `AppConfig` — settings, recents, and
    // the open-project set — loaded from disk once here and shared by every window. Disk
    // is a startup input, never a live source: from now on this store is the truth and
    // `write_config` is the only thing that writes the file. Writes are per-channel, so a
    // project opening wakes the recents readers without touching the theme.
    //
    // The theme itself is pure *derived* state: each window's `use_strata_theme` resolves
    // the settings selection (+ OS appearance while Sync-with-OS is on, via Freya's
    // per-window `Platform.preferred_theme`) through the shared registry — no stored
    // applied-theme id to keep coherent.
    let (config, reopen) = create_global_config();
    // …and the app-global **live** window registry: which windows exist right now, so a
    // project that already has one is focused rather than opened twice.
    let windows = create_global_windows();
    // The menubar. Its builder runs at `resumed`, on this very thread, and hands back the
    // File menu's handles — which land in a third app-global so the focused window can keep
    // Open Recent and Close Project pointed at itself (`menu::use_file_menu`). The builder
    // captures the resolved chords rather than the config handle, since accelerators are
    // read once; the event *handler* holds the live handles, so dispatch resolves current
    // bindings and can open a recent straight from the renderer.
    let menu_chords = menu::menu_chords(&config.peek().settings);
    let menu_state = menu::create_global_menu();
    // Everything a window — or the menubar handler — is handed, in one value.
    let app = AppCtx {
        themes,
        config,
        windows,
        menu: menu_state,
    };
    let menu_app = app.clone();
    let launch_config = LaunchConfig::new()
        // The muda menubar replaces winit's default menu at resume. Crucially its
        // Quit is a *custom* item routed through the close-request path (red-button
        // semantics, T2 confirm keeps its say) — winit's own Quit sent Cocoa's
        // `terminate:` directly, swallowing ⌘Q before the keymap AND bypassing the
        // `on_close` veto. (Known gap: a Dock-icon "Quit" still `terminate:`s
        // un-vetoed — winit 0.30 exposes no `applicationShouldTerminate`; its 0.31
        // "bring your own app delegate" closes this, see P6-02.)
        .with_menu(
            move || {
                let (menu, handles) = menu::app_menu(menu_chords);
                let mut menu_state = menu_state;
                menu_state.set(Some(handles));
                menu
            },
            move |event, ctx| menu::handle_menu_event(event, ctx, menu_app.clone()),
        );
    // One window per project to restore, or the launcher. `with_window` may be called any
    // number of times, so the whole restore set opens as the app's initial windows — no
    // first-window-spawns-the-rest dance.
    let launch_config = match startup(&config.peek(), reopen) {
        Startup::Projects(roots) => roots.into_iter().fold(launch_config, |cfg, root| {
            cfg.with_window(ProjectApp::window(app.clone(), root))
        }),
        Startup::Launcher => launch_config.with_window(LauncherApp::window(app)),
    };
    launch(launch_config);
}

/// What the app opens on launch.
enum Startup {
    /// Reopen these project folders, one window each — the set that had a window at the
    /// last quit (or a folder named on the command line).
    Projects(Vec<PathBuf>),
    /// The welcome window: nothing to reopen.
    Launcher,
}

/// Decide the launch windows, RustRover's rule: when "Reopen projects on startup" is on,
/// reopen **every** project that had a window at the last quit (filtered to the ones still
/// on disk); otherwise show the welcome window. A folder named on the command line
/// (`strata path/to/project`) wins outright — that's an explicit "open this".
///
/// `reopen` is the persisted open-set, taken out of the store by `create_global_config`:
/// stale by definition once the process is running, since windows re-add themselves as they
/// open. Note that only a *quit* leaves it populated — closing every window by hand empties
/// it, which is what makes "I closed everything" mean "start me at the launcher".
///
/// A path that won't resolve is reported and skipped rather than fatal: a project folder
/// that has been moved or deleted since the last run is ordinary, and the launcher is a
/// perfectly good place to land.
fn startup(config: &AppConfig, reopen: Vec<String>) -> Startup {
    if let Some(arg) = env::args().nth(1) {
        // Through the shared normalisation, like every other open path: naming a project's
        // own `.strata` directory opens the project, not a fresh one scaffolded inside it.
        return match platform::resolve_project_folder(Path::new(&arg)) {
            Some(root) => Startup::Projects(vec![root]),
            None => Startup::Launcher,
        };
    }
    if config.settings.reopen_on_startup {
        let roots: Vec<PathBuf> = reopen
            .iter()
            .filter_map(|path| match fs::canonicalize(path) {
                // A folder that no longer holds a project isn't reopened — restoring a
                // window would silently scaffold a fresh `.strata/` into it.
                Ok(root) if project_io::exists_at(&root) => Some(root),
                Ok(root) => {
                    tracing::warn!("not reopening `{}`: no project there", root.display());
                    None
                }
                Err(e) => {
                    tracing::warn!("not reopening `{path}`: {e}");
                    None
                }
            })
            .collect();
        if !roots.is_empty() {
            return Startup::Projects(roots);
        }
    }
    Startup::Launcher
}
