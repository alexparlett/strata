//! **How long a child window may live**: no longer than the project subtree whose handles it
//! holds.
//!
//! Export and Configure are their own OS windows, so they cannot inherit the project window's
//! context — every store, log, counter and gate they need is carried across as a launch value.
//! All of those are created *inside* `ProjectRoot` and are `GenerationalBox`-backed, so the two
//! things that remount that subtree free them and let the storage be reused: opening another
//! project in the same window ([`OpenPref::This`](strata_core::config::OpenPref)) and an engine
//! restart (P4-07). Neither closes the window, and neither changes the window **id** the child
//! was pinned to — a re-root changes the folder, a restart changes neither. A child left open
//! across one holds dangling handles: the next read panics on a reclaimed box, and a Save before
//! that writes into a store nothing is left to serve.
//!
//! So a child window is not pinned to a window, it is pinned to a **[`Subtree`]** — the folder
//! and the engine generation that are `ProjectRoot`'s own diff key, plus the live handle to
//! compare the generation against. `ProjectRoot` provides one, so every child opened from it is
//! bound to the same three facts and no call site can assemble a mismatched trio, and
//! [`use_owner_pin`] is the one predicate over it. Anything that later hands a child window a
//! subtree handle gets this by taking a `Subtree` and calling that hook, rather than growing a
//! third copy of the rule.
//!
//! **Why the generation is safe to hold** — it is the one handle here that is *not*
//! subtree-scoped. [`EngineRestart`] is owned by `ProjectApp`, the window layer, deliberately
//! above the subtree so that the bump survives the very remount it causes. That is exactly what
//! makes it readable from a window that outlived the subtree.

use freya::prelude::*;
use freya::winit::window::WindowId;

use crate::apps::project::EngineRestart;
use crate::platform::windows::{WindowKind, WindowRegistry};
use crate::state::AppCtx;

/// The **open project** a child window's handles belong to: `ProjectRoot`'s identity as it was
/// when the child opened, and the live handle that says whether it still is.
///
/// Provided by `ProjectRoot` and taken as a launch value by every window it opens.
#[derive(Clone, PartialEq)]
pub struct Subtree {
    /// The project folder — half of `ProjectRoot`'s diff key, and what a re-root changes.
    pub project: String,
    /// The engine generation — the other half, and what a restart changes.
    pub generation: u64,
    /// The window's live generation, for reading the *current* value back. Owned by
    /// `ProjectApp`, above the subtree, so a child window may hold it after the subtree it
    /// describes has gone.
    pub restart: EngineRestart,
}

impl Subtree {
    /// Whether the subtree this describes is still the one standing.
    ///
    /// Both halves of the diff key, in the registry's terms: `windows` answers which project
    /// the owner window shows *now* (a re-root rewrites its entry; a close removes it), and the
    /// generation is read live off the window's own handle. Reactive on purpose — this is what
    /// wakes [`use_owner_pin`].
    ///
    /// **The registry is asked first, and alone.** That ordering is not tidiness or cheapness —
    /// it is what keeps the second read legal. `restart` is a `State` in the *owner window's*
    /// scope, so once that window has closed its box is reclaimed and reading the generation
    /// panics. An owner that is gone from the registry shows nothing, so this arm answers "not
    /// current" without the generation at all — which is also why "my owner closed" needs no
    /// clause of its own. An early return rather than the left half of an `&&`, because a `u64`
    /// compare beside a `String` clone reads like the cheap half and invites a swap, and a swap
    /// here panics on every ordinary close of a project window that has a child window open.
    fn is_current(&self, windows: WindowRegistry, owner: WindowId) -> bool {
        let showing = windows
            .read()
            .by_id()
            .get(&owner)
            .and_then(|kind| match kind {
                WindowKind::Project(path) => Some(path.clone()),
                _ => None,
            });
        if showing.as_deref() != Some(self.project.as_str()) {
            return false;
        }
        self.restart.generation() == self.generation
    }
}

/// Tie this child window to the project subtree it was opened on: close as soon as that subtree
/// is no longer the one standing. Call once in a child window root, with the owner id and the
/// [`Subtree`] it was launched with.
///
/// **Closing with the owner is ours, not AppKit's** — the same reason Settings needs a pin of its
/// own. A child window is closed by its parent, but AppKit does that behind winit's back: the
/// `NSWindow` goes and Freya, which only removes a window on a close it was asked for, keeps a
/// live scope for a window that is no longer on screen. Expressing it in the registry's terms
/// also covers the platforms where the child relationship is a no-op.
///
/// The window does not look its own id up, unlike the two pins this replaces: the answer is a
/// property of the *owner*, so a child that hasn't yet learned its own id can already be closed
/// — and closing is [`Platform::close_current_window`], which needs no id.
pub fn use_owner_pin(app: AppCtx, owner: WindowId, subtree: Subtree) {
    let platform = use_hook(Platform::get);
    let windows = app.windows;
    use_side_effect(move || {
        if !subtree.is_current(windows, owner) {
            platform.close_current_window();
        }
    });
}
