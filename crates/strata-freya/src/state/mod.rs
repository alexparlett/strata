//! **App-global** state — the cross-window singletons, and nothing else. Per-window model
//! lives in each window's own Radio stores under `apps/<window>/state/`
//! (`docs/FREYA_STATE_ARCHITECTURE.md` §2).
//!
//! Today that is one store: the machine-global [`AppConfig`](strata_core::config::AppConfig)
//! — settings, recents, and the open-project set.

mod config;

pub use config::*;
