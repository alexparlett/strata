//! `Badge` + `StatusDot` — the status pills and state dot of the design system
//! (`docs/DESIGN_SYSTEM.md` §07). Part of the **S28** control library.
//!
//! `Badge` is the uppercase-mono pill (CONNECTED / READY / CACHED / ERROR / DRAFT);
//! `StatusDot` is the 8px state dot (with a pulse on the running state). Additive
//! `.ds-badge` / `.ds-dot` classes — the R1 results dot (`.res-dot`) is left as-is
//! and can adopt `StatusDot` during its own pass.

use dioxus::prelude::*;

/// Status-pill variants (§07). The label text is `children` (rendered uppercase).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum BadgeVariant {
    /// Accent-tinted — e.g. CONNECTED.
    Accent,
    /// Green — e.g. READY.
    Ready,
    /// Warm/amber — e.g. CACHED.
    Cached,
    /// Red — e.g. ERROR.
    Error,
    /// Neutral grey — e.g. DRAFT.
    #[default]
    Draft,
}

impl BadgeVariant {
    fn class(self) -> &'static str {
        match self {
            BadgeVariant::Accent => "accent",
            BadgeVariant::Ready => "ready",
            BadgeVariant::Cached => "cached",
            BadgeVariant::Error => "error",
            BadgeVariant::Draft => "draft",
        }
    }
}

/// A status pill. `children` is the (short, uppercased-by-CSS) label. Pass a
/// `color` token (e.g. `"var(--t-map)"`) to render a custom-colour tint outside the
/// semantic palette (a `color-mix` of it) — overrides `variant`.
#[component]
pub fn Badge(
    #[props(default)] variant: BadgeVariant,
    #[props(into, default)] color: String,
    children: Element,
) -> Element {
    if color.is_empty() {
        let cls = format!("ds-badge {}", variant.class());
        rsx! { span { class: "{cls}", {children} } }
    } else {
        let style =
            format!("background: color-mix(in srgb, {color} 15%, transparent); color: {color};");
        rsx! { span { class: "ds-badge", style: "{style}", {children} } }
    }
}

/// The 8px state dot (§07). `Run` pulses; the rest are static colours.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum DotStatus {
    #[default]
    Idle,
    Run,
    Ok,
    Err,
    Plan,
}

impl DotStatus {
    /// (fill colour, pulsing) for the dot.
    fn color(self) -> (&'static str, bool) {
        match self {
            DotStatus::Idle => ("var(--dim2)", false),
            DotStatus::Run => ("var(--accent)", true),
            DotStatus::Ok => ("var(--green)", false),
            DotStatus::Err => ("var(--red2)", false),
            DotStatus::Plan => ("var(--purple)", false),
        }
    }
}

/// The design-system dot (§07) — the single primitive behind every status / indicator /
/// SQL-type dot. `color` is any CSS colour or var (defaults to a neutral dim); `square`
/// renders the rounded-square type swatch; `pulse` adds a running halo (same colour).
#[component]
pub fn Dot(
    #[props(into, default)] color: String,
    #[props(default)] square: bool,
    #[props(default)] pulse: bool,
    /// Diameter in px (default 8).
    #[props(default = 8)]
    size: u32,
    #[props(into, default)] style: String,
) -> Element {
    let c: &str = if color.is_empty() {
        "var(--dim2)"
    } else {
        &color
    };
    let mut cls = String::from("ds-dot");
    if square {
        cls.push_str(" sq");
    }
    if pulse {
        cls.push_str(" pulse");
    }
    let st = format!("--dot-c:{c};background:{c};width:{size}px;height:{size}px;{style}");
    rsx! {
        span { class: "{cls}", style: "{st}" }
    }
}

/// A semantic state dot — colour + pulse driven by `status`. Composes [`Dot`].
#[component]
pub fn StatusDot(#[props(default)] status: DotStatus) -> Element {
    let (color, pulse) = status.color();
    rsx! {
        Dot { color: color, pulse: pulse }
    }
}
