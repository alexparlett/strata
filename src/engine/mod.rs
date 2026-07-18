//! DataFusion engine on a dedicated thread with its own Tokio runtime.
//!
//! Pagination model (bounded memory): each query is executed **once** and its
//! full result is spooled to a temporary parquet **snapshot** on disk. The true
//! row count comes from a `COUNT(*)` over the snapshot, and every page is a
//! bounded `LIMIT/OFFSET` read from it — so RAM only ever holds one page, no
//! matter how far the user pages, and no query is ever recomputed per page.
//!
//! UI → engine: `tokio::mpsc::unbounded` of [`Command`]. engine → UI:
//! `tokio::mpsc::unbounded` of [`Event`], drained by a Dioxus coroutine.
//!
//! This module is the **handle** — the per-window UI-side owner of the connection.
//! The worker loop and its helpers live in the submodules; `pub use` keeps the public
//! types at their original `crate::engine::*` paths.

mod catalog;
mod explain;
mod export;
mod message;
mod query;
mod worker;
pub mod config;
pub mod serialize;

pub use message::{Command, Event, TableMeta, TableSpec};
pub use query::purge_snapshot_root;

use std::sync::atomic::{AtomicU64, Ordering};

use dioxus::prelude::{Global, ReadableExt, WritableExt};
use dioxus_stores::*;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::sql::FunctionCatalog;

/// Process-unique id per spawned engine (one per project window), used to scope
/// snapshot files so windows never collide.
static ENGINE_SEQ: AtomicU64 = AtomicU64::new(0);

/// This window's engine — the UI-side owner of the connection: the command channel
/// (`send`), the request-id counter (`next_req`), and the registered SQL functions.
/// [`Engine::spawn`] starts the worker thread and stashes the inbox, handing back the
/// event stream for the caller to drain. The instance lives in the private `Global`
/// below (Dioxus per-window state must be a `Global`; `cmd_tx` also isn't `PartialEq`,
/// so `Engine` can't derive `Store` and instead rides whole in an `Option`, like the
/// whole-value stores in `crate::events`).
#[derive(Store)]
pub struct Engine {
    cmd_tx: UnboundedSender<Command>,
    /// This window's event stream — `Some` until the single drain task takes it
    /// (`take_evt_rx`). A receiver is single-consumer, so it can't stay a live store
    /// borrow: holding one across the async drain loop collides with any other engine
    /// write (e.g. `set_functions`) on the same signal.
    evt_rx: Option<UnboundedReceiver<Event>>,
    /// Monotonic request-id source — an `AtomicU64` so `next_req` mutates it through a
    /// *read* borrow of the store (no store write ⇒ it never notifies `functions` readers).
    next_req: AtomicU64,
    /// The engine's registered SQL functions — read reactively by the language service.
    functions: FunctionCatalog,
}

/// This window's single engine — `None` until [`Engine::spawn`], then `Some`. A
/// `GlobalStore` needs a `Store`-able type and `Engine` holds a non-`PartialEq`
/// `Sender`, so it rides whole in an `Option` (accessed as one value, cf. the
/// whole-value stores in `crate::events`) rather than deriving `Store` per field.
static ENGINE: GlobalStore<Engine> = Global::new(|| Engine::spawn());

pub fn store() -> Store<Engine> {
    ENGINE.resolve()
}

impl Engine {
    /// Start this window's engine worker (seeded with the current `datafusion.*`
    /// `overrides`, W2), stash the instance, and return the event stream for the caller
    /// to drain. Later config changes arrive as [`Command::SetEngineConfig`].
    pub fn spawn() -> Engine {
        let overrides = crate::settings::engine_overrides();
        let (cmd_tx, cmd_rx) = unbounded_channel::<Command>();
        let (evt_tx, evt_rx) = unbounded_channel::<Event>();
        let engine_id = ENGINE_SEQ.fetch_add(1, Ordering::Relaxed);
        std::thread::Builder::new()
            .name(format!("df-engine-{engine_id}"))
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(worker::engine_loop(cmd_rx, evt_tx, engine_id, overrides));
            })
            .expect("spawn engine");
        Engine {
            cmd_tx,
            evt_rx: Some(evt_rx),
            next_req: AtomicU64::new(1),
            functions: FunctionCatalog::default(),
        }
    }

    /// Send a command to this window's engine (no-op if it isn't up yet).
    pub fn send(cmd: Command) {
        let store = ENGINE.resolve();
        let e = store.peek();
        let _ = e.cmd_tx.send(cmd);
    }

    /// Allocate the next request id (monotonic).
    pub fn next_req() -> u64 {
        let store = ENGINE.resolve();
        let g = store.peek();

        g.next_req.fetch_add(1, Ordering::Relaxed)
    }

    /// A clone of the registered SQL functions — reactive (the editor's language catalog).
    pub fn functions() -> FunctionCatalog {
        let store = ENGINE.resolve();
        let g = store.read();
        g.functions.clone()
    }

    /// Replace the registered SQL functions (`Event::Functions`).
    pub fn set_functions(functions: FunctionCatalog) {
        let mut store = ENGINE.resolve();
        let mut g = store.write();
        g.functions = functions;
    }

    /// Take this window's event stream for the single drain task. A receiver is
    /// single-consumer, so it leaves the store (`None` after) rather than being held
    /// as a live borrow across the drain loop — which would collide with any other
    /// engine write. Panics if taken twice.
    pub fn take_evt_rx() -> UnboundedReceiver<Event> {
        let mut store = ENGINE.resolve();
        let mut g = store.write();
        g.evt_rx.take().expect("engine event stream already taken")
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
