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
use dioxus_stores::*;

use crate::query_error::QueryError;
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
/// location for jump/squiggle and a short code/class chip.
#[derive(Clone, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub source: DiagSource,
    pub message: String,
    /// `line L:C` (matches `QueryError::loc`) — the Problems-row display label.
    pub loc: Option<String>,
    /// Short class/rule chip, e.g. "Planning Error" or a lint id.
    pub code: Option<String>,
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
            code: Some(qe.etype.clone()),
            span: None,
        }
    }
}

/// This window's per-tab **validation** diagnostics, keyed by workspace id.
/// (Execution errors are not stored here — see the module docs.)
pub static DIAGS: GlobalStore<HashMap<u64, Vec<Diagnostic>>> = Global::new(|| HashMap::new());

/// Replace tab `id`'s validation slice (the validator is authoritative each pass).
/// Always writes the key — an empty slice is a valid "clean" state; entries are
/// swept on tab close / project open. Mirrors `runs::edit` (insert-then-`entry.write`)
/// so per-key subscribers (Problems view, rail badge) are notified — a bare `.insert()`
/// updated the map but did not wake them.
pub fn set(id: u64, diags: Vec<Diagnostic>) {
    let mut store = DIAGS.resolve();
    if !store.contains_key(&id) {
        store.insert(id, Vec::new());
    }
    if let Some(mut entry) = store.get(id) {
        *entry.write() = diags;
    }
}

/// Drop closed tabs' diagnostics (paired with `runs::drop_ids`).
pub fn drop_ids(ids: &HashSet<u64>) {
    DIAGS.resolve().retain(|id, _| !ids.contains(id));
}

/// Clear every tab's diagnostics (project open — ids get reassigned).
pub fn clear() {
    DIAGS.resolve().clear();
}

/// All problems for tab `id`: its validation slice (from [`DIAGS`]) unioned with
/// its execution error (from `runs`). **Reactive** — call inside a component
/// render so the reads subscribe (both stores track this key, creation included).
pub fn problems_for(id: u64) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = DIAGS
        .resolve()
        .get(id)
        .map(|e| e.read().clone())
        .unwrap_or_default();
    if let Some(qe) = crate::runs::RUNS
        .resolve()
        .get(id)
        .and_then(|e| e.read().query_error.clone())
    {
        out.push(Diagnostic::from_query_error(&qe));
    }
    out
}

/// Total error-severity problems across all open tabs (Problems header + rail
/// badge). **Reactive** — call inside a component render.
pub fn total_errors() -> usize {
    let sess = crate::session::store();
    sess.workspaces()
        .iter()
        .map(|w| {
            problems_for(w.id().cloned())
                .iter()
                .filter(|d| d.is_error())
                .count()
        })
        .sum()
}
