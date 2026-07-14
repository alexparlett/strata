//! Settings ▸ Appearance page — Sync-with-OS + the theme preview grid.

use dioxus::prelude::*;

use crate::config::Settings;
use crate::ui::components::{Body, Caption, Icon, Micro, Spacer, Strong, Toggle};
use crate::ui::icons::{IconName, IconSize};

/// Sync-with-OS toggle + theme grid. Reads the shared draft; picking a theme edits the
/// draft **and** previews it live across every window.
#[component]
pub(super) fn Appearance() -> Element {
    let ctx = use_context::<super::SettingsCtx>();
    let mut draft = ctx.draft;
    let d = draft.read();
    let sync_os = d.sync_os;
    let theme_id = d.theme.clone();
    drop(d);
    let os_dark = *crate::settings::OS_DARK.read();
    let active_id = crate::theme::effective_id(&theme_id, sync_os, os_dark);
    let os_label = if os_dark { "dark" } else { "light" };
    let grid_style = if sync_os {
        "opacity:.45;pointer-events:none;"
    } else {
        ""
    };
    rsx! {
        super::Anchor { id: "sync-os",
            div { class: "settings-row", style: "margin-bottom:var(--sp-6);",
                div { style: "flex:1;",
                    Strong { style: "display:block;", "Sync with OS" }
                    Caption { style: "display:block;color:var(--dim2);margin-top:var(--sp-1);", "Match your system light/dark appearance automatically." }
                }
                Toggle {
                    on: sync_os,
                    on_toggle: move |_| {
                        let v = !sync_os;
                        draft.write().sync_os = v;
                        crate::settings::preview_sync_os(v);
                    },
                }
            }
        }
        div { class: "settings-divider" }
        super::Anchor { id: "theme",
            Strong { style: "display:block;margin:var(--sp-5) 0 var(--sp-4);", "Theme" }
            if sync_os {
                Caption { style: "display:block;margin-bottom:var(--sp-4);", "Following your system appearance ({os_label}). Turn off Sync with OS to choose a theme." }
            }
            div { class: "theme-grid", style: "{grid_style}",
                for t in crate::theme::registry() {
                    {theme_card(t, &active_id, draft)}
                }
            }
        }
    }
}

/// One theme preview card — a mini mockup rendered in the theme's own colours, plus name
/// / source badge / active check. Clicking edits the draft's theme + previews it live.
fn theme_card(
    t: &crate::theme::ResolvedTheme,
    active_id: &str,
    mut draft: Signal<Settings>,
) -> Element {
    let id = t.id.clone();
    let name = t.name.clone();
    let active = t.id == active_id;
    let p0 = t.color("bg").to_string();
    let p1 = t.color("panel").to_string();
    let p2 = t.color("dim2").to_string();
    let p3 = t.color("accent").to_string();
    let source = match t.source {
        crate::theme::Source::Builtin => "Built-in",
        crate::theme::Source::User => "User",
        crate::theme::Source::Plugin => "Plugin",
    };
    let ring = if active {
        "var(--accent)"
    } else {
        "var(--line2)"
    };
    let ringw = if active { "2px" } else { "1px" };
    rsx! {
        button {
            class: "theme-card",
            style: "border:{ringw} solid {ring};",
            onclick: move |_| {
                draft.write().theme = id.clone();
                crate::settings::preview_theme(id.clone());
            },
            div { style: "height:78px;display:flex;flex-direction:column;background:{p0};",
                div { style: "height:16px;background:{p1};display:flex;align-items:center;padding:0 var(--sp-3);gap:var(--sp-2);",
                    span { style: "width:5px;height:5px;border-radius:50%;background:{p2};" }
                    span { style: "width:34px;height:4px;border-radius:var(--r-xs);background:{p2};" }
                }
                div { style: "flex:1;display:flex;gap:var(--sp-3);padding:var(--sp-3);",
                    div { style: "width:26px;border-radius:var(--r-xs);background:{p1};" }
                    div { style: "flex:1;display:flex;flex-direction:column;gap:var(--sp-2);",
                        span { style: "width:70%;height:5px;border-radius:var(--r-xs);background:{p3};" }
                        span { style: "width:45%;height:5px;border-radius:var(--r-xs);background:{p2};" }
                        span { style: "width:55%;height:5px;border-radius:var(--r-xs);background:{p2};" }
                    }
                }
            }
            div { class: "theme-cardfoot",
                Body { "{name}" }
                Micro { class: "theme-src", "{source}" }
                Spacer {}
                if active { Icon { name: IconName::Check, size: IconSize::Sm, color: "var(--accent)" } }
            }
        }
    }
}
