//! Per-window **layout store** — panel + drawer *visibility* (sidebar / inspector
//! open, and the bottom drawer's open flag + active tab).
//!
//! Split out of the old central app state (F7, "Avoid Large Groups of State"). Visibility is
//! genuinely shared — the activity rail toggles it and the root decides whether to
//! render the panel / drawer — so it lives in a small per-window `dioxus-stores`
//! `Store` (a `GlobalStore` is per-VirtualDom). Runtime-only, never persisted.
//! (The drawer's *events data* lives apart in [`crate::events`].)
//!
//! **Panel sizes + resize drags are deliberately NOT here.** Each resizable
//! component (sidebar, inspector, editor, drawer, grid column) owns its own size as a
//! *local reactive signal* and renders its own [`crate::action::panel::Resizer`]
//! handle, which mutates that local signal — there is no shared resize state.
//! Mutators write through field lenses. See [[workbench-and-runs]].

use dioxus::prelude::*;
use dioxus_stores::*;

use crate::state::LogTab;

/// Window panel + drawer visibility — per-window runtime UI state; not persisted.
#[derive(Store, Clone, PartialEq)]
pub struct Layout {
    pub sidebar_open: bool,
    pub inspector_open: bool,
    /// Whether the bottom drawer is open.
    pub drawer_open: bool,
    /// Which tab the bottom drawer shows (History / Events / Problems).
    pub drawer_tab: LogTab,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            sidebar_open: true,
            inspector_open: true,
            drawer_open: false,
            drawer_tab: LogTab::History,
        }
    }
}

/// This window's layout store. Per-window: a `GlobalStore` is per-app, and each
/// project window is its own Dioxus app (like [`crate::runs::RUNS`]).
pub static LAYOUT: GlobalStore<Layout> = Global::new(|| Layout::default());

/// This window's layout store.
pub fn store() -> Store<Layout> {
    LAYOUT.resolve()
}

// ---- reads — each subscribes to just its own field's lens ---------------------

pub fn sidebar_open() -> bool {
    store().sidebar_open().cloned()
}
pub fn inspector_open() -> bool {
    store().inspector_open().cloned()
}
pub fn drawer_open() -> bool {
    store().drawer_open().cloned()
}
pub fn drawer_tab() -> LogTab {
    store().drawer_tab().cloned()
}

// ---- mutators — lens writes ---------------------------------------------------

/// Toggle the catalog sidebar.
pub fn toggle_sidebar() {
    let v = !store().sidebar_open().cloned();
    store().sidebar_open().set(v);
}

/// Show / hide the column inspector.
pub fn set_inspector_open(open: bool) {
    store().inspector_open().set(open);
}

/// Open the bottom drawer to `tab`, or close it if it's already showing `tab` (the
/// rail toggle — clicking the active tab's button dismisses the drawer).
pub fn toggle_drawer(tab: LogTab) {
    let s = store();
    if s.drawer_open().cloned() && s.drawer_tab().cloned() == tab {
        s.drawer_open().set(false);
    } else {
        s.drawer_open().set(true);
        s.drawer_tab().set(tab);
    }
}

/// Toggle the drawer open/closed, keeping the current tab (drawer close button).
pub fn toggle_drawer_open() {
    let v = !store().drawer_open().cloned();
    store().drawer_open().set(v);
}

/// Switch the active drawer tab without changing the open state.
pub fn set_drawer_tab(tab: LogTab) {
    store().drawer_tab().set(tab);
}
