//! **Revealing a row** — how something outside a form asks it to single one of its rows out.
//!
//! The Settings window's search (P4-09) is what needs this: a hit names a setting, picking it
//! routes to that setting's page, and then the row on it has to be found. The **row** is what
//! knows how to do that — it is the thing that has a measured area to scroll to and a box to
//! flash — so the ask is a slot a row takes, never a call into one.
//!
//! Two contexts, because they have two lifetimes:
//!
//! - [`Reveal`] is the **ask**, and belongs to the window. It is written before the page holding
//!   the target has mounted, and has to survive the navigation that mounts it.
//! - [`RevealScroll`] is the **frame** a row scrolls itself into, and belongs to the page — that
//!   is what owns the `ScrollView`.
//!
//! Both are optional. A form with no [`Reveal`] above it is a form of ordinary rows (the export
//! window, the Configure window), and a row asked for inside a form that doesn't scroll still
//! flashes where it stands.

use freya::components::ScrollController;
use freya::prelude::*;

/// The one row a form has been asked to reveal, by [`Row::anchor`](super::Row::anchor).
///
/// One slot rather than a set: a reveal is a place the user is being taken *now*, and the newest
/// ask is the one they are waiting on.
#[derive(Clone, Copy, PartialEq)]
pub struct Reveal(State<Option<&'static str>>);

impl Reveal {
    /// The empty slot — for a window root to hand its subtree through
    /// [`use_provide_context`].
    pub fn empty() -> Self {
        Self(State::create(None))
    }

    /// Ask for the row carrying `anchor`.
    pub fn ask(self, anchor: &'static str) {
        let mut slot = self.0;
        slot.set(Some(anchor));
    }

    /// Whether `anchor` is the row being asked for. A **reactive** read, which is the whole point
    /// of the slot: the row that answers usually does not exist yet when the ask is made.
    pub fn wanted(self, anchor: &'static str) -> bool {
        *self.0.read() == Some(anchor)
    }

    /// Clear the ask — called by the row that acted on it, so a reveal happens exactly once and
    /// no later render repeats it.
    pub fn taken(self) {
        let mut slot = self.0;
        slot.set(None);
    }
}

/// The scrolling frame a revealed row brings itself into view within — whatever put the form in a
/// `ScrollView` provides this beside it.
#[derive(Clone, Copy, PartialEq)]
pub struct RevealScroll(ScrollController);

impl RevealScroll {
    pub fn new(controller: ScrollController) -> Self {
        Self(controller)
    }

    /// Scroll the minimum amount needed to bring `area` into view. A no-op when it already is, so
    /// a row that is on screen is never yanked about.
    pub fn reveal(self, area: Area) {
        let mut controller = self.0;
        controller.scroll_to_item(area);
    }
}
