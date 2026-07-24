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
//! `derive_channel` stays the default (`vec![self]`): granularity comes from *which* channel
//! a component subscribes to, not from fan-out.

use freya::radio::RadioChannel;

use super::session::{SessionState, TabId};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Chan {
    Tabs,
    Tab(TabId),
    Request(TabId),
    View(TabId),
    Diagnostics(TabId),
}

impl RadioChannel<SessionState> for Chan {}
