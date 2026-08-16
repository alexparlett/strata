//! The window's **event log** satellite — the record behind the drawer's Events tab (P3-13 ·
//! state-arch §8).
//!
//! A context signal rather than a store, on the same terms as [`History`](super::history::History):
//! nothing needs surgical per-channel updates, because one append wakes exactly one reader (the
//! Events body, when it is mounted).
//!
//! **Appended by whoever observed the fact.** There is no producer hook and there must not be one:
//! an event is not derivable from anything, so the only layer that can honestly record it is the
//! one that watched it happen — the catalog scan for each def, Save and the drop confirm for their
//! own mutations, the tab's request keeper for a Run's outcome (or, for an intercepted statement,
//! the fold that applies its effect, since only that knows whether the def was written). The
//! opposite of the diagnostics driver, for the opposite reason: diagnostics are a pure function of
//! the buffer and the catalog, while a log is a history of things that no longer exist to be
//! re-read.
//!
//! **Ephemeral, and never a second copy.** Nothing here is persisted and nothing here is the *only*
//! copy of anything — a registration failure lives on its catalog row, a run failure in that run's
//! own query entry. The log is the **record that they happened**, in one place and in order, which
//! is what no surface showing live state can give.
//!
//! **No `origin` field.** The level is real: the dot's colour and an error's message tone. An
//! origin is not, because every message already names what it is about, so a structured copy could
//! disagree with the sentence beside it — the reason a `Diagnostic` carries no `TabId`. A filter,
//! or a toast host wanting "recent warn+", can add the field when it is the thing being built.

use std::collections::VecDeque;

use freya::prelude::{
    use_consume, use_provide_context, use_side_effect, use_state, State, WritableUtils,
};
use freya::query::{QueryStateData, UseQuery};

use crate::apps::project::query::{QueryOutcome, RunQuery};
use strata_core::util::fmt_int;
use strata_core::util::now_hms;
use strata_engine::{stopped_on_purpose, CANCELLED};

/// How many events are kept, newest-first. The log is a scrollback, not an audit trail — old
/// enough to answer "what did the scan say", short enough that it can't grow without bound in a
/// window left open for a week.
const CAP: usize = 200;

/// What an event says about itself: the dot's tone, off the sheet's semantic ramp (`success` /
/// `info` / `warning` / `error`). Four, because that ramp has four: the canvas's
/// separate `run` kind painted the same colour as `info` and differed in nothing else.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogLevel {
    /// Something the user asked for finished, and worked.
    Ok,
    /// Something happened that is neither an outcome nor a fault.
    Info,
    /// Finished, but not the way it was asked for (a cancelled run).
    Warning,
    /// Failed.
    Error,
}

/// One row of the log.
pub struct LogEvent {
    /// Append order — the row's list key, so an event arriving above another doesn't shuffle the
    /// rest through each other's scopes. Per log, which is all a key needs to be.
    pub seq: u64,
    pub level: LogLevel,
    pub message: String,
    /// The local wall clock the event was appended at, `HH:MM:SS`
    /// ([`now_hms`](strata_core::util::now_hms)) — formatted once, at append, because that is when
    /// the clock says what it said.
    pub at: String,
}

/// The window's event log: newest first, capped at [`CAP`].
#[derive(Default)]
pub struct Log {
    events: VecDeque<LogEvent>,
    next_seq: u64,
}

impl Log {
    /// Append `message` at `level`, stamped now, and drop the oldest event past [`CAP`].
    pub fn push(&mut self, level: LogLevel, message: impl Into<String>) {
        self.next_seq += 1;
        self.events.push_front(LogEvent {
            seq: self.next_seq,
            level,
            message: message.into(),
            at: now_hms(),
        });
        while self.events.len() > CAP {
            self.events.pop_back();
        }
    }

    /// The events, newest first.
    pub fn events(&self) -> impl Iterator<Item = &LogEvent> {
        self.events.iter()
    }

    /// How many events are held — the drawer header's count on this tab, which is also what
    /// decides the tab's empty state and whether **Clear** is live. One question, one answer; no
    /// `is_empty` beside it, because a second way to ask it would just be a second thing to keep
    /// in step.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Empty the log — the drawer's **Clear** (the first working one; P3-11 shipped the button
    /// parked because nothing had a log to clear yet).
    pub fn clear(&mut self) {
        self.events.clear();
    }
}

/// The event log in context.
pub type LogCtx = State<Log>;

/// Stand this project's log up and provide it. Call once in the window root — **before**
/// `use_init_project`, whose scan is the first thing that records into it.
pub fn use_init_log() -> LogCtx {
    use_provide_context(|| State::create(Log::default()))
}

/// Append one event. A free function over the handle rather than a hook, because every observer
/// is a spawned task or an event handler by the time it knows what happened; the handle is
/// captured at render time and passed in, like the engine and the stores.
pub fn log_event(mut log: LogCtx, level: LogLevel, message: impl Into<String>) {
    log.write().push(level, message);
}

/// Record a Run press's outcome, once, when it settles.
///
/// Called from the tab's request keeper (`views::keeper`) beside `use_history_recording`, and for
/// the same reason: the pin is mounted for the press's whole life, so a backgrounded tab's run is
/// still observed — and stamped — at its real completion time. (The one edge history also has: a
/// settle landing in the same update pass that unmounts its pin goes unrecorded.) The local flag
/// is the whole dedup: the pin is keyed by the press's nonce, so there is never a second observer
/// of the same settle to guard against.
///
/// Both halves of a settle are recorded, unlike history's success-only rule — a log of runs that
/// omitted the failures would be the one thing nobody looks at a log for.
pub fn use_run_logging(query: UseQuery<RunQuery>) {
    let log = use_consume::<LogCtx>();
    let mut logged = use_state(|| false);
    use_side_effect(move || {
        if *logged.peek() {
            return;
        }
        let settled = match &*query.read().state() {
            QueryStateData::Settled { res, .. } => Some(run_event(res)),
            _ => None,
        };
        let Some(event) = settled else {
            return;
        };
        logged.set(true);
        if let Some((level, message)) = event {
            log_event(log, level, message);
        }
    });
}

