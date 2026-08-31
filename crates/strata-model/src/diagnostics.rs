//! Diagnostics **vocabulary** — what the SQL validator asserts about a tab's text, and what the
//! Problems view reasons in. The per-tab store that holds them is app-side
//! (`QueryTab::diagnostics`).
//!
//! A diagnostic is a **live fact about text**, not a log entry: every validation pass replaces a
//! tab's slice wholesale, so fixing the SQL retracts the problem. Query *execution* failures belong
//! to a run and never reach here: they live in that run's own query entry, which is where the
//! results pane renders them.

use std::ops::Range;

/// Diagnostic severity (LSP-ish). Only [`Error`](Severity::Error) counts toward the Problems
/// badge and the drawer's header tally; warnings and infos still list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// One problem with a tab's SQL: a severity, a message, and where it is.
///
/// No class/rule code — the Problems row is icon · message · line, and a chip would compete with
/// the message for it. No owning tab either: a diagnostic is *stored on* the tab it belongs to, and
/// the Problems view gets the owner from the group it renders the row in.
#[derive(Clone, PartialEq, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    /// `line L:C` — the Problems-row display label.
    pub loc: Option<String>,
    /// Byte range into the tab's SQL, driving the editor squiggle. Interpreted against the text
    /// the pass ran on, which the tab's validation stamp records.
    pub span: Option<Range<usize>>,
}

impl Diagnostic {
    pub fn is_error(&self) -> bool {
        matches!(self.severity, Severity::Error)
    }
}
