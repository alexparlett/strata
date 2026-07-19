//! `SearchDialog` — a trigger button that opens a small anchored **search popover** (a
//! find/filter box), built on the [`Popup`]/[`Backdrop`] base (S29). It is deliberately
//! *not* a [`DropdownMenu`](super::dropdown_menu::DropdownMenu): a menu closes on any inner
//! click (its rows are actions), whereas here the popover holds a live text field, so
//! clicks inside must keep it open — which the base [`Popup`] already does by stopping
//! click propagation. Dismissal (Esc / outside-click, via the [`Backdrop`]) and the ✕ both
//! call `on_open(false)`; the caller clears the query on close.
//!
//! **Controlled `open`** (a `bool` + `on_open`), so the state can live wherever the caller
//! keeps it (`runs`, per-tab) and be toggled from elsewhere — e.g. the ⌘F `Find` command.
//! Because that means the popover can open *without* a click on the trigger, the trigger
//! rect is measured both on click and when the popover mounts (see the inner `onmounted`).

use std::rc::Rc;

use dioxus::prelude::*;

use super::popup::{Backdrop, Popup, Rect, RectAlign};
use super::{Icon, TextInput};
use crate::ui::icons::{IconName, IconSize};

/// A find/filter popover anchored to its own trigger button.
#[component]
pub fn SearchDialog(
    /// Trigger button inner content (typically a search [`Icon`]).
    trigger: Element,
    /// Trigger button classes (e.g. `"ds-icon-btn toolbar compact"`).
    #[props(into, default)]
    trigger_class: String,
    /// Trigger tooltip.
    #[props(into, default)]
    title: String,
    /// Current query text.
    #[props(into)]
    value: String,
    /// Fired when the query changes.
    oninput: EventHandler<String>,
    /// Whether the popover is open (controlled).
    open: bool,
    /// Fired to request an open-state change: `true` from the trigger, `false` from the ✕
    /// / backdrop dismiss. The caller owns closing behaviour (e.g. clearing the query).
    on_open: EventHandler<bool>,
    #[props(into, default)] placeholder: String,
    /// Trailing field content (e.g. a match-count label), shown before the ✕.
    trailing: Option<Element>,
    /// Popover width in px.
    #[props(default = 320)]
    width: u32,
    /// Placement relative to the trigger (default below, right-aligned).
    #[props(default = RectAlign::BOTTOM_END)]
    align: RectAlign,
    /// Popover card class (default the `.res-find-panel` search chrome).
    #[props(into, default)]
    card_class: &'static str,
) -> Element {
    let mut anchor = use_signal(|| Rect::point(0.0, 0.0));
    let mut trigger_ref = use_signal(|| None::<Rc<MountedData>>);
    let card = if card_class.is_empty() {
        "res-find-panel"
    } else {
        card_class
    };

    // Measure the trigger and anchor the popover to it. Called on click (so the anchor is
    // ready before the popover mounts) and again when the popover mounts (so opening via
    // the `Find` command — no click — still anchors correctly).
    let measure = move || {
        let handle = trigger_ref.peek().clone();
        spawn(async move {
            let Some(h) = handle else { return };
            if let Ok(r) = h.get_client_rect().await {
                anchor.set(Rect {
                    x: r.origin.x,
                    y: r.origin.y,
                    w: r.size.width,
                    h: r.size.height,
                });
            }
        });
    };

    rsx! {
        button {
            class: "{trigger_class}",
            title: "{title}",
            onmounted: move |e| trigger_ref.set(Some(e.data())),
            onclick: move |_| {
                measure();
                on_open.call(true);
            },
            {trigger}
        }
        if open {
            Backdrop { on_close: move |_| on_open.call(false),
                Popup { anchor: anchor(), align, width, card_class: card,
                    // Re-measure on mount so a keyboard-opened popover anchors correctly;
                    // `display:contents` keeps this wrapper out of the flex layout.
                    div { style: "display:contents", onmounted: move |_| measure(),
                        TextInput {
                            bare: true,
                            grow: true,
                            autofocus: true,
                            mono: true,
                            icon: IconName::Search,
                            value,
                            placeholder,
                            oninput: move |v| oninput.call(v),
                            trailing: rsx! {
                                if let Some(tr) = trailing {
                                    {tr}
                                }
                                span {
                                    class: "res-find-close",
                                    title: "Close (Esc)",
                                    onclick: move |_| on_open.call(false),
                                    Icon { name: IconName::Close, size: IconSize::Xs }
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}
