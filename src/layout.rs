//! Per-window **layout store** — panel *visibility* only (sidebar / inspector open).
//!
//! Split out of `AppState` (F7, "Avoid Large Groups of State"). Visibility is
//! genuinely shared — the activity rail toggles it and the root decides whether to
//! render the panel — so it lives in a small per-window `dioxus-stores` `Store`
//! (a `GlobalStore` is per-VirtualDom). Runtime-only, never persisted.
//!
//! **Panel sizes + resize drags are deliberately NOT here.** Each resizable
//! component (sidebar, inspector, editor, drawer, grid column) owns its own size as a
//! *local reactive signal* and renders its own [`crate::action::panel::Resizer`]
//! handle, which mutates that local signal — there is no shared resize state.
//! Mutators write through field lenses. See [[workbench-and-runs]].

use dioxus::prelude::*;
use dioxus_stores::*;

/// Window panel visibility — per-window runtime UI state; not persisted.
#[derive(Store, Clone, PartialEq)]
pub struct Layout {
    pub sidebar_open: bool,
    pub inspector_open: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            sidebar_open: true,
            inspector_open: true,
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
