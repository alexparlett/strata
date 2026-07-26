//! Diagnostics **vocabulary** — what the SQL validator asserts about a tab's text, and what
//! the Problems view reasons in. Framework-agnostic, so the `sql` validator (which produces
//! them) can depend *down* onto vocabulary; the per-tab store that holds them is app-side
//! (`QueryTab::diagnostics` in `strata-freya`).
//!
//! A diagnostic is a **live fact about text**, not a log entry: every validation pass replaces
//! a tab's slice wholesale, so fixing the SQL retracts the problem on the next pass. Query
//! *execution* failures are deliberately not modelled here — they belong to a run, and the
//! results pane renders one in full (banner, code frame, caret, hint) from
//! [`QueryError`](crate::QueryError).

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
/// No class/rule code: the design's Problems row is icon · message · line, and a code chip was
/// a third thing competing with the message for a single line.
///
/// No owning tab either. A diagnostic is *stored on* the tab it belongs to, so carrying a
/// `TabId` as well would be the same fact under two names — and this crate is leaf vocabulary
/// that knows nothing about tabs. The Problems view gets the owner from the group it renders
/// the row in.
#[derive(Clone, PartialEq, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    /// `line L:C` — the Problems-row display label.
    pub loc: Option<String>,
    /// Byte range into the tab's SQL — drives the editor squiggle, and the click-to-jump when
    /// it lands. Interpreted against the text the pass ran on, which the tab's validation
    /// stamp records.
    pub span: Option<Range<usize>>,
}

impl Diagnostic {
    pub fn is_error(&self) -> bool {
        matches!(self.severity, Severity::Error)
    }
}
