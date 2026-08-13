//! The Session's Radio channels.
//!
//! `Tabs` is strip structure; `Tab(id)` is one tab's fields, a first-class data-carrying channel so
//! editing one tab wakes only its subscribers. `Request(id)`, `View(id)` and `Chart(id)` are each
//! split further off it for the same reason: a Run press must wake the results pane and not the
//! editor, a body flip must wake only the results pane, and picking a Y column must re-chart and
//! nothing else. `Layout` is the window's panel arrangement, and `LayoutSize` is split off it so a
//! resize drag (fired ~per-frame) persists without re-rendering the shell — nobody subscribes to
//! it; the shell only *peeks* it to seed `initial_size`.
//!
//! `Diagnostics` is **not** per tab, deliberately: every consumer is cross-tab, so each would
//! re-render on any tab's change anyway, and the editor does not read it at all. Per-tab
//! granularity would buy nothing and cost the thing that matters — a component cannot subscribe a
//! variable number of channels.
//!
//! Two **fan-ins**, both synthetic: nobody writes them directly, they are channels other writes
//! derive (via [`derive_channel`](RadioChannel::derive_channel)) so one subscriber can watch a
//! whole class of change.
//!
//! - `Persist` — every write that changes what `session.json` stores notifies it, so the lone
//!   autosave subscriber wakes on any of them without waking the per-tab UI channels.
//! - `Text` — every `Tab(id)` write notifies it, so the one validation driver can watch **any**
//!   tab's buffer from a single subscription. Otherwise it would need one subscription per open
//!   tab, which is a variable hook count and therefore impossible.
//!
//! Otherwise `derive_channel` stays the default: granularity comes from *which* channel a component
//! subscribes to, not from fan-out.

use freya::radio::RadioChannel;

use strata_model::TabId;

use super::session::SessionState;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Chan {
    Tabs,
    Tab(TabId),
    Request(TabId),
    View(TabId),
    /// That tab's chart encoding (Rz2 — `ChartConfig`): the mark, the column assignments and
    /// the sort. Its own channel, so an encoder edit wakes the chart body alone.
    Chart(TabId),
    /// Every tab's validation verdict, on one channel — see the module note.
    Diagnostics,
    /// The window's panel arrangement (P3-01): which side panels / drawer are open, on
    /// which pane / tab. The shell + activity rail subscribe, so a collapse / toggle
    /// re-renders them.
    Layout,
    /// The panels' last sizes (P3-01), split off `Layout` so a resize drag persists
    /// without re-rendering the shell — nobody subscribes; the shell only peeks it to seed
    /// `initial_size`.
    LayoutSize,
    /// Synthetic fan-in for **any tab's buffer**, so the validation driver can watch every
    /// tab at once. Nobody writes it; `Tab(_)` derives it.
    Text,
    /// Synthetic sink for session autosave (P4-14). Nobody writes it directly — it's the
    /// extra channel that structural / buffer / view-mode writes derive, so one debounced
    /// side effect can observe *every* persist-relevant change. The ephemeral channels
    /// (`Request` = the run trigger, `Diagnostics` = validation) are deliberately left out:
    /// their state never reaches disk, so folding them in would only churn the file on a
    /// Run press or a squiggle.
    Persist,
}

impl RadioChannel<SessionState> for Chan {
    fn derive_channel(self, _state: &SessionState) -> Vec<Self> {
        match self {
            Chan::Tab(_) => vec![self, Chan::Persist, Chan::Text],
            Chan::Tabs | Chan::View(_) | Chan::Chart(_) | Chan::Layout | Chan::LayoutSize => {
                vec![self, Chan::Persist]
            }
            Chan::Request(_) | Chan::Diagnostics | Chan::Text | Chan::Persist => vec![self],
        }
    }
}
