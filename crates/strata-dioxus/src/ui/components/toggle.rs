//! `Toggle` — the on/off switch of the design system (`docs/DESIGN_SYSTEM.md`
//! §06). Part of the **S28** control library.
//!
//! A `role="switch"` button: a 34×19 track + 15px knob, with an optional trailing
//! label (`children`; a `:empty` label collapses so a bare switch has no gap).
//! Controlled via `on` + `on_toggle(new_state)` — mirrors [`Checkbox`](super::Checkbox).

use dioxus::prelude::*;

#[component]
pub fn Toggle(
    on: bool,
    on_toggle: EventHandler<bool>,
    #[props(default)] disabled: bool,
    /// "Available but off" affordance — a highlighted off-track + accent ring hinting
    /// the switch *can* be turned on (e.g. hive partitioning once every source is a
    /// directory). Ignored when `on`.
    #[props(default)]
    avail: bool,
    /// Optional helper line beneath the label (§22 settings row).
    #[props(into, default)]
    sub: String,
    children: Element,
) -> Element {
    let track = if on {
        "ds-toggle on"
    } else if avail {
        "ds-toggle avail"
    } else {
        "ds-toggle"
    };
    let has_sub = !sub.is_empty();
    rsx! {
        button {
            r#type: "button",
            "role": "switch",
            "aria-checked": if on { "true" } else { "false" },
            class: if disabled { "ds-toggle-row disabled" } else { "ds-toggle-row" },
            disabled: disabled,
            onclick: move |_| { if !disabled { on_toggle.call(!on); } },
            span {
                class: "{track}",
                span { class: "ds-toggle-knob" }
            }
            if has_sub {
                span { class: "ds-toggle-text",
                    span { class: "ds-toggle-label", {children} }
                    span { class: "ds-toggle-sub", "{sub}" }
                }
            } else {
                span { class: "ds-toggle-label", {children} }
            }
        }
    }
}
