//! **What Settings asks a provider with** — the one thing about the model-listing refresh that
//! is this window's rather than the app's.
//!
//! The refresh itself lives in [`crate::state::listings`] (AS-04 moved it there): the guard, the
//! two keeps, the retraction rule and the thread it runs on are the same for every surface that
//! fetches a list, and the chat pane's composer footer is one of them with no `SettingsCtx` in
//! reach. What stays here is how *this* window fills in an [`Ask`] — from the uncommitted draft,
//! which is what Apply would write and therefore what a test should prove.

use strata_core::ai::ProviderKind;
use strata_core::secret::Secret;

use crate::apps::settings::SettingsCtx;
use crate::state::Ask;

/// Extend the shared ask with the Settings window's own source for it.
///
/// An inherent impl beside the surface rather than in `state::listings`: reading a draft is
/// exactly the part of the ask that is not shared, and the funnel must not learn about a window.
pub trait FromDraft {
    /// What the window currently holds for `kind` — the ask a surface makes when it is not the
    /// Configure dialog.
    ///
    /// The dialog builds its own from its boxes, because it tests what is on screen rather than
    /// what is filed. Everywhere else — a picker refreshing a stale list — the draft *is* what
    /// is on screen, and a pending key beats the stored marker for the dialog's own reason: it
    /// is what Apply would send.
    ///
    /// `peek` throughout: this is called from an effect and from event handlers, and
    /// subscribing them to the whole draft would re-run every one of them on any keystroke in
    /// the window.
    fn from_draft(ctx: SettingsCtx, kind: ProviderKind) -> Ask;
}

impl FromDraft for Ask {
    fn from_draft(ctx: SettingsCtx, kind: ProviderKind) -> Ask {
        let keys = ctx.ai_keys.peek();
        let pending = keys.touched(kind);
        Ask {
            kind,
            base_url: ctx.base_url_of(kind),
            typed: Secret::new(keys.get(kind)),
            // An entry that is *touched* and empty is a pending removal, so falling back to the
            // stored marker there would authenticate with a key on its way out — the same trap
            // the dialog's Test names.
            stored: (!pending)
                .then(|| {
                    ctx.draft
                        .peek()
                        .ai
                        .setup(kind)
                        .and_then(|setup| setup.key.clone())
                })
                .flatten(),
        }
    }
}

/// Ask the provider what it serves, keeping both halves of the answer in this window's slots.
///
/// A one-line spelling of [`crate::state::listings::refresh`] with `SettingsCtx`'s two handles
/// filled in — not a second implementation of it, which is the whole reason the funnel moved.
pub fn refresh(ctx: SettingsCtx, ask: Ask) -> Option<freya::prelude::TaskHandle> {
    crate::state::refresh(ctx.listings, ctx.probes, ask)
}
