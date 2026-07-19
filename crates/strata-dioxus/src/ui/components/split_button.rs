//! `SplitButton` — the accent split button of the design system
//! (`docs/DESIGN_SYSTEM.md` §07, canvas). Part of the **S28** control library.
//!
//! A primary (accent) face + an attached caret that opens a **solid-accent menu**
//! — the one place a menu carries colour. The face runs `on_main`; the caret
//! toggles the menu (`children` = the accent rows, class `.ds-accent-item`). Pass
//! `show_caret: false` (e.g. while a query runs) to collapse to a plain face.
//! This is the reusable shell the S30 Run control will consume.

use dioxus::prelude::*;

use super::Icon;
use crate::ui::icons::{IconName, IconSize};

#[component]
pub fn SplitButton(
    #[props(into)] label: String,
    #[props(into, default)] kbd: String,
    /// Optional leading icon on the face (a typed [`IconName`]).
    icon: Option<IconName>,
    /// Leading-icon size (default `IconSize::Sm`, 14px).
    #[props(default = IconSize::Sm)]
    icon_size: IconSize,
    #[props(default)] disabled: bool,
    /// Show the caret + menu affordance (default true).
    #[props(default = true)]
    show_caret: bool,
    /// Face (primary action) click.
    on_main: EventHandler<()>,
    /// Accent-menu rows (`.ds-accent-item` buttons); an inside click closes the menu.
    children: Element,
) -> Element {
    let mut open = use_signal(|| false);
    rsx! {
        div { class: "ds-split",
            button {
                r#type: "button",
                class: if show_caret { "ds-split-main" } else { "ds-split-main solo" },
                disabled: disabled,
                onclick: move |_| { if !disabled { on_main.call(()); } },
                if let Some(ic) = icon {
                    span { class: "ds-split-ico", {ic.el(icon_size)} }
                }
                span { class: "ds-split-label", "{label}" }
                if !kbd.is_empty() {
                    span { class: "ds-split-kbd", "{kbd}" }
                }
            }
            if show_caret {
                button {
                    r#type: "button",
                    class: "ds-split-caret",
                    disabled: disabled,
                    "aria-haspopup": "menu",
                    "aria-expanded": if open() { "true" } else { "false" },
                    onclick: move |_| { if !disabled { let v = !open(); open.set(v); } },
                    Icon { name: IconName::ChevronDown, size: IconSize::Xs }
                }
            }
            if open() {
                div { class: "ds-split-backdrop", onclick: move |_| open.set(false) }
                div {
                    class: "ds-split-menu",
                    "role": "menu",
                    onclick: move |_| open.set(false),
                    {children}
                }
            }
        }
    }
}
