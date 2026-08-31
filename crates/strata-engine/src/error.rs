//! What an [`Engine`](crate::Engine) call answers with instead of a value.

use std::fmt;

use thiserror::Error;

use crate::sources::source::ConnectRefusal;
use crate::statements::Refusal;

/// Why a call stopped rather than failed.
///
/// A stopped call is news the caller already has — it cancelled, or it dispatched again — so a
/// surface showing a settled error must never present one as a fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The caller aborted the call, through [`Workspace::cancel`](crate::Workspace::cancel) or
    /// [`Catalog::cancel_profile`](crate::Catalog::cancel_profile).
    Cancelled,
    /// A newer run replaced this one: its result is discarded and its snapshot retired.
    SupersededRun,
    /// A newer scan replaced this one.
    SupersededScan,
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let said = match self {
            StopReason::Cancelled => "cancelled",
            StopReason::SupersededRun => "superseded by a newer run",
            StopReason::SupersededScan => "superseded by a newer scan",
        };
        f.write_str(said)
    }
}

/// Why an [`Engine`](crate::Engine) call did not produce a value.
///
/// [`Stopped`](Self::Stopped) is the variant every surface has to tell apart, and matching it is
/// the whole judgement: everything else is a failure the caller reports in the engine's own words.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum EngineError {
    /// The call was stopped rather than failed.
    #[error("{0}")]
    Stopped(StopReason),
    /// The statement was refused, and never ran. The [`Refusal`] carries its classification.
    #[error("{}", .0.message())]
    Refused(Refusal),
    /// Planning or execution failed, in the engine's own readable diagnosis.
    #[error("{0}")]
    Failed(String),
    /// The runtime dropped or panicked the task carrying the call — a different fault from the
    /// call's own, which is why the message names the call rather than the work.
    #[error("{what} task failed: {why}")]
    Task {
        /// The call the runtime dropped.
        what: String,
        /// What the runtime said about it.
        why: String,
    },
}

impl EngineError {
    /// Returns the [`Task`](EngineError::Task) error naming the call the runtime dropped.
    pub(crate) fn task(what: &str, why: impl fmt::Display) -> Self {
        EngineError::Task {
            what: what.into(),
            why: why.to_string(),
        }
    }
}

impl From<String> for EngineError {
    /// Reads a diagnosis the engine produced as prose into [`Failed`](EngineError::Failed).
    ///
    /// A refusal with a [`Refusal`] to hand travels as [`Refused`](EngineError::Refused) instead,
    /// so a caller can still match its `reason` after the sentence is rendered.
    fn from(diagnosis: String) -> Self {
        EngineError::Failed(diagnosis)
    }
}

impl From<ConnectRefusal> for EngineError {
    /// A failed connect is a failure like any other to the caller that awaited it.
    ///
    /// The facet is not lost with it: the **ledger** keeps it, which is where every surface reads
    /// a registration outcome from. This is only the answer to *this* call.
    fn from(refusal: ConnectRefusal) -> Self {
        EngineError::Failed(refusal.reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statements::Reason;

    /// Every surface that shows a settled error renders these, and the two supersedes are the
    /// ones that must not read as a fault: a consumer that recognised only the first would
    /// report a supersede as a failure the user never had.
    #[test]
    fn a_stop_reads_as_the_sentence_the_user_sees() {
        assert_eq!(StopReason::Cancelled.to_string(), "cancelled");
        assert_eq!(
            StopReason::SupersededRun.to_string(),
            "superseded by a newer run"
        );
        assert_eq!(
            StopReason::SupersededScan.to_string(),
            "superseded by a newer scan"
        );
    }

    /// A refusal renders the sentence it already carried, so wrapping one changes no wording.
    #[test]
    fn a_refusal_renders_its_own_message() {
        let refusal = Refusal::from(Reason::Batch);
        assert_eq!(
            EngineError::Refused(refusal.clone()).to_string(),
            refusal.message()
        );
    }

    /// A diagnosis is passed through whole, and a runtime failure names the call it was carrying.
    #[test]
    fn a_failure_reads_as_the_engines_own_words() {
        assert_eq!(
            EngineError::from("Schema error: No field named x".to_string()).to_string(),
            "Schema error: No field named x"
        );
        assert_eq!(
            EngineError::task("query", "task was cancelled").to_string(),
            "query task failed: task was cancelled"
        );
    }
}
