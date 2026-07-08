//! `Popup` — the **generic positioned-card base**, plus its positioning vocabulary
//! (modelled on egui): a popup is anchored to a [`Rect`] and placed with a [`RectAlign`]
//! (which corner of the popup pins to which corner of the anchor). The consumer just
//! names an alignment (`RectAlign::BOTTOM_START`, …) and the `Rect` — no pixel math.
//!
//! The `child` half of the alignment maps to a CSS `transform`, so a popup placed above
//! or left of its anchor needs **no size measurement**: `translate(-100%)` shifts it by
//! its own extent. Composition around the base:
//! - dismissable menu / dropdown = [`Backdrop`] `{ Popup { … } }`;
//! - hover tooltip = [`Tooltip`](super::tooltip::Tooltip) (`Popup` + pointer-transparent).
//!
//! Auto-flip (egui's `find_best_align`) is deliberately not here yet — that one needs the
//! popup's measured size.

use dioxus::prelude::*;

/// A screen point in client pixels (still handy for cursor/pointer anchors → `Rect::point`).
#[derive(Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// A rect in client pixels — a popup's anchor ("parent" rect).
#[derive(Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    /// A zero-sized rect at a point (cursor / caret anchors).
    pub fn point(x: f64, y: f64) -> Self {
        Self { x, y, w: 0.0, h: 0.0 }
    }
}

/// One-axis alignment: the start edge, the centre, or the end edge.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Min,
    Center,
    Max,
}

impl Align {
    /// Fraction along the axis (0 / 0.5 / 1) — for locating a point on the parent rect.
    fn frac(self) -> f64 {
        match self {
            Align::Min => 0.0,
            Align::Center => 0.5,
            Align::Max => 1.0,
        }
    }
    /// CSS `translate` component that pins this edge of the *child* to the anchor point.
    fn translate(self) -> &'static str {
        match self {
            Align::Min => "0",
            Align::Center => "-50%",
            Align::Max => "-100%",
        }
    }
}

/// A 2-D alignment (a corner / edge-centre / centre of a rect).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Align2 {
    pub x: Align,
    pub y: Align,
}

const fn a2(x: Align, y: Align) -> Align2 {
    Align2 { x, y }
}

/// How a popup (`child`) aligns to its anchor (`parent`) — the `child` corner is pinned to
/// the `parent` corner. Named presets cover the 12 common menu placements (egui parity).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RectAlign {
    pub parent: Align2,
    pub child: Align2,
}

impl Default for RectAlign {
    fn default() -> Self {
        RectAlign::BOTTOM_START
    }
}

impl RectAlign {
    // Below the anchor.
    pub const BOTTOM_START: RectAlign = RectAlign { parent: a2(Align::Min, Align::Max), child: a2(Align::Min, Align::Min) };
    pub const BOTTOM: RectAlign = RectAlign { parent: a2(Align::Center, Align::Max), child: a2(Align::Center, Align::Min) };
    pub const BOTTOM_END: RectAlign = RectAlign { parent: a2(Align::Max, Align::Max), child: a2(Align::Max, Align::Min) };
    // Above the anchor.
    pub const TOP_START: RectAlign = RectAlign { parent: a2(Align::Min, Align::Min), child: a2(Align::Min, Align::Max) };
    pub const TOP: RectAlign = RectAlign { parent: a2(Align::Center, Align::Min), child: a2(Align::Center, Align::Max) };
    pub const TOP_END: RectAlign = RectAlign { parent: a2(Align::Max, Align::Min), child: a2(Align::Max, Align::Max) };
    // Right of the anchor.
    pub const RIGHT_START: RectAlign = RectAlign { parent: a2(Align::Max, Align::Min), child: a2(Align::Min, Align::Min) };
    pub const RIGHT: RectAlign = RectAlign { parent: a2(Align::Max, Align::Center), child: a2(Align::Min, Align::Center) };
    pub const RIGHT_END: RectAlign = RectAlign { parent: a2(Align::Max, Align::Max), child: a2(Align::Min, Align::Max) };
    // Left of the anchor.
    pub const LEFT_START: RectAlign = RectAlign { parent: a2(Align::Min, Align::Min), child: a2(Align::Max, Align::Min) };
    pub const LEFT: RectAlign = RectAlign { parent: a2(Align::Min, Align::Center), child: a2(Align::Max, Align::Center) };
    pub const LEFT_END: RectAlign = RectAlign { parent: a2(Align::Min, Align::Max), child: a2(Align::Max, Align::Max) };

    /// Flip along one/both axes — the alternatives tried by [`RectAlign::best`].
    pub fn flip_x(self) -> Self {
        RectAlign {
            parent: Align2 { x: flip(self.parent.x), y: self.parent.y },
            child: Align2 { x: flip(self.child.x), y: self.child.y },
        }
    }
    pub fn flip_y(self) -> Self {
        RectAlign {
            parent: Align2 { x: self.parent.x, y: flip(self.parent.y) },
            child: Align2 { x: self.child.x, y: flip(self.child.y) },
        }
    }
    pub fn flip(self) -> Self {
        self.flip_x().flip_y()
    }

