//! The app-global **model listings** — what each provider last reported, for as long as the
//! app is installed rather than as long as a window is open.
//!
//! **Why app-global.** Two surfaces pick a model from this list and neither owns it: Settings
//! ▸ AI ▸ Chat picks what a new chat starts on, and the chat pane's composer picks per
//! conversation (AS-04). A list held by the Settings window would be empty in the project
//! window, and a list held by either would be empty at the next launch — which is the failure
//! the satellite exists to remove, since a `Select` whose only content arrives from a network
//! call is an empty `Select` every time the app starts.
//!
//! **Disk is a startup input**, exactly as it is for the config store: [`create_global_listings`]
//! reads the file once in `main` and after that this slot is the truth. [`write_listings`] is
//! the only thing that writes it.
//!
//! Distinct from the Settings window's `Probes`, which is *not* persisted and must not be: a
//! probe is the state of a request the user made minutes ago, and a "verified" restored from
//! disk at launch would be a claim nothing had checked. A listing is the answer that request
//! returned, which stays true until the address or the credential moves.

use freya::prelude::State;
use strata_core::models::{self, Listings};

/// The app-global listings slot — created in `main` and handed to every window root.
pub type ModelListings = State<Listings>;

/// Load the satellite into the one app-global slot. Call **once**, in `main`, before `launch`
/// — not a hook.
///
/// The read is synchronous and blocking, like [`config::load`](strata_core::config::load) two
/// lines above it: there is no event loop yet to hold up, and this is a small JSON file in the
/// user's own config directory rather than a project on a mount that may have stopped
/// answering.
pub fn create_global_listings() -> ModelListings {
    State::create_global(models::load())
}

/// Mutate the listings and persist them — **the** write path; nothing else calls
/// [`models::save`].
///
/// Returns whether the edit reached disk. The in-memory slot is updated either way, on
/// `write_config`'s reasoning: the surface must show what was just fetched, and a listing that
/// fails to persist costs one refetch at the next launch.
///
/// **Not reported at a surface**, and that is the difference from `SettingsCtx::apply`: nothing
/// here is a deliberate commit the user pressed a button for. This is a cache being filled by a
/// fetch they did not ask for, so a failure is a `tracing` line and a refetch, never a message
/// about a file they have no reason to know exists.
///
/// The write itself is synchronous, like every other write of a file in the config directory
/// (`state::write_config`). It runs after an offloaded fetch has already come back, so the
/// blocking half of the refresh — the keystore read and the HTTP round trip — is off the render
/// thread; what is left is a few hundred bytes to the same directory the config store writes to
/// on every project open.
pub fn write_listings(state: ModelListings, edit: impl FnOnce(&mut Listings)) -> bool {
    let mut state = state;
    edit(&mut state.write());
    match models::save(&state.peek()) {
        Ok(()) => true,
        Err(e) => {
            tracing::error!("{e}");
            false
        }
    }
}
