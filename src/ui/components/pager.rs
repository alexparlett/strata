//! `Pager` — the pagination cluster of the design system (`docs/DESIGN_SYSTEM.md`
//! §03 "IN CONTEXT"). Part of the **S28** control library.
//!
//! First / prev / jump-input / "of N" / next / last, built on the `Pager` icon-
//! button variant. Controlled: the caller owns `page` and reacts to `on_jump`
//! (already clamped to `[1, page_count]`). The page-size chooser is a separate
//! [`Select`](super::Select) placed alongside — not part of this cluster.

use dioxus::prelude::*;

use super::{IconButton, IconButtonVariant};
use crate::ui::icons::{IconName, IconSize};

#[component]
pub fn Pager(
    /// 1-based current page.
    page: u32,
    /// Total pages (coerced to at least 1).
    page_count: u32,
    /// Jump to a page — the value is pre-clamped to `[1, page_count]`.
    on_jump: EventHandler<u32>,
) -> Element {
    let pc = page_count.max(1);
    let p = page.clamp(1, pc);
    let at_first = p <= 1;
    let at_last = p >= pc;
    rsx! {
        div { class: "ds-pager",
            IconButton { icon: IconName::First,
                variant: IconButtonVariant::Pager,
                disabled: at_first,
                title: "First page",
                onclick: move |_| on_jump.call(1),
            }
            IconButton { icon: IconName::Prev,
                variant: IconButtonVariant::Pager,
                disabled: at_first,
                title: "Previous",
                onclick: move |_| on_jump.call(p.saturating_sub(1).max(1)),
            }
            div { class: "ds-pager-jump",
                input {
                    class: "ds-pager-input",
                    r#type: "text",
                    "inputmode": "numeric",
                    value: "{p}",
                    spellcheck: false,
                    onchange: move |e| {
                        if let Ok(n) = e.value().trim().parse::<u32>() {
                            on_jump.call(n.clamp(1, pc));
                        }
                    },
                }
                span { class: "ds-pager-of", "of {pc}" }
            }
            IconButton { icon: IconName::Next,
                variant: IconButtonVariant::Pager,
                disabled: at_last,
                title: "Next",
                onclick: move |_| on_jump.call((p + 1).min(pc)),
            }
            IconButton { icon: IconName::Last,
                variant: IconButtonVariant::Pager,
                disabled: at_last,
                title: "Last page",
                onclick: move |_| on_jump.call(pc),
            }
        }
    }
}
