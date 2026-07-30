//! The **agent keepers** (AA-03) — one invisible query subscriber per parked run reply, which
//! is the [`keeper`](super::keeper) pattern applied to a second question.
//!
//! A tab's own request keeper already observes every press's settle, and records history and
//! the event log from it. It cannot also answer the agent, because the two have different
//! lifetimes: the tab's pin tracks the tab's *current* request and unmounts the moment a newer
//! press replaces it, while an ask that has been superseded still has a caller waiting to be
//! told so. So an agent run has two observers of one settle, doing two different jobs — and
//! **one** execution, because both subscribe through [`QuerySpec::query`] and a `Query`'s
//! settings are its cache identity (AGENTS.md §2). freya-query attaches a mounting subscriber
//! to the entry that is already in flight rather than dispatching it again.
//!
//! Nothing here judges a settle. A stop and a fault both arrive as the engine's `Err` string
//! and travel out as one; `strata_core::engine::stopped_on_purpose` is asked in exactly one
//! place in the whole system — the `run` tool — and a second reading of it here would be the
//! copy that drifts.

use freya::prelude::*;
use freya::query::{use_query, QueryStateData};
use strata_agent::{RunSettle, Settled};

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{QueryOutcome, QuerySpec};
use crate::apps::project::state::AgentRuns;

/// One keeper per parked run reply, rendered invisibly at the project root beside
/// `RequestKeepers`. Subscribes the parked list, which the driver pushes to and the keepers
/// themselves drain.
#[derive(PartialEq)]
pub struct AgentKeepers;

impl Component for AgentKeepers {
    fn render(&self) -> impl IntoElement {
        let runs = use_consume::<AgentRuns>();
        let specs: Vec<QuerySpec> = runs.read().iter().map(|r| r.spec.clone()).collect();
        rect().children(specs.into_iter().map(|spec| {
            // Keyed by the press's nonce, like every other pin on a run: two asks in one tab
            // are two presses, and the second must not inherit the first's scope.
            let run = spec.run;
            AgentKeeper {
                spec,
                key: DiffKey::None,
            }
            .key(run)
        }))
    }
}

/// The pin: a bare subscriber on the press the ask dispatched, which answers the ask when it
/// settles and then removes itself. Renders nothing.
#[derive(PartialEq)]
struct AgentKeeper {
    spec: QuerySpec,
    key: DiffKey,
}

impl KeyExt for AgentKeeper {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for AgentKeeper {
    fn render(&self) -> impl IntoElement {
        let engine = use_consume::<EngineCtx>();
        let runs = use_consume::<AgentRuns>();
        let query = use_query(self.spec.query(&engine));
        let run = self.spec.run;
        use_side_effect(move || {
            // Resolved while the query's borrow is held and released before the write below —
            // the same shape `use_run_logging` uses, and for the same reason.
            let settled = match &*query.read().state() {
                QueryStateData::Settled { res, .. } => Some(outcome(res)),
                _ => None,
            };
            let Some(settled) = settled else {
                return;
            };
            let mut runs = runs;
            let mut parked = runs.write();
            let Some(at) = parked.iter().position(|r| r.spec.run == run) else {
                return;
            };
            if let Some(reply) = parked[at].reply.take() {
                let _ = reply.send(Ok(settled));
            }
            // Dropping the entry is what unmounts this keeper: lifetime is mount, here as
            // everywhere else, so there is no bookkeeping to get wrong.
            parked.remove(at);
        });
        rect()
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// A settled press as the vocabulary's [`RunSettle`] — the engine's answer, unjudged.
fn outcome(res: &Result<QueryOutcome, String>) -> RunSettle {
    match res {
        Ok(QueryOutcome::Rows(page)) => Ok(Settled::Rows(page.output.clone())),
        Ok(QueryOutcome::Plan(plan)) => Ok(Settled::Plan(plan.clone())),
        Err(e) => Err(e.clone()),
    }
}