/// What a settled Run reads as in the log.
///
/// A run that was **stopped** is a warning, not an error, and `engine::stopped_on_purpose` is the
/// one place that knows which settles those are — deliberately not a string compare here. The
/// engine has *three* such strings, not one: `cancelled` (an abort), `superseded by a newer run` (a
/// press that finished after a newer one replaced it) and `superseded by a newer scan` (the profile
/// equivalent). This used to test `e == "cancelled"`, which mapped a supersede to a red `Error` row
/// reading "superseded by a newer run" — a fault the user never had, and precisely the string
/// that must never read as a problem. Everything else `Err` is the engine's own
/// message, the same text the results pane frames.
///
/// `None` means **somebody else records this settle**, which today is exactly one case: an
/// intercepted statement (ED-02) is logged by `state::statement`'s fold, because its message
/// claims something durable ("Table 't' created") and only the fold knows whether the def
/// reached `project.json`. Logging it here as well would be two rows arguing about one action —
/// the `save_view` lesson, which is where that gate came from.
fn run_event(res: &Result<QueryOutcome, String>) -> Option<(LogLevel, String)> {
    let event = match res {
        Ok(QueryOutcome::Statement(_)) => return None,
        Ok(QueryOutcome::Rows(page)) => (
            LogLevel::Ok,
            format!(
                "Query executed · {} rows · {} ms",
                fmt_int(page.output.total as u64),
                page.output.elapsed_ms
            ),
        ),
        Ok(QueryOutcome::Plan(plan)) => (
            LogLevel::Ok,
            match plan.analyze {
                true => "Explained query with analyze".into(),
                false => "Explained query".into(),
            },
        ),
        Err(e) if stopped_on_purpose(e) => (
            LogLevel::Warning,
            match e.as_str() {
                CANCELLED => "Query cancelled".into(),
                stopped => format!("Query {stopped}"),
            },
        ),
        Err(e) => (LogLevel::Error, e.clone()),
    };
    Some(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_engine::{SUPERSEDED_RUN, SUPERSEDED_SCAN};

    /// Newest first, and bounded: the log is a scrollback, so the oldest event is what goes.
    #[test]
    fn events_are_newest_first_and_capped() {
        let mut log = Log::default();
        for i in 0..CAP + 2 {
            log.push(LogLevel::Info, format!("event {i}"));
        }
        assert_eq!(log.len(), CAP);
        let messages: Vec<&str> = log.events().map(|e| e.message.as_str()).collect();
        assert_eq!(messages[0], format!("event {}", CAP + 1));
        assert_eq!(
            messages[CAP - 1],
            "event 2",
            "the two oldest events aged out"
        );
    }

    /// Sequence numbers keep rising past the cap — they are list keys, so a recycled one would
    /// hand a new event the outgoing row's scope.
    #[test]
    fn sequence_numbers_are_unique_across_the_cap() {
        let mut log = Log::default();
        for _ in 0..CAP + 5 {
            log.push(LogLevel::Ok, "x");
        }
        let seqs: Vec<u64> = log.events().map(|e| e.seq).collect();
        assert_eq!(seqs[0], (CAP + 5) as u64);
        assert!(seqs.windows(2).all(|w| w[0] > w[1]), "strictly descending");
    }

    #[test]
    fn clear_empties_the_log() {
        let mut log = Log::default();
        log.push(LogLevel::Error, "boom");
        assert_eq!(log.len(), 1);
        log.clear();
        assert_eq!(log.len(), 0);
        log.push(LogLevel::Ok, "again");
        assert_eq!(log.events().next().unwrap().seq, 2);
    }

    /// A cancel is a warning, not a failure — the distinction the results pane also makes.
    #[test]
    fn a_cancelled_run_logs_as_a_warning() {
        let (level, message) = run_event(&Err(CANCELLED.to_string())).expect("logged here");
        assert_eq!(level, LogLevel::Warning);
        assert_eq!(message, "Query cancelled");
    }

    /// **And so is a supersede.** A press that finished after a newer one replaced it settles a
    /// *different* string from a cancel (`superseded by a newer run`), which an `e == "cancelled"`
    /// test missed — logging it as a red `Error` reading "superseded by a newer run", a fault the
    /// user never had. Every string the engine calls `stopped_on_purpose` maps to a warning; the
    /// arm is reached through that predicate, so a fourth one can't quietly fall through to
    /// `Error` again.
    #[test]
    fn a_superseded_run_logs_as_a_warning_too() {
        for stopped in [SUPERSEDED_RUN, SUPERSEDED_SCAN] {
            let (level, message) = run_event(&Err(stopped.to_string())).expect("logged here");
            assert_eq!(level, LogLevel::Warning, "{stopped}");
            assert_eq!(message, format!("Query {stopped}"));
        }
    }

    /// Any other failure is the engine's own message, verbatim — the same text the results pane
    /// frames, so the two can't describe one run differently.
    #[test]
    fn a_failed_run_logs_the_engines_message() {
        let (level, message) = run_event(&Err("Schema error: No field named 'amont'".to_string()))
            .expect("logged here");
        assert_eq!(level, LogLevel::Error);
        assert_eq!(message, "Schema error: No field named 'amont'");
    }
}
