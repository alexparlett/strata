//! `Callout` — the left-accent-bar message card of the design system
//! (`docs/DESIGN_SYSTEM.md` §08). Part of the **S28** control library.
//!
//! An inline panel callout: a semantic left bar + icon + message (`children`).
//! `info` / `warn` / `error` set the colour; a default icon is chosen per variant
//! unless `icon` is supplied. (The floating S27 lint popover is the *tooltip*
//! variant of this card, rendered via `Popup{backdrop:false}` — see [`Tooltip`].)

use dioxus::prelude::*;

use crate::ui::icons::{IconName, IconSize};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum CalloutVariant {
    #[default]
    Info,
    Warn,
    Error,
}

impl CalloutVariant {
    fn class(self) -> &'static str {
        match self {
            CalloutVariant::Info => "info",
            CalloutVariant::Warn => "warn",
            CalloutVariant::Error => "err",
        }
    }

    fn default_icon(self) -> IconName {
        match self {
            CalloutVariant::Info => IconName::Info,
            CalloutVariant::Warn => IconName::Warning,
            CalloutVariant::Error => IconName::ErrCircle,
        }
    }
}

/// A semantic callout card. `children` is the message; `icon` overrides the
/// variant's default glyph.
#[component]
pub fn Callout(
    #[props(default)] variant: CalloutVariant,
    icon: Option<IconName>,
    children: Element,
) -> Element {
    let cls = format!("ds-callout {}", variant.class());
    let ico = icon.unwrap_or_else(|| variant.default_icon());
    rsx! {
        div { class: "{cls}",
            span { class: "ds-callout-ico", {ico.el(IconSize::Sm)} }
            span { class: "ds-callout-msg", {children} }
        }
    }
}
