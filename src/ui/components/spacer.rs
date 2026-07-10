//! `Spacer` — a flex spacer (`flex: 1`) that pushes its siblings apart. The tiny
//! layout primitive behind the pervasive `div { class: "spacer" }`.

use dioxus::prelude::*;

#[component]
pub fn Spacer() -> Element {
    rsx! {
        div { class: "spacer" }
    }
}
