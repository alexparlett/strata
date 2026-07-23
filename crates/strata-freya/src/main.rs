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

mod apps;
mod keymap;
mod menu;
mod theme;
pub mod components;

fn main() {
    // Clear snapshot leftovers from a previous crashed run (each live engine only ever
    // cleans its own subdirectory — safe only here, before any engine exists).
    strata_core::engine::purge_snapshot_root();
    // Discover the theme registry once (built-ins + the user themes dir) — every window
    // shares this one handle via context.
    let themes = crate::theme::ThemesCtx::discover();
    // The app-global **reactive settings**: loaded from disk once here, then written only
    // by UI (the Phase 4 Settings window — which also persists via `config::save`; disk is
    // a startup input, never a live source). Any write repaints every window that reads
    // the changed field. The theme is pure *derived* state: each window's
    // `use_strata_theme` resolves the selection (+ OS appearance while Sync-with-OS is
    // on, via Freya's per-window `Platform.preferred_theme`) through the shared registry
    // — no stored applied-theme id to keep coherent.
    let settings = State::create_global(strata_core::config::load().settings);
    // The menubar builds on the event loop thread (`Send` closure), so it captures the
    // resolved chords — plain data — not the settings handle. The event *handler* runs
    // on the renderer (main) thread and does capture `settings`, so Edit dispatch
    // resolves live bindings.
    let menu_chords = menu::menu_chords(&settings.peek());
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
                move |event, ctx| menu::handle_menu_event(event, ctx, settings),
            )
            .with_window(
                ProjectApp::window(themes, settings)
            ),
    );
}
