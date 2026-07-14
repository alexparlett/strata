//! Settings ▸ System page — startup, default project directory, open behaviour, safety.

use dioxus::prelude::*;

use crate::ui::components::{
    Caption, Eyebrow, IconButton, IconButtonVariant, Segment, SegmentOption, Strong, TextInput,
    Toggle,
};
use crate::ui::icons::IconName;

#[component]
pub(super) fn System() -> Element {
    let mut draft = use_context::<super::SettingsCtx>().draft;
    let d = draft.read();
    let reopen = d.reopen_on_startup;
    let default_dir = d.default_project_dir.clone();
    let open_pref = d.open_pref;
    let confirm_close = d.confirm_close_running;
    let max_history = d.max_history;
    drop(d);
    rsx! {
        Eyebrow { class: "settings-sublabel", "STARTUP" }
        super::Anchor { id: "reopen",
            div { class: "settings-row",
                div { style: "flex:1;",
                    Strong { style: "display:block;", "Reopen projects on startup" }
                    Caption { style: "display:block;color:var(--dim2);margin-top:var(--sp-1);", "Reopen the projects you had open when you last quit." }
                }
                Toggle {
                    on: reopen,
                    on_toggle: move |_| { let v = !reopen; draft.write().reopen_on_startup = v; },
                }
            }
        }
        div { class: "settings-divider", style: "margin:var(--sp-6) 0;" }
        Eyebrow { class: "settings-sublabel", "PROJECTS" }
        super::Anchor { id: "default-dir",
            Strong { style: "display:block;margin-bottom:var(--sp-1);", "Default project directory" }
            Caption { style: "display:block;color:var(--dim2);margin-bottom:var(--sp-4);", "Preselected in the Open dialog. Leave blank to use your last location." }
            div { class: "row", style: "gap:var(--sp-3);margin-bottom:var(--sp-6);",
                TextInput { value: "{default_dir}", mono: true, grow: true, placeholder: "~/data",
                    oninput: move |_| {},
                    onchange: move |v| { draft.write().default_project_dir = v; } }
                IconButton { icon: IconName::Folder, variant: IconButtonVariant::Toolbar, title: "Choose…",
                    onclick: move |_| { spawn(async move {
                        if let Some(h) = rfd::AsyncFileDialog::new().pick_folder().await {
                            let p = h.path().to_string_lossy().into_owned();
                            draft.write().default_project_dir = p;
                        }
                    }); },
                }
            }
        }
        super::Anchor { id: "open-pref",
            Strong { style: "display:block;margin-bottom:var(--sp-2);", "Opening a project" }
            Caption { style: "display:block;color:var(--dim2);margin-bottom:var(--sp-4);", "When you open a project from a window that already has one, where should it open?" }
            Segment {
                value: match open_pref {
                    crate::config::OpenPref::Ask => "ask",
                    crate::config::OpenPref::This => "this",
                    crate::config::OpenPref::New => "new",
                },
                on_select: move |v: String| {
                    let p = match v.as_str() {
                        "this" => crate::config::OpenPref::This,
                        "new" => crate::config::OpenPref::New,
                        _ => crate::config::OpenPref::Ask,
                    };
                    draft.write().open_pref = p;
                },
                options: vec![
                    SegmentOption::new("ask", "Ask every time"),
                    SegmentOption::new("this", "This window"),
                    SegmentOption::new("new", "New window"),
                ],
            }
        }
        div { class: "settings-divider", style: "margin:var(--sp-6) 0;" }
        Eyebrow { class: "settings-sublabel", "SAFETY" }
        super::Anchor { id: "confirm-close",
            div { class: "settings-row",
                div { style: "flex:1;",
                    Strong { style: "display:block;", "Confirm before closing a tab or window with a running query" }
                    Caption { style: "display:block;color:var(--dim2);margin-top:var(--sp-1);", "Asks only when a scan is in flight — silent otherwise." }
                }
                Toggle {
                    on: confirm_close,
                    on_toggle: move |_| { let v = !confirm_close; draft.write().confirm_close_running = v; },
                }
            }
        }
        div { class: "settings-divider", style: "margin:var(--sp-6) 0;" }
        Eyebrow { class: "settings-sublabel", "HISTORY" }
        super::Anchor { id: "max-history",
            Strong { style: "display:block;margin-bottom:var(--sp-2);", "Query history limit" }
            Caption { style: "display:block;color:var(--dim2);margin-bottom:var(--sp-4);", "How many past runs to keep in the history panel. Older entries drop off once the cap is reached." }
            Segment {
                value: "{max_history}",
                on_select: move |v: String| {
                    if let Ok(n) = v.parse::<usize>() { draft.write().max_history = n; }
                },
                options: vec![
                    SegmentOption::new("25", "25"),
                    SegmentOption::new("50", "50"),
                    SegmentOption::new("100", "100"),
                    SegmentOption::new("200", "200"),
                ],
            }
        }
    }
}
