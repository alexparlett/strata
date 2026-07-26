//! The window's **request keepers** — one invisible query subscriber per open tab's
//! current Run press. The results pane mounts only for the *active* tab, so on its own a
//! backgrounded tab's press would have no subscriber: freya-query cleans an unsubscribed
//! entry after its `clean_time`, and a revisit past that would find nothing and silently
//! re-execute the press — aborting it mid-flight if it was still running, or re-running
//! SQL the user only pressed once (SNAPSHOT_SPEC §6: a Run is an *action*). The keeper
//! makes subscriber presence track request **currency** instead of tab visibility: while
//! a press is some tab's `QueryTab::request`, its entry is held live; the moment it is
//! superseded, cancelled, or its tab closes, its pin unmounts and the entry ages out on
//! freya-query's own clean time. No imperative cache management — lifetime *is* mount.
//!
//! Mounted by `ProjectRoot`, beside the tab-close engine funnel — deliberately **not**
//! inside the workbench: the invariant is session-scoped (every open tab's request), so
//! its guarantor must live exactly as long as the open project, not as long as whichever
//! layout happens to show the workbench.
//!
//! The pin also owns **history recording** (P4-14): it observes the press settle even
//! while its tab is backgrounded, so a successful run lands in history at its real
//! completion time rather than whenever the tab is next revisited. (One narrow edge
//! remains: a run whose settle lands in the same update pass that unmounts its pin — a
//! supersede at the instant of completion — is not recorded.)

use freya::prelude::*;
use freya::query::use_query;
use freya::radio::use_radio;
use strata_model::TabId;

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::QuerySpec;
use crate::apps::project::state::{use_history_recording, Chan, SessionState};

/// One keeper per open tab, rendered invisibly at the project root. Subscribes the tab
/// *set* only (`Chan::Tabs`); each keeper tracks its own tab's request on its own channel,
/// so a press wakes exactly one keeper and a keystroke wakes none.
#[derive(PartialEq)]
pub struct RequestKeepers;

impl Component for RequestKeepers {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<SessionState, Chan>(Chan::Tabs);
        let tabs: Vec<TabId> = radio.read().tabs.keys().copied().collect();
        rect().children(tabs.into_iter().map(|id| {
            RequestKeeper {
                id,
                key: DiffKey::None,
            }
            .key(id)
            .into()
        }))
    }
}

/// One tab's keeper: subscribes the tab's Run trigger (`Chan::Request(id)`, the same
/// channel the results pane reads) and holds a pin on the current press, if any.
#[derive(PartialEq)]
struct RequestKeeper {
    id: TabId,
    key: DiffKey,
}

impl KeyExt for RequestKeeper {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for RequestKeeper {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<SessionState, Chan>(Chan::Request(self.id));
        let spec = radio.read().request(self.id).cloned();
        rect().map(spec, |el, spec| {
            // Keyed by the press's nonce, like `ResultsBody`: a new press drops the old
            // pin — its superseded entry starts aging out — and mounts a fresh one.
            let run = spec.run;
            el.child(
                RequestPin {
                    spec,
                    key: DiffKey::None,
                }
                .key(run),
            )
        })
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The pin itself: a bare subscriber on the press's query (built through
/// [`QuerySpec::query`], like every Run subscription — the settings are cache identity).
/// Mounting attaches to the in-flight execution, or dispatches it if the pin mounts
/// before the results body — either way the in-flight count dedups to one execution.
/// Renders nothing.
#[derive(PartialEq)]
struct RequestPin {
    spec: QuerySpec,
    key: DiffKey,
}

impl KeyExt for RequestPin {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for RequestPin {
    fn render(&self) -> impl IntoElement {
        let engine = use_consume::<EngineCtx>();
        let query = use_query(self.spec.query(&engine));
        use_history_recording(query, self.spec.run, self.spec.sql.clone());
        rect()
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}
