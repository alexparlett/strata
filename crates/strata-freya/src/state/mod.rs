//! **App-global** state — the cross-window singletons, and nothing else. Per-window model
//! lives in each window's own Radio stores under `apps/<window>/state/`
//! (`docs/FREYA_STATE_ARCHITECTURE.md` §2).
//!
//! Three of them, all created in `main` before `launch` and handed to every window root as
//! one [`AppCtx`]:
//!
//! - the machine-global [`AppConfig`](strata_core::config::AppConfig) store — settings,
//!   recents, the open-project set ([`config`]);
//! - the live window registry — which windows exist and what they show
//!   ([`crate::platform::windows`]);
//! - the menubar's mutable handles, so the focused window can keep the File menu pointed at
//!   itself ([`crate::menu`]) — plus the open path it points Open Recent at
//!   ([`crate::platform::open`]).
//!
//! Plus the theme registry, which is immutable after discovery and so is a plain `Arc`
//! rather than a store.

mod config;

pub use config::*;

use crate::menu::MenuState;
use crate::platform::{FocusedOpen, WindowRegistry};
use crate::theme::ThemesCtx;

/// Everything `main` creates once and every window needs: the app-globals plus the shared
/// theme registry.
///
/// One struct rather than four parameters because they travel together everywhere — a
/// window root, the window-open path, the menubar handler — and threading them
/// individually turned every one of those signatures into plumbing. A window root reads
/// what it needs off this and shares the rest into its tree (`use_share_config`).
#[derive(Clone)]
pub struct AppCtx {
    pub themes: ThemesCtx,
    pub config: ConfigStation,
    pub windows: WindowRegistry,
    pub menu: MenuState,
    /// The focused window's open path, parked by `use_file_menu` — see [`FocusedOpen`]. It
    /// sits beside `menu` because it exists for the same reason: the File menu is app-global
    /// but its contents belong to one window, and Open Recent is the item that carries data
    /// rather than a chord.
    pub open: FocusedOpen,
}

/// Every field is a handle on a process-wide singleton created before the first window, so
/// two `AppCtx`s are always the same four handles — which is exactly what a component's
/// diff should conclude when it holds one as a prop. Reactivity comes from the *stores*
/// (subscribe to a channel), never from the handle changing.
///
/// Written out because `RadioStation` has no `PartialEq` of its own, and a derive would
/// therefore be impossible rather than merely different.
impl PartialEq for AppCtx {
    fn eq(&self, other: &Self) -> bool {
        self.themes == other.themes
    }
}
