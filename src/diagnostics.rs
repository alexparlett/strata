//! First-class, per-tab **diagnostics** — the model behind the Problems view.
//!
//! A problem is *not* a log entry. It's a live fact about a tab's SQL that some
//! provider asserts and later retracts. Two providers feed the Problems view:
//!
//! * **Validation** — static analysis that needs *no* execution (e.g. a keyword
//!   typo `FORM`→`FROM`, unbalanced parens). Computed by `crate::sql::analyze`,
//!   debounced per tab (`ui::workbench::workspace::use_revalidate`), then stored via
//!   [`set`]; authoritative — it *replaces* the tab's slice each pass, so fixing the
//!   SQL clears the problem on the next keystroke, no run required. Stored in [`DIAGS`].
//! * **Execution** — a query that actually failed. Already tracked, correctly
//!   lifecycled, as `runs::WorkspaceRun::query_error` (set on failure, cleared on
//!   rerun-start and on success). We don't duplicate it here; [`problems_for`]
//!   simply *unions* it in.
//!
//! Both are keyed by the same `crate::session::WorkspaceId`. Reactivity mirrors
//! `crate::runs`: `DIAGS.get(id)` tracks just that key (creation included), so the
//! Problems view + rail badge re-render when a tab's diagnostics change. Never
//! persisted; dropped on tab close ([`drop_ids`]) and project open ([`clear`]).

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use dioxus::prelude::*;

use crate::model::QueryError;
// Lens accessors (`.workspaces()`, `.id()`) to sum problems across tabs.
use crate::session::{SessionStoreExt, WorkspaceStoreExt};

/// Diagnostic severity (LSP-ish). Only `Error` counts toward the Problems badge.
/// `Warning`/`Info` are unused until the static validator lands (execution errors
/// are always `Error`) — allow so the stub phase stays warning-clean.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Which provider asserted a diagnostic. `Validation` lives in [`DIAGS`];
/// `Execution` is synthesized on the fly from `runs::query_error`. `Validation`
/// isn't constructed until the validator lands — allow through the stub phase.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DiagSource {
    Validation,
    Execution,
}

/// One problem on a tab: a severity, a message, and (optionally) a `line L:C`
/// location for jump/squiggle.
///
/// No class/rule code: the design's Problems row is icon · message · line, and a code
/// chip was a third thing competing with the message for a single line. It was also
/// near-redundant — an execution error's code was its `etype`, which is already the
/// message whenever the error has no body. Re-add it with a place to show it if the
/// validator (E1) ever needs to distinguish rules.
#[derive(Clone, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub source: DiagSource,
    pub message: String,
    /// `line L:C` (matches `QueryError::loc`) — the Problems-row display label.
    pub loc: Option<String>,
    /// Byte range into the tab's SQL (S25) — drives the editor squiggle + the
    /// click-to-select jump. `None` for execution errors (only a `line:col` string).
    pub span: Option<Range<usize>>,
}

impl Diagnostic {
    pub fn is_error(&self) -> bool {
        matches!(self.severity, Severity::Error)
    }

    /// Fold a failed query's structured error into an execution diagnostic.
    fn from_query_error(qe: &QueryError) -> Self {
        let head = qe.message.lines().next().unwrap_or("").trim();
        let message = if head.is_empty() {
            qe.etype.clone()
        } else {
            head.to_string()
        };
        Self {
            severity: Severity::Error,
            source: DiagSource::Execution,
            message,
            loc: qe.loc.clone(),
            span: None,
        }
    }
}

/// This window's per-tab **validation** diagnostics, keyed by workspace id.
/// (Execution errors are not stored here — see the module docs.)
///
/// Read/written **coarsely** (whole-map `read`/`write`), not per-key: cross-tab
/// aggregators (the Problems drawer + rail badge) are always mounted and would
/// subscribe to a key *before* it exists, and the per-key `.get(id)` subscription
/// doesn't fire for that absent→present transition. A whole-map `read` subscribes to
/// every change, so any `set`/`drop`/`clear` wakes them — one store, no epoch.
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

/// Total problem count (all severities) across all open tabs — the Problems header
/// count + rail badge. **Reactive** — call inside a component render.
pub fn total_problems() -> usize {
    let sess = crate::session::store();
    sess.workspaces()
        .iter()
        .map(|w| problems_for(w.id().cloned()).len())
        .sum()
}
