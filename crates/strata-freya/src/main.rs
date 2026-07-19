//! Strata — the Freya (Skia / native) frontend. The Freya-port target; rides the
//! shared `strata-core` alongside the transitional `strata-dioxus` app. See
//! `docs/FREYA_PORT_PLAN.md` (§3 for this crate's internal layout).
//!
//! Layout grows per phase: `apps/<window>/` holds one self-contained OS window each
//! (Phase 1 = the project window). Top-level `state/` (global singletons), `engine/`
//! (bridge), `components/` (DS widgets), `theme.rs`, and `platform/` come online as the
//! phase that needs them lands.
//!
//! No Tokio runtime here on purpose: the engine owns its own runtime on its own thread,
//! and the UI only does non-blocking `cmd_tx.send()` + awaits the engine's `tokio::sync`
//! event channel (executor-agnostic) under Freya's `spawn`.

use apps::project::ProjectApp;
use freya::prelude::*;

mod apps;

fn main() {
    launch(LaunchConfig::new().with_window(WindowConfig::new_app(ProjectApp)));
}
