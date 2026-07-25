//! Strata — the Freya (Skia / native) frontend. The Freya-port target; rides the
//! shared `strata-core` alongside the transitional `strata-dioxus` app. See
//! `docs/FREYA_PORT_PLAN.md` (§3 for this crate's internal layout).
//!
//! Layout grows per phase: `apps/<window>/` holds one self-contained OS window each
//! (Phase 1 = the project window). Top-level `state/` (global singletons), `engine/`
//! (bridge), `components/` (DS widgets), `theme.rs`, and `platform/` come online as the
//! phase that needs them lands.
//!
//! No Tokio runtime here on purpose: the engine facade owns a private runtime, and the
//! UI just awaits its methods (`JoinHandle`s are executor-agnostic) — see
//! `strata_core::engine` and `docs/SNAPSHOT_SPEC.md` §7.

use apps::project::ProjectApp;
use freya::prelude::*;
use strata_core::engine::purge_snapshot_root;

use crate::state::create_global_config;
use crate::theme::ThemesCtx;

mod apps;
pub mod components;
mod keymap;
mod menu;
mod state;
mod theme;

fn main() {
    // First thing: nothing logged before this exists. Every `tracing::*` call in the app
    // and in `strata-core` is a no-op until a subscriber is installed.
    init_logging();
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
    //
    // `reopen` is the set of projects that had a window at the last exit — the
    // "Reopen projects on startup" list. Restoring it needs multi-window spawn (P4-01);
    // until then the launch window is `resolve_launch_root`'s and this is only reported.
    let (config, reopen) = create_global_config();
    tracing::debug!("reopen-on-startup candidates: {reopen:?}");
    // The menubar builds on the event loop thread (`Send` closure), so it captures the
    // resolved chords — plain data — not the config handle. The event *handler* runs
    // on the renderer (main) thread and does capture `config`, so Edit dispatch
    // resolves live bindings.
    let menu_chords = menu::menu_chords(&config.peek().settings);
    launch(
        LaunchConfig::new()
            // The muda menubar replaces winit's default menu at resume. Crucially its
            // Quit is a *custom* item routed through the close-request path (red-button
            // semantics, T2 confirm keeps its say) — winit's own Quit sent Cocoa's
            // `terminate:` directly, swallowing ⌘Q before the keymap AND bypassing the
            // `on_close` veto. (Known gap: a Dock-icon "Quit" still `terminate:`s
            // un-vetoed — winit 0.30 exposes no `applicationShouldTerminate`; its 0.31
            // "bring your own app delegate" closes this, see P6-02.)
            .with_menu(
                move || menu::app_menu(menu_chords),
                move |event, ctx| menu::handle_menu_event(event, ctx, config),
            )
            .with_window(ProjectApp::window(themes, config)),
    );
}

/// Install a tracing subscriber. Defaults to `warn` for deps + `info` for every `strata*`
/// crate (`EnvFilter` matches targets by prefix, so one directive covers `strata_freya`,
/// `strata_core`, `strata_model`, …); override with `RUST_LOG`. `try_init` is a no-op if a
/// subscriber is already installed.
fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,strata=info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
