//! The Session's Radio channels.
//!
//! `Tabs` = strip structure (order / active). `Tab(id)` = one tab's fields — Valin's
//! `follow_tab(id)`, a first-class data-carrying channel so editing one tab wakes only that
//! tab's subscribers. `Request(id)` = that tab's Run trigger alone, split from `Tab(id)` so a
//! press wakes only the tab's results pane and toolbar — never the editor — and keystrokes
//! never wake the results. `View(id)` = that tab's Table/Chart results view mode (P2-07),
//! split the same way so a body flip wakes only the tab's results pane. `Diagnostics(id)` =
//! that tab's validation diagnostics (P2-18), split so a validation pass settling wakes only
//! diagnostics readers (the Problems drawer, P3-12) — never the editor or the results.
//! `Layout` = the window's panel arrangement (which side panels / drawer are open + active —
//! P3-01), which the shell + activity rail subscribe to. `LayoutSize` = the panels' last sizes,
//! split off `Layout` so a resize drag (which `ResizableContainer` fires ~per-frame) persists
//! without re-rendering the shell — nobody subscribes to it, the shell only *peeks* it to seed
//! `initial_size`. `Persist` is the one **fan-in**: every write that changes what `session.json`
//! stores *also* notifies it (via [`derive_channel`](RadioChannel::derive_channel)), so the lone
//! autosave subscriber (P4-14) wakes on any of them without waking — or being woken by — the
//! per-tab UI channels. Otherwise `derive_channel` stays the default (`vec![self]`): granularity
//! comes from *which* channel a component subscribes to, not from fan-out.

use freya::radio::RadioChannel;

use strata_model::TabId;

use super::session::SessionState;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Chan {
    Tabs,
    Tab(TabId),
    Request(TabId),
    View(TabId),
    Diagnostics(TabId),
    /// The window's panel arrangement (P3-01): which side panels / drawer are open, on
    /// which pane / tab. The shell + activity rail subscribe, so a collapse / toggle
    /// re-renders them.
    Layout,
    /// The panels' last sizes (P3-01), split off `Layout` so a resize drag persists
    /// without re-rendering the shell — nobody subscribes; the shell only peeks it to seed
    /// `initial_size`.
    LayoutSize,
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
            // Persisted facets → also wake autosave.
            Chan::Tabs | Chan::Tab(_) | Chan::View(_) | Chan::Layout | Chan::LayoutSize => {
                vec![self, Chan::Persist]
            }
            // Ephemeral / the sink itself → just themselves.
            Chan::Request(_) | Chan::Diagnostics(_) | Chan::Persist => vec![self],
        }
    }
}
