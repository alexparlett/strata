//! Panel-resize action handlers + the shared drag-handle component. Called from
//! `action::dispatch`; the handle itself dispatches `Action::StartResize`.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, ResizeTarget, Resizing};

/// Begin a panel drag. `origin` is the pointer's client coordinate on the drag
/// axis; `start` is the panel's current size. Axis, direction and min/max clamp
/// are derived from the target.
pub fn start_resize(mut state: Signal<AppState>, target: ResizeTarget, origin: f64, start: f64) {
    let (axis_x, sign, min, max) = match target {
        ResizeTarget::Sidebar => (true, 1.0, 210.0, 520.0),
        ResizeTarget::Inspector => (true, -1.0, 220.0, 560.0),
        ResizeTarget::Editor => (false, 1.0, 92.0, 480.0),
        ResizeTarget::Log => (false, -1.0, 120.0, 480.0),
    };
    state.write().resizing = Some(Resizing {
        target,
        axis_x,
        sign,
        origin,
        start,
        min,
        max,
    });
}

/// Apply a pointer move during an active drag (no-op otherwise).
pub fn resize_move(mut state: Signal<AppState>, x: f64, y: f64) {
    let r = state.read().resizing.clone();
    let Some(r) = r else {
        return;
    };
    let cur = if r.axis_x { x } else { y };
    let new = (r.start + (cur - r.origin) * r.sign).clamp(r.min, r.max);
    let mut s = state.write();
    match r.target {
        ResizeTarget::Sidebar => s.sidebar_w = new,
        ResizeTarget::Inspector => s.inspector_w = new,
        ResizeTarget::Editor => s.editor_h = new,
        ResizeTarget::Log => s.log_h = new,
    }
}

pub fn end_resize(mut state: Signal<AppState>) {
    if state.read().resizing.is_some() {
        state.write().resizing = None;
    }
}

/// Toggle the catalog sidebar.
pub fn toggle_sidebar(mut state: Signal<AppState>) {
    let mut s = state.write();
    s.sidebar_open = !s.sidebar_open;
}

/// Close the column inspector.
pub fn close_inspector(mut state: Signal<AppState>) {
    state.write().inspector_open = false;
}

/// Toggle the window between filling the screen work area and its previous size
/// (OS "zoom" — RustRover-style double-click-title-bar). Restores to the last
/// manual size; a manually screen-filled window is a no-op toggle.
pub fn toggle_window_fill(_state: Signal<AppState>) {
    dioxus::desktop::window().toggle_maximized();
}

/// A draggable resize handle for `target`, placed between/at the edge of panels.
/// Captures the pointer anchor + current size on mousedown and emits
/// `Action::StartResize`; the root's `onmousemove`/`onmouseup` drive the rest.
pub fn resize_handle(state: Signal<AppState>, target: ResizeTarget) -> Element {
    let axis_x = matches!(target, ResizeTarget::Sidebar | ResizeTarget::Inspector);
    let cls = if axis_x { "resizer col" } else { "resizer row" };
    rsx! {
        div {
            class: cls,
            title: "Drag to resize",
            onmousedown: move |e| {
                // Stop the webview from starting a text-selection drag.
                e.prevent_default();
                let c = e.client_coordinates();
                let (origin, start) = {
                    let s = state.read();
                    match target {
                        ResizeTarget::Sidebar => (c.x, s.sidebar_w),
                        ResizeTarget::Inspector => (c.x, s.inspector_w),
                        ResizeTarget::Editor => (c.y, s.editor_h),
                        ResizeTarget::Log => (c.y, s.log_h),
                    }
                };
                dispatch(state, Action::StartResize { target, origin, start });
            },
        }
    }
}
