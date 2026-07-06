//! A per-window **bus** for UI overlay actions.
//!
//! Overlay visibility (Settings, Export, Config, the command palette) is a pure UI
//! concern, kept *out* of `AppState`. Each such overlay is an always-mounted
//! **host** component that owns a local `show` signal and renders nothing until
//! it's set. Triggers still go through [`crate::action::dispatch`] as normal —
//! but for overlay actions `dispatch` publishes here instead of mutating
//! `AppState`, and the hosts **subscribe** by reading [`BUS`] inside a reactive
//! effect and flipping their own `show`. That's "the dispatcher routes the action
//! to whichever host is interested," with no shared state.
//!
//! Each project window is its own `VirtualDom` (see [`crate::window`]), and a
//! `GlobalSignal` is scoped to its `VirtualDom`, so this bus is **per-window** —
//! overlays never cross-trigger between windows.

use dioxus::prelude::*;

use crate::action::Action;

/// The latest published overlay action, tagged with a monotonic sequence number so
/// that re-publishing the *same* action (e.g. two `ToggleSettings` in a row) still
/// notifies subscribers — a plain value wouldn't change and wouldn't re-fire.
pub static BUS: GlobalSignal<Option<(u64, Action)>> = Signal::global(|| None);

/// Publish an overlay action to every subscribed host in this window.
pub fn publish(action: Action) {
    let next = BUS
        .peek()
        .as_ref()
        .map(|(seq, _)| seq.wrapping_add(1))
        .unwrap_or(1);
    *BUS.write() = Some((next, action));
}

/// Whether an [`Action`] is an overlay action (routed to [`BUS`] by `dispatch`
/// rather than run against `AppState`).
pub fn is_overlay(action: &Action) -> bool {
    matches!(action, Action::ToggleSettings)
}
