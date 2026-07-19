//! Typography — a base [`Text`] span plus **one component per concrete role** from
//! the design-system §02 ramp (v17: 6 sizes × 3 weights). The app uses these instead
//! of inline `font:` so the type scale is the single source of truth. Each role fixes
//! weight + size + family (via a `--t-*` var behind its `.ds-txt-*` class); colour and
//! layout tweaks ride the `class` / `style` passthrough (never re-declare `font` there).
//! Deliberately **not** an enum-variant API — every role is its own component so call
//! sites read `Title { … }`, `Meta { … }`.
//!
//! Roles (v18): display **Hero** (600/26) · **Metric** (600/22 mono) · **Code** (500/20
//! mono); UI text **Title** (600/14.5) · **Strong** (600/13) · **Body** (500/13) ·
//! **Control** (600/12.5, button/control label) · **Prose** (400/12.5) · **Caption**
//! (400/11); mono **MonoValue** (500/12.5, discrete value) · **Readout** (400/12, flowing
//! code & data block) · **Eyebrow** (600/10, field label, upper) · **Meta** (500/10) ·
//! **Path** (400/11, recessive path/subtitle) · **Micro** (600/9, upper).

use dioxus::prelude::*;

/// The base text span (`.ds-txt`). Prefer a concrete role component below; use this
/// directly only for a one-off that carries just utility classes. `class` and `style`
/// pass through for colour / layout — do **not** put `font` in `style` (the role owns it).
#[component]
pub fn Text(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    let cls = format!("ds-txt {class}");
    rsx! {
        span { class: "{cls}", style: "{style}", {children} }
    }
}

/// Hero display (600 · 26px). Specimens / welcome only.
#[component]
pub fn Hero(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-hero {class}", style: style, {children} } }
}

/// Metric / data value (mono 600 · 22px).
#[component]
pub fn Metric(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-metric {class}", style: style, {children} } }
}

/// Inline code / SQL (mono 500 · 20px).
#[component]
pub fn Code(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-code {class}", style: style, {children} } }
}

/// Title / window (600 · 14.5px).
#[component]
pub fn Title(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-title {class}", style: style, {children} } }
}

/// Strong body — emphasised (600 · 13px).
#[component]
pub fn Strong(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-strong {class}", style: style, {children} } }
}

/// Body medium — default UI text (500 · 13px).
#[component]
pub fn Body(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-body {class}", style: style, {children} } }
}

/// Control label — button / control text (600 · 12.5px). The control tier.
#[component]
pub fn Control(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-control {class}", style: style, {children} } }
}

/// Body regular — descriptions & secondary prose (400 · 12.5px).
#[component]
pub fn Prose(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-prose {class}", style: style, {children} } }
}

/// Caption — small supporting text (400 · 11px).
#[component]
pub fn Caption(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-caption {class}", style: style, {children} } }
}

/// Mono value — inline data / paths / figures (mono 500 · 12.5px).
#[component]
pub fn MonoValue(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-mono {class}", style: style, {children} } }
}

/// Readout — flowing mono code & data block (mono 400 · 12px). Editor / plan / cell /
/// error readouts; block-level so multi-line content wraps naturally.
#[component]
pub fn Readout(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-readout {class}", style: style, {children} } }
}

/// Eyebrow — uppercase field label (mono 600 · 10px, tracked).
#[component]
pub fn Eyebrow(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-eyebrow {class}", style: style, {children} } }
}

/// Meta — recessive mono label / timestamp (mono 500 · 10px).
#[component]
pub fn Meta(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-meta {class}", style: style, {children} } }
}

/// Path — recessive mono path / subtitle / footer (mono 400 · 11px). Recessive by
/// weight *and* colour; the flowing sibling of [`Meta`].
#[component]
pub fn Path(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-path {class}", style: style, {children} } }
}

/// Micro — smallest badge / column-header tier (mono 600 · 9px, upper).
#[component]
pub fn Micro(
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    children: Element,
) -> Element {
    rsx! { Text { class: "ds-txt-micro {class}", style: style, {children} } }
}
