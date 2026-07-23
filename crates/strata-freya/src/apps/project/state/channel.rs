//! The Session's Radio channels.
//!
//! `Tabs` = strip structure (order / active). `Tab(id)` = one tab's fields — Valin's
//! `follow_tab(id)`, a first-class data-carrying channel so editing one tab wakes only that
//! tab's subscribers. `Request(id)` = that tab's Run trigger alone, split from `Tab(id)` so a
//! press wakes only the tab's results pane and toolbar — never the editor — and keystrokes
//! never wake the results. `derive_channel` stays the default (`vec![self]`): granularity
//! comes from *which* channel a component subscribes to, not from fan-out.

use freya::radio::RadioChannel;

use super::session::{SessionState, TabId};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Chan {
    Tabs,
    Tab(TabId),
    Request(TabId),
}

impl RadioChannel<SessionState> for Chan {}
