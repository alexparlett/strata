//! The app-side **reactive wrapper** around the framework-agnostic engine handle.
//!
//! [`strata_core::engine::Engine`] is a plain connection object (tokio channels + an
//! atomic request counter); this module owns the per-window `GlobalSignal` that holds
//! it and re-exposes the exact static API the app already calls — `Engine::send`,
//! `Engine::functions`, the `command!` macro — so every `crate::engine::*` path (and
//! the protocol types) resolves unchanged. The reactive story lives here, not in core:
//! `functions()` reads through the signal (subscribes), `set_functions()` writes
//! through it (notifies), while `send()`/`next_req()` peek (no notify) so the query
//! hot path never wakes the language-service readers. The Freya frontend will hold the
//! same core handle in its own context instead of this Dioxus `Global`.

use dioxus::prelude::*;
use tokio::sync::mpsc::UnboundedReceiver;

use strata_core::engine::Engine as CoreEngine;
// The engine's two public submodules also keep their `crate::engine::*` paths: the UI
// settings screen reads `config` (datafusion key defs / validation), and the grid +
// copy/export actions read `serialize` (RecordBatch → TSV/JSON text).
pub use strata_core::engine::{config, serialize};
// Re-export the engine protocol + one-shot snapshot cleanup at their original
// `crate::engine::*` paths, so existing call sites (and `command!`) don't move.
pub use strata_core::engine::{purge_snapshot_root, Command, Event, TableMeta, TableSpec};

use strata_core::engine::sql::FunctionCatalog;

/// This window's engine handle — lazily spawned on first access, seeded with the
/// current `datafusion.*` overrides (W2). Per-window Dioxus state ⇒ a `GlobalSignal`
/// (resolved per window scope). Held whole (the core handle isn't `PartialEq`/`Clone`,
/// which a `GlobalSignal` doesn't require — we only ever borrow + mutate it in place).
static ENGINE: GlobalSignal<CoreEngine> =
    Global::new(|| CoreEngine::spawn(crate::settings::engine_overrides()));

/// Namespace for the static engine accessors. A unit type, **not** the handle itself
/// (that's [`strata_core::engine::Engine`], living in the `GlobalSignal` above) — it
/// only exists so the app's `crate::engine::Engine::*` call sites and the `command!`
/// macro keep resolving unchanged.
pub struct Engine;

impl Engine {
    /// Send a command to this window's engine. Non-reactive (`.peek()` — `cmd_tx` is
    /// `&self`), so the query hot path never notifies the `functions()` readers.
    pub fn send(cmd: Command) {
        ENGINE.peek().send(cmd);
    }

    /// Allocate the next request id (monotonic). Non-reactive (`.peek()` — the atomic
    /// bump takes `&self`), so it never notifies the `functions()` readers.
    pub fn next_req() -> u64 {
        ENGINE.peek().next_req()
    }

    /// A clone of the registered SQL functions — a **reactive** read (`.read()`), so
    /// the editor's language catalog re-derives when [`Event::Functions`] lands via
    /// [`set_functions`](Engine::set_functions).
    pub fn functions() -> FunctionCatalog {
        ENGINE.read().functions().clone()
    }

    /// Replace the registered SQL functions (`Event::Functions`). Writes through the
    /// signal (`.write()`), notifying the `functions()` readers.
    pub fn set_functions(functions: FunctionCatalog) {
        ENGINE.write().set_functions(functions);
    }

    /// Take this window's event stream for the single drain task (panics if taken
    /// twice — see the core handle).
    pub fn take_evt_rx() -> UnboundedReceiver<Event> {
        ENGINE.write().take_evt_rx()
    }
}

/// Send a [`Command`] to this window's engine — sugar for [`Engine::send`] that
/// prefixes `Command::`, mirroring the `crate::event_*!` log macros. `#[macro_export]`
/// puts it at the crate root, so call it fully-qualified: `crate::command!(…)`.
/// Everything after the name is the variant, so struct / tuple / unit variants all
/// work: `crate::command!(CleanupAll)`, `crate::command!(Cancel { ws_id, req_id })`,
/// `crate::command!(Register(spec))`.
#[macro_export]
macro_rules! command {
    ($($variant:tt)+) => {
        $crate::engine::Engine::send($crate::engine::Command::$($variant)+)
    };
}
