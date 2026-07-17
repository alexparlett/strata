//! Engine-restart prompt (W2): a saved `datafusion.runtime.*` change (memory / spill
//! limits) can't be applied to the running engine — its `RuntimeEnv` is fixed at
//! build — so this asks whether to restart the window now. **Restart** respawns the
//! window for the same project (its fresh engine picks up the new limits); **Not now**
//! leaves the change to apply the next time the window opens.
//!
//! Always-mounted host reading the per-window overlay store (`crate::overlays`), like
//! the other confirm dialogs; renders nothing until the prompt is up.

use dioxus::prelude::*;

use crate::ui::components::{Button, ButtonVariant, Dialog, Icon, Prose, Title};
use crate::ui::icons::{IconName, IconSize};

#[component]
pub fn EngineRestartHost() -> Element {
    if !crate::overlays::OVERLAYS.resolve().read().engine_restart {
        return rsx! {};
    }
    rsx! {
        Dialog { on_close: move |_| crate::overlays::close_engine_restart(), card_class: "confirm".to_string(), z: 80,
            div { class: "confirm-pad",
                div { class: "confirm-head-row",
                    div { class: "confirm-ico accent", Icon { name: IconName::Refresh, size: IconSize::Lg } }
                    div { style: "min-width:0;",
                        Title { class: "confirm-title", "Restart to apply" }
                        Prose { class: "confirm-sub", "Engine settings" }
                    }
                }
                Prose { class: "confirm-msg", "Changes require a restart. Would you like to restart now?" }
            }
            div { class: "confirm-foot split",
                Button { variant: ButtonVariant::Secondary, onclick: move |_| crate::overlays::close_engine_restart(), "Not now" }
                Button {
                    variant: ButtonVariant::Primary,
                    onclick: move |_| {
                        crate::overlays::close_engine_restart();
                        crate::action::projects::restart_window();
                    },
                    "Restart"
                }
            }
        }
    }
}
