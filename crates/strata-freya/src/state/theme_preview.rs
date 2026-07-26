//! The Settings window's **live theme preview** — the one piece of its draft that is
//! app-global.
//!
//! Everything else the Settings window edits stays local to it and lands in the config store
//! on Save. The theme can't: picking one has to repaint *every* window immediately, and the
//! choice is still uncommitted, so it can't go through [`write_config`] (which persists) and
//! it can't sit on the Settings window (which no other window can read).
//!
//! So the theme derivation gains a second, higher-priority input: while this slot holds a
//! selection, [`use_strata_theme`] resolves *that* instead of the committed settings. The
//! theme stays **pure derived state** — there is still no stored applied-theme id anywhere,
//! just one more value the same derivation reads.
//!
//! Its lifetime is the Settings window's: `None` whenever that window is closed. Save writes
//! the selection into the settings and clears the slot in the same breath (the derivation
//! lands on the identical id, so nothing repaints twice); Cancel, Esc and every other way the
//! window can go simply clear it, which *is* the revert.
//!
//! [`write_config`]: crate::state::write_config
//! [`use_strata_theme`]: crate::theme::use_strata_theme

use freya::prelude::State;
use strata_core::config::Settings;
use strata_core::theme::effective_id;

/// A theme *selection*: the id the user picked, plus whether Sync-with-OS overrides it with
/// the light/dark default. The two fields always travel together — resolving either one
/// alone gives the wrong theme — which is why the preview carries the pair rather than an id.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ThemeSel {
    pub theme: String,
    pub sync_os: bool,
}

impl ThemeSel {
    /// The theme id this selection actually resolves to, given the current OS appearance.
    pub fn effective(&self, os_dark: bool) -> String {
        effective_id(&self.theme, self.sync_os, os_dark)
    }
}

impl From<&Settings> for ThemeSel {
    fn from(settings: &Settings) -> Self {
        Self {
            theme: settings.theme.clone(),
            sync_os: settings.sync_os,
        }
    }
}

/// The app-global preview slot — created in `main` and handed to every window root, because
/// the whole point is that one window's uncommitted pick repaints all of them.
pub type ThemePreview = State<Option<ThemeSel>>;

/// Create the slot. Call **once**, in `main`, before `launch` — not a hook.
pub fn create_global_theme_preview() -> ThemePreview {
    State::create_global(None)
}
