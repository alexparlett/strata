//! Diagnostics **vocabulary** — the shapes the Problems view reasons in. The per-tab
//! store that holds validation diagnostics is app-side (`crate::diagnostics` in the
//! frontend, over a `GlobalStore`); only these framework-agnostic types live here, so the
//! `sql` validator (which produces them) can depend *down* onto vocabulary.

use std::ops::Range;

use crate::QueryError;

/// Diagnostic severity (LSP-ish). Only `Error` counts toward the Problems badge.
/// `Warning`/`Info` are unused until the static validator lands (execution errors are
/// always `Error`) — allow so the stub phase stays warning-clean.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Which provider asserted a diagnostic. `Validation` lives in the app's store;
/// `Execution` is synthesized on the fly from `runs::query_error`. `Validation` isn't
/// constructed until the validator lands — allow through the stub phase.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DiagSource {
    Validation,
    Execution,
}

/// One problem on a tab: a severity, a message, and (optionally) a `line L:C` location
/// for jump/squiggle.
///
/// No class/rule code: the design's Problems row is icon · message · line, and a code
/// chip was a third thing competing with the message for a single line.
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
    pub fn from_query_error(qe: &QueryError) -> Self {
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