    /// The child rect `(left, top, w, h)` this alignment would produce for `size`.
    fn resolved(self, anchor: Rect, size: (f64, f64), gap: f64) -> (f64, f64, f64, f64) {
        let gx = gap_sign(self.parent.x, self.child.x);
        let gy = gap_sign(self.parent.y, self.child.y);
        let ax = anchor.x + self.parent.x.frac() * anchor.w + gx * gap;
        let ay = anchor.y + self.parent.y.frac() * anchor.h + gy * gap;
        (ax - self.child.x.frac() * size.0, ay - self.child.y.frac() * size.1, size.0, size.1)
    }

    fn fits(self, anchor: Rect, size: (f64, f64), vp: (f64, f64), gap: f64) -> bool {
        let (l, t, w, h) = self.resolved(anchor, size, gap);
        l >= 0.0 && t >= 0.0 && l + w <= vp.0 && t + h <= vp.1
    }

    /// The first of `[self, flip_y, flip_x, flip]` whose popup (of `size`) fits inside the
    /// `vp` viewport; else `self`. Egui's `find_best_align`, for edge auto-flipping.
    pub fn best(self, anchor: Rect, size: (f64, f64), vp: (f64, f64), gap: f64) -> Self {
        [self, self.flip_y(), self.flip_x(), self.flip()]
            .into_iter()
            .find(|a| a.fits(anchor, size, vp, gap))
            .unwrap_or(self)
    }

    /// The CSS `left/top/transform` that places the popup against `anchor`, pushed out by
    /// `gap` px along the placement side.
    pub fn style(self, anchor: Rect, gap: f64) -> String {
        let gx = gap_sign(self.parent.x, self.child.x);
        let gy = gap_sign(self.parent.y, self.child.y);
        let px = anchor.x + self.parent.x.frac() * anchor.w + gx * gap;
        let py = anchor.y + self.parent.y.frac() * anchor.h + gy * gap;
        format!(
            "left:{px}px;top:{py}px;transform:translate({},{});",
            self.child.x.translate(),
            self.child.y.translate()
        )
    }
}

/// The gap direction along one axis: push the child away from the parent when they sit on
/// opposite edges (Max↔Min), else no gap.
fn gap_sign(parent: Align, child: Align) -> f64 {
    match (parent, child) {
        (Align::Max, Align::Min) => 1.0,
        (Align::Min, Align::Max) => -1.0,
        _ => 0.0,
    }
}

/// Flip a single axis (Min↔Max; Center unchanged).
fn flip(a: Align) -> Align {
    match a {
        Align::Min => Align::Max,
        Align::Max => Align::Min,
        Align::Center => Align::Center,
    }
}

/// Viewport size in client (CSS) px = OS window physical size / scale factor.
fn viewport() -> (f64, f64) {
    let win = dioxus::desktop::window();
    let sf = win.scale_factor();
    let sz = win.inner_size();
    (sz.width as f64 / sf, sz.height as f64 / sf)
}

/// A fixed-position card placed against `anchor` per `align`. `card_class` styles it
/// (default the shared `ds-menu`); `children` is the body. Stops click/contextmenu
/// propagation so, composed inside a [`Backdrop`], an inside-click doesn't dismiss it.
#[component]
pub fn Popup(
    anchor: Rect,
    #[props(default)] align: RectAlign,
    card_class: Option<String>,
    width: Option<u32>,
    children: Element,
) -> Element {
    let card = card_class.unwrap_or_else(|| "ds-menu".to_string());
    let wstyle = width.map(|w| format!("width:{w}px;")).unwrap_or_default();
    // Effective alignment: starts at `align`, then auto-flips after measuring the card if
    // the requested placement would overflow the viewport (egui `find_best_align`).
    let mut eff = use_signal(|| align);
    let pos = eff().style(anchor, 4.0);
    rsx! {
        div {
            class: "{card}",
            style: "position:fixed;{pos}{wstyle}z-index:78;",
            onmounted: move |e| {
                let m = e.data();
                spawn(async move {
                    if let Ok(r) = m.get_client_rect().await {
                        let best = align.best(anchor, (r.size.width, r.size.height), viewport(), 4.0);
                        if *eff.peek() != best {
                            eff.set(best);
                        }
                    }
                });
            },
            onclick: move |e| e.stop_propagation(),
            oncontextmenu: move |e| e.stop_propagation(),
            {children}
        }
    }
}

/// Full-screen dismiss layer for a menu/dropdown: catches an outside click / right-click /
/// Esc and calls `on_close`, and grabs focus so Esc is caught without a document listener.
/// Compose it around a [`Popup`]: `Backdrop { on_close, Popup { anchor, … } }`.
#[component]
pub fn Backdrop(on_close: EventHandler<()>, children: Element) -> Element {
    rsx! {
        div {
            class: "ctx-backdrop",
            tabindex: "0",
            onmounted: move |e| {
                spawn(async move { let _ = e.set_focus(true).await; });
            },
            onmousedown: move |e| e.stop_propagation(),
            onclick: move |_| on_close.call(()),
            oncontextmenu: move |e| {
                e.prevent_default();
                on_close.call(());
            },
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    e.prevent_default();
                    on_close.call(());
                }
            },
            {children}
        }
    }
}
