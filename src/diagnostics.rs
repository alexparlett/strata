//! First-class, per-tab **diagnostics** — the store behind the Problems view.
//!
//! The diagnostic *types* (`Diagnostic`/`DiagSource`/`Severity`) are shared vocabulary and
//! now live in `strata-model` (the `sql` validator produces them). This module keeps the
//! per-window store that holds each tab's *validation* diagnostics, and the union with the
//! *execution* error that already lives on `runs::WorkspaceRun::query_error`.
//!
//! A problem is *not* a log entry. It's a live fact about a tab's SQL that some provider
//! asserts and later retracts. Validation is authoritative — it *replaces* the tab's slice
//! each pass (`crate::sql::analyze`, debounced), so fixing the SQL clears the problem on the
//! next keystroke, no run required. Keyed by `crate::session::WorkspaceId`; never persisted;
//! dropped on tab close ([`drop_ids`]) and project open ([`clear`]).

use std::collections::{HashMap, HashSet};

use dioxus::prelude::*;

pub use crate::model::{DiagSource, Diagnostic, Severity};
// Lens accessors (`.workspaces()`, `.id()`) to sum problems across tabs.
use crate::session::{SessionStoreExt, WorkspaceStoreExt};

/// This window's per-tab **validation** diagnostics, keyed by workspace id.
/// (Execution errors are not stored here — see the module docs.)
///
/// Read/written **coarsely** (whole-map `read`/`write`), not per-key: cross-tab
/// aggregators (the Problems drawer + rail badge) are always mounted and would subscribe to
/// a key *before* it exists, and the per-key `.get(id)` subscription doesn't fire for that
/// absent→present transition. A whole-map `read` subscribes to every change, so any
/// `set`/`drop`/`clear` wakes them — one store, no epoch.
pub static DIAGS: GlobalStore<HashMap<u64, Vec<Diagnostic>>> = Global::new(|| HashMap::new());

/// Replace tab `id`'s validation slice (the validator is authoritative each pass).
/// An empty slice drops the key ("clean"); a coarse whole-map write notifies readers.
pub fn set(id: u64, diags: Vec<Diagnostic>) {
    let mut store = DIAGS.resolve();
    let mut map = store.write();
    if diags.is_empty() {
        map.remove(&id);
    } else {
        map.insert(id, diags);
    }
}

/// Drop closed tabs' diagnostics (paired with `runs::drop_ids`).
pub fn drop_ids(ids: &HashSet<u64>) {
    let mut store = DIAGS.resolve();
    store.write().retain(|id, _| !ids.contains(id));
}

/// Clear every tab's diagnostics (project open — ids get reassigned).
pub fn clear() {
    let mut store = DIAGS.resolve();
    store.write().clear();
}

/// All problems for tab `id`: its validation slice (from [`DIAGS`]) unioned with its
/// execution error (from `runs`). **Reactive** — reads the whole `DIAGS` map so any
/// diagnostic change re-renders the caller (see the store docs).
pub fn problems_for(id: u64) -> Vec<Diagnostic> {
    let store = DIAGS.resolve();
    let mut out: Vec<Diagnostic> = store.read().get(&id).cloned().unwrap_or_default();
    if let Some(qe) = crate::runs::RUNS
        .resolve()
        .get(id)
        .and_then(|e| e.read().query_error.clone())
    {
        out.push(Diagnostic::from_query_error(&qe));
    }
    out
}

/// Total problem count (all severities) across all open tabs — the Problems header count +
/// rail badge. **Reactive** — call inside a component render.
pub fn total_problems() -> usize {
    let sess = crate::session::store();
    sess.workspaces()
        .iter()
        .map(|w| problems_for(w.id().cloned()).len())
        .sum()
}
