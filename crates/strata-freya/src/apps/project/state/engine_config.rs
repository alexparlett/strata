//! The window's **engine-config driver**: keep this project's engine in step with
//! Settings ▸ Engine ▸ Properties (P4-07, DEV_TASKS W2).
//!
//! The engine is per project window; the setting is app-wide, one `BTreeMap` on the config
//! global. The Settings window has no engine of its own to talk to, so Apply does what every
//! other setting does — it writes the config — and each open project window picks the change up
//! here. One hook, one subscription, one call: [`Engine::set_config`] writes the `ConfigOptions`
//! half onto the live session and answers whether a restart is still owed.
//!
//! **The restart is the remount, and it asks on the same terms as a close.** A changed
//! `datafusion.runtime.*` cannot be written to a running engine — the `RuntimeEnv` is fixed when
//! the `SessionContext` is built — so the only honest way to apply one is to build a new engine
//! and stand the project up against it. That is already what `ProjectRoot`'s `render_key` does
//! for a re-root (AGENTS.md §2), so the restart is a bump of that key rather than a second path
//! that re-points a live store: the outgoing engine is dropped, its session flushed, and the
//! project registers its tables and views through the very hooks that run at launch. And because
//! it drops the engine it aborts whatever is in flight, which makes it one more action that
//! destroys a window's work — so it goes through the **one** T2 confirm
//! ([`CloseTarget::Restart`]) on the same predicate as every other, rather than a confirm of its
//! own. Declining leaves the engine as it was and the restart still owed, so the next config
//! write offers it again ([`Engine::restart_owed`]).

use std::sync::atomic::Ordering;
use std::sync::Arc;

use freya::prelude::*;

use crate::apps::project::close::{CloseGuard, CloseTarget};
use crate::apps::project::contexts::EngineCtx;
use crate::state::{use_config, use_config_station, ConfigChan};

/// This window's **engine generation** — bumping it rebuilds the engine.
///
/// A window fact, not a project one: it is owned by `ProjectApp` and read into `ProjectRoot`'s
/// diff key, so the bump survives the very remount it causes. Provided into the tree because the
/// two things that can trigger a restart sit either side of the confirm — the driver below, and
/// the dialog's confirmed answer.
#[derive(Clone, Copy, PartialEq)]
pub struct EngineRestart(State<u64>);

impl EngineRestart {
    /// Rebuild the engine now. The caller has already asked whatever needed asking.
    pub fn restart(self) {
        let mut generation = self.0;
        let next = *generation.peek() + 1;
        generation.set(next);
    }

    /// The current generation, for the subtree's diff key. Reactive — this is the read that makes
    /// a bump a remount.
    pub fn generation(&self) -> u64 {
        *self.0.read()
    }
}

/// Stand the window's engine generation up and provide it. Called by `ProjectApp`, above the
/// project subtree, so it outlives every restart it causes.
pub fn use_engine_restart() -> EngineRestart {
    let generation = use_state(|| 0u64);
    use_provide_context(move || EngineRestart(generation))
}

/// Keep this window's engine pointed at the app's engine overrides, for as long as the project is
/// open. Mounted by `ProjectRoot`, after the engine it drives.
pub fn use_engine_config(engine: &EngineCtx, confirm: State<Option<CloseTarget>>) {
    let config = use_config(ConfigChan::Settings);
    let station = use_config_station();
    let guard = use_consume::<Arc<CloseGuard>>();
    let restart = use_consume::<EngineRestart>();
    let engine = engine.clone();

    use_side_effect(move || {
        let overrides = config.read().settings.engine.clone();
        if !engine.set_config(overrides) {
            return;
        }
        // The same predicate as the `on_close` hook, `TabCloser::close` and `OpenCtx::reroot`:
        // the engine's own in-flight answer, and the user's pref about being asked.
        let running = guard.running.load(Ordering::Relaxed);
        if !(running && station.peek().settings.confirm_close_running) {
            restart.restart();
            return;
        }
        // **Never over-write a question already on screen.** Every other writer of this slot is
        // a press inside *this* window, which the open dialog's own modal barrier already blocks;
        // this one arrives from the Settings window and can land while the T2 confirm is up. The
        // one it would replace is the one that matters most: a `CloseTarget::Window` raised by
        // ⌘Q holds a vetoed close *and* the app-wide quitting flag, which only that variant's
        // answer clears (`keep_open`'s `end_quit`). Swapping it for a restart abandons the close
        // and latches the flag, and every later close then skips the launcher hand-off. The
        // restart stays owed either way (`Engine::restart_owed`), so the next config write asks
        // again — which is exactly the behaviour a declined restart already has.
        let mut confirm = confirm;
        if confirm.peek().is_none() {
            confirm.set(Some(CloseTarget::Restart));
        }
    });
}
