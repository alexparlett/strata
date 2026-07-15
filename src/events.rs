//! The **Events log** — a per-window store of app events (the bottom drawer's
//! Events tab). Written by [`push`] / [`push_err`] from across the action layer and
//! the engine-event handler; read by the drawer. Runtime-only, never persisted —
//! split out of `AppState` (F7 B8), like [`crate::runs`] / [`crate::diagnostics`].
//!
//! The events `Vec` is a per-window `GlobalStore` (like [`crate::runs::RUNS`]); row
//! ids come from a process-global counter — they need only be unique within a
//! window's list, which is all the drawer needs for keys + row lookup.

use std::sync::atomic::{AtomicU64, Ordering};

use dioxus::prelude::*;
use dioxus_stores::*;

use crate::query_error::QueryError;
use crate::state::{now_hms, LogEvent, LogKind};

/// Cap on retained events (newest first).
const CAP: usize = 200;

/// This window's events log (per-window, like [`crate::runs::RUNS`]).
pub static EVENTS: GlobalStore<Vec<LogEvent>> = Global::new(|| Vec::new());

/// Monotonic row-id source (process-global; ids need only be unique within a list).
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn store() -> Store<Vec<LogEvent>> {
    EVENTS.resolve()
}

// ---- reads (drawer Events tab) ------------------------------------------------

/// A clone of the events, newest first.
pub fn items() -> Vec<LogEvent> {
    store().read().clone()
}

/// The number of events (the Events tab count).
pub fn len() -> usize {
    store().read().len()
}

// ---- mutations ----------------------------------------------------------------

/// Append an ordinary event.
pub fn push(kind: LogKind, msg: impl Into<String>) {
    push_err(kind, msg, None, None);
}

/// Append an event, optionally attaching a structured error (the expandable
/// Events-row detail) and the owning query tab.
pub fn push_err(kind: LogKind, msg: impl Into<String>, err: Option<QueryError>, ws: Option<u64>) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut s = store();
    let mut events = s.write();
    events.insert(
        0,
        LogEvent {
            id,
            kind,
            msg: msg.into(),
            ts: now_hms(),
            err,
            open: false,
            ws,
        },
    );
    events.truncate(CAP);
}

/// Clear all events (the drawer's Clear on the Events tab).
pub fn clear() {
    let mut s = store();
    s.write().clear();
}

/// Toggle an Events error row's expanded detail; no-op if the id is gone.
pub fn toggle_row(id: u64) {
    let mut s = store();
    let mut events = s.write();
    if let Some(e) = events.iter_mut().find(|e| e.id == id) {
        e.open = !e.open;
    }
}
