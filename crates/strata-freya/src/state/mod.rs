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
//!   ([`crate::platform::open`]);
//! - the Settings window's live theme preview ([`theme_preview`]) — the one half of its
//!   uncommitted draft that every *other* window has to read;
//! - the model listings satellite ([`listings`]) — what each provider last reported, which
//!   both the Settings model picker and the chat composer choose from.
//!
//! Plus the theme registry, which is immutable after discovery and so is a plain `Arc`
//! rather than a store.

mod config;
mod listings;
mod theme_preview;

pub use config::*;
pub use listings::*;
pub use theme_preview::*;

use std::rc::Rc;

use strata_agent::assistant::Assistant;

use crate::agent::AgentCtx;
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
    /// The Settings window's uncommitted theme pick, or `None` — see [`theme_preview`]. On
    /// the bundle rather than in the Settings window because every window's theme derivation
    /// reads it: that is what makes the preview live everywhere at once.
    pub preview: ThemePreview,
    pub menu: MenuState,
    /// The focused window's open path, parked by `use_file_menu` — see [`FocusedOpen`]. It
    /// sits beside `menu` because it exists for the same reason: the File menu is app-global
    /// but its contents belong to one window, and Open Recent is the item that carries data
    /// rather than a chord.
    pub open: FocusedOpen,
    /// Agent access (AA-03): the cross-thread service directory every project window joins,
    /// and the slot holding whatever MCP server is listening. Here rather than in a static of
    /// its own for the reason the rest are — a window is handed one value, not eight.
    pub agent: AgentCtx,
    /// What each provider last reported serving (AS-06) — read by Settings' model picker and,
    /// when AS-04 lands, by the composer footer's. On the bundle for the preview's reason:
    /// more than one window picks a model, and the list is a property of the machine rather
    /// than of whichever window happened to fetch it.
    pub listings: ModelListings,
    /// What each provider last said when it was asked (AS-06) — app-global beside `listings`,
    /// because a credential edit has to retract every surface's copy at once and there is only
    /// one satellite for them to race into. Not persisted; see [`Probes`].
    pub probes: ProviderProbes,
    /// The assistant's runtime (AS-02) — one per app, handed to each project window's chat
    /// pane. `None` when it could not be built, which the composer states rather than crashing
    /// over: nothing else in the app needs it.
    ///
    /// An `Rc` rather than a store: it is a handle on threads, never a value that changes, and
    /// nothing reacts to it.
    pub assistant: Option<Rc<Assistant>>,
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
