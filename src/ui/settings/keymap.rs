//! Settings ▸ Keymap page — read-only list of commands + their live bindings.

use dioxus::prelude::*;

use crate::ui::components::{Body, Caption, Eyebrow};

#[component]
pub(super) fn Keymap() -> Element {
    rsx! {
        super::Anchor { id: "keymap",
            Caption { style: "display:block;color:var(--dim2);margin-bottom:var(--sp-5);", "Read-only. ⌘ shortcuts also respond to Ctrl." }
            div { class: "keymap-box",
                for cmd in crate::keymap::ALL_COMMANDS {
                    {keymap_row(cmd)}
                }
            }
        }
    }
}

/// One read-only row — a command's label + description and its live, override-aware
/// chord, rendered straight from `crate::keymap` so the list can't drift from the real
/// bindings saved in `Settings::keybinds`.
fn keymap_row(cmd: crate::config::Command) -> Element {
    let (label, desc) = crate::keymap::describe(cmd);
    let caps = crate::keymap::chord_caps(&crate::keymap::effective_chord(cmd));
    rsx! {
        div { class: "keymap-row",
            div { style: "flex:1;min-width:0;",
                Body { style: "display:block;", "{label}" }
                Caption { style: "display:block;color:var(--dim2);margin-top:var(--sp-1);", "{desc}" }
            }
            div { class: "row", style: "gap:var(--sp-2);flex:none;",
                for cap in caps.iter() {
                    Eyebrow { class: "keycap", "{cap}" }
                }
            }
        }
    }
}
