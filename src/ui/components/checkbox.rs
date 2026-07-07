//! `Checkbox` — a small, accessible, controlled checkbox + label row. Our own,
//! modelled on `dioxus-primitives`' `checkbox` primitive (which is unreleased and
//! needs the Dioxus CLI for its bundled assets, so we don't pull it in): a
//! `button` with `role="checkbox"` + `aria-checked`, controlled via `checked` +
//! `on_toggle(new_state)`. The whole row is the toggle target; `children` is the
//! label. Simplified vs the primitive — no indeterminate state or hidden form input.

use dioxus::prelude::*;

use crate::ui::icons;

#[component]
pub fn Checkbox(checked: bool, on_toggle: EventHandler<bool>, children: Element) -> Element {
    rsx! {
        button {
            r#type: "button",
            "role": "checkbox",
            "aria-checked": if checked { "true" } else { "false" },
            class: "chk-row",
            onclick: move |_| on_toggle.call(!checked),
            span {
                class: if checked { "chk on" } else { "chk" },
                "data-state": if checked { "checked" } else { "unchecked" },
                if checked {
                    {icons::check(11)}
                }
            }
            span { class: "chk-label", {children} }
        }
    }
}
