//! [`Icon`] — the single primitive for rendering a glyph (S28, design system §07).
//!
//! Wraps a typed [`IconName`] SVG in an `inline-flex` span (`.ds-ico`) so it aligns
//! optically with adjacent text on its own — no `span { display:flex … }` wrapper at
//! the call site. Colour comes from `currentColor`, so it inherits by default; pass
//! `color` to override. For a control's own icon slot (which already wraps the glyph)
//! use `IconName::el(size)` directly instead of nesting an `Icon`.

use dioxus::prelude::*;

use crate::ui::icons::{IconName, IconSize};

#[component]
pub fn Icon(
    /// Which glyph — a compile-checked enum, not a string.
    name: IconName,
    /// Size on the §07 scale. Defaults to `IconSize::Md` (16px).
    #[props(default = IconSize::Md)]
    size: IconSize,
    /// Optional colour override (any CSS colour/var); inherits `currentColor` if unset.
    #[props(into, default)]
    color: String,
    /// Extra class(es) merged onto `.ds-ico`.
    #[props(into, default)]
    class: String,
    /// Inline style passthrough (appended after any `color`).
    #[props(into, default)]
    style: String,
) -> Element {
    let cls = if class.is_empty() {
        "ds-ico".to_string()
    } else {
        format!("ds-ico {class}")
    };
    let mut st = String::new();
    if !color.is_empty() {
        st.push_str("color:");
        st.push_str(&color);
        st.push(';');
    }
    st.push_str(&style);
    rsx! {
        span { class: "{cls}", style: "{st}", {name.el(size)} }
    }
}
