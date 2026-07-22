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
mod theme;
pub mod components;

fn main() {
    // Clear snapshot leftovers from a previous crashed run (each live engine only ever
    // cleans its own subdirectory — safe only here, before any engine exists).
    strata_core::engine::purge_snapshot_root();
    launch(
        LaunchConfig::new()
            .with_window(
                ProjectApp::window()
            ),
    );
}
