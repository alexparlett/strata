//! The Session's Radio channels.
//!
//! `Tabs` = strip structure (order / active). `Tab(id)` = one tab's fields — Valin's
//! `follow_tab(id)`, a first-class data-carrying channel so editing one tab wakes only that
//! tab's subscribers. `derive_channel` stays the default (`vec![self]`): granularity comes from
//! *which* channel a component subscribes to, not from fan-out.

use freya::radio::RadioChannel;

use super::session::{SessionState, TabId};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Chan {
    Tabs,
    Tab(TabId),
}

impl RadioChannel<SessionState> for Chan {}
