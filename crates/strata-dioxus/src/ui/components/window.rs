//! `Window` — a non-modal, egui-style floating panel (Settings / Export /
//! Configure). Unlike [`Dialog`](super::dialog::Dialog) there's no scrim and it
//! doesn't block the app behind it: it floats at a geometry the container owns,
//! draggable by its titlebar and resizable from the bottom-right grip. Mount it
//! conditionally; the caller owns *whether* it's open and hands in the body as
//! `children`. Closing (titlebar ✕ or Esc) calls `on_close`. See
//! `docs/OVERLAY_ARCHITECTURE.md`.

use dioxus::prelude::*;

use super::{Path, Title};
use crate::ui::icons::{IconName, IconSize};

/// A floating window's geometry in client pixels (top-left corner + size).
#[derive(Clone, Copy, PartialEq)]
pub struct WinGeom {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl WinGeom {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
}

/// An in-progress titlebar drag (`resize == false`) or corner resize. Records the
/// pointer position and the geometry at the moment the drag began, so each move
/// applies an absolute delta (no drift).
#[derive(Clone, Copy)]
struct Drag {
    resize: bool,
    px: f64,
    py: f64,
    orig: WinGeom,
}

/// Non-modal floating window. `init` sets the opening geometry (default 720×560
/// near the top-left); the container then owns geometry internally as the user
/// drags/resizes.
#[component]
pub fn Window(
    on_close: EventHandler<()>,
    title: String,
    /// Small muted line under the title (e.g. "appearance & behavior").
    subtitle: Option<String>,
    /// Leading titlebar icon (a typed [`IconName`]).
    icon: Option<IconName>,
    /// Titlebar-icon size (default `IconSize::Md`, 16px).
    #[props(default = IconSize::Md)]
    icon_size: IconSize,
    /// Opening geometry (default: 720×560 at 220,96).
    init: Option<WinGeom>,
    /// Minimum size while resizing (defaults 360×240).
    min_w: Option<f64>,
    min_h: Option<f64>,
    /// Optional footer row (`.modal-foot`).
    footer: Option<Element>,
    children: Element,
) -> Element {
    let mut geom = use_signal(|| init.unwrap_or_else(|| WinGeom::new(220.0, 96.0, 720.0, 560.0)));
    let mut drag = use_signal(|| None::<Drag>);
    let min_w = min_w.unwrap_or(360.0);
    let min_h = min_h.unwrap_or(240.0);
    let g = geom();

    rsx! {
        // The floating window. Focusable (no scrim) so Esc works immediately.
        div {
            class: "ps-window",
            tabindex: "0",
            style: "left:{g.x}px;top:{g.y}px;width:{g.w}px;height:{g.h}px;",
            onmounted: move |e| {
                spawn(async move { let _ = e.set_focus(true).await; });
            },
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    e.prevent_default();
                    e.stop_propagation();
                    on_close.call(());
                }
            },
            // Titlebar — the drag handle.
            div {
                class: "modal-head win-titlebar",
                onmousedown: move |e| {
                    let c = e.client_coordinates();
                    drag.set(Some(Drag { resize: false, px: c.x, py: c.y, orig: geom() }));
                },
                if let Some(ic) = icon {
                    div { class: "modal-ico", {ic.el(icon_size)} }
                }
                div { style: "flex:1;min-width:0;",
                    Title { class: "modal-title", "{title}" }
                    if let Some(sub) = subtitle {
                        Path { class: "modal-sub", "{sub}" }
                    }
                }
                button {
                    class: "icon-btn plain",
                    style: "width:30px;height:30px;",
                    // Clicking the close button must not begin a titlebar drag.
                    onmousedown: move |e| e.stop_propagation(),
                    onclick: move |_| on_close.call(()),
                    {IconName::Close.el(IconSize::Sm)}
                }
            }
            // Body (caller supplies its own padding).
            div { class: "win-body", {children} }
            if let Some(f) = footer {
                div { class: "modal-foot", {f} }
            }
            // Resize grip (bottom-right corner).
            div {
                class: "win-grip",
                onmousedown: move |e| {
                    e.stop_propagation();
                    let c = e.client_coordinates();
                    drag.set(Some(Drag { resize: true, px: c.x, py: c.y, orig: geom() }));
                },
            }
        }
        // While dragging/resizing, a full-screen transparent layer captures the
        // pointer so tracking continues even when the cursor leaves the window.
        if let Some(d) = drag() {
            div {
                class: "win-drag-capture",
                style: if d.resize { "cursor:nwse-resize;" } else { "cursor:grabbing;" },
                onmousemove: move |e| {
                    let c = e.client_coordinates();
                    let (dx, dy) = (c.x - d.px, c.y - d.py);
                    let mut ng = d.orig;
                    if d.resize {
                        ng.w = (d.orig.w + dx).max(min_w);
                        ng.h = (d.orig.h + dy).max(min_h);
                    } else {
                        ng.x = d.orig.x + dx;
                        ng.y = (d.orig.y + dy).max(0.0);
                    }
                    geom.set(ng);
                },
                onmouseup: move |_| drag.set(None),
            }
        }
    }
}
