//! Settings modal: Appearance / Data display / System / Keymap.
use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, SettingsCat};
use crate::ui::components::{WinGeom, Window};
use crate::ui::icons;

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Global shortcuts shown read-only in the Keymap category: (label, desc, keys).
const KEYMAP: &[(&str, &str, &[&str])] = &[
    ("Command palette", "Search tables, columns & commands", &["⌘", "K"]),
    ("New query tab", "Open a fresh SQL tab", &["⌘", "T"]),
    ("Reopen closed tab", "Restore the last tab you closed", &["⇧", "⌘", "T"]),
    ("Close tab", "Close the active query tab", &["⌘", "W"]),
    ("Save query", "Save the active query to the project", &["⌘", "S"]),
    ("Run query", "Execute the current SQL", &["⌘", "↵"]),
    ("Settings", "Open this panel", &["⌘", ","]),
    ("Cycle windows", "Focus the next project window", &["⌘", "`"]),
    ("Dismiss", "Close overlays & menus", &["Esc"]),
];

/// Left-nav category icon.
fn settings_cat_icon(name: &str) -> Element {
    match name {
        "palette" => icons::palette(15),
        "grid" => icons::grid(15),
        "sliders" => icons::sliders(15),
        "keyboard" => icons::keyboard(15),
        _ => rsx! {},
    }
}

/// Always-mounted host for the Settings window. Reads the overlay store reactively
/// (so it re-renders when `settings` flips) and renders nothing until it's open —
/// visibility is derived during render, no local state, no effect. Triggers (the
/// header gear, ⌘,) call `overlays::toggle_settings`. See
/// `docs/OVERLAY_ARCHITECTURE.md`.
#[component]
pub fn SettingsHost() -> Element {
    if !crate::overlays::OVERLAYS.read().settings {
        return rsx! {};
    }
    rsx! {
        SettingsModal { on_close: move |_| crate::overlays::set_settings(false) }
    }
}

#[component]
pub fn SettingsModal(on_close: EventHandler<()>) -> Element {
    let state = use_context::<Signal<AppState>>();
    // The active category is transient UI state, local to this window.
    let mut cat_sig = use_signal(|| SettingsCat::Appearance);
    let cat = cat_sig();
    let s = state.read();
    let theme_id = s.theme_id.clone();
    let sync_os = s.sync_os;
    let os_dark = s.os_dark;
    let density_compact = s.density_compact;
    let zebra = s.zebra;
    let row_limit = s.row_limit;
    let reopen = s.reopen_on_startup;
    let default_dir = s.default_project_dir.clone();
    let open_pref = s.open_pref.clone();
    let confirm_close = s.confirm_close_running;
    drop(s);

    // When Sync-with-OS is on, the effective theme follows the system appearance
    // and the grid is disabled.
    let active_id = crate::theme::effective_id(&theme_id, sync_os, os_dark);
    let crumb = match cat {
        SettingsCat::Appearance => "Appearance",
        SettingsCat::DataDisplay => "Data display",
        SettingsCat::System => "System",
        SettingsCat::Keymap => "Keymap",
    };
    let grid_style = if sync_os { "opacity:.45;pointer-events:none;" } else { "" };
    let os_label = if os_dark { "dark" } else { "light" };

    rsx! {
        Window {
            on_close: move |_| on_close.call(()),
            title: "Settings".to_string(),
            subtitle: "appearance & behavior".to_string(),
            icon: icons::gear(16),
            init: WinGeom::new(260.0, 90.0, 760.0, 600.0),
            min_w: 640.0,
            min_h: 440.0,
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn accent", style: "height:34px;", onclick: move |_| on_close.call(()), "Done" }
            },
            div { class: "settings-body",
                    div { class: "settings-nav",
                        div { class: "settings-navlabel", "SETTINGS" }
                        for (c, label, ic) in [
                            (SettingsCat::Appearance, "Appearance", "palette"),
                            (SettingsCat::DataDisplay, "Data display", "grid"),
                            (SettingsCat::System, "System", "sliders"),
                            (SettingsCat::Keymap, "Keymap", "keyboard"),
                        ] {
                            button {
                                class: if cat == c { "settings-nav-item on" } else { "settings-nav-item" },
                                onclick: move |_| cat_sig.set(c),
                                span { class: "sn-ic", {settings_cat_icon(ic)} }
                                span { "{label}" }
                            }
                        }
                    }
                    div { class: "settings-pane ps-scroll",
                        div { class: "settings-crumb",
                            "Settings " span { style: "color:var(--faint2);", "›" } " "
                            span { style: "color:var(--text3);", "{crumb}" }
                        }
                        match cat {
                            SettingsCat::Appearance => rsx! {
                                div { class: "settings-row", style: "cursor:pointer;margin-bottom:20px;",
                                    onclick: move |_| dispatch(state, Action::ToggleSyncOs),
                                    div { style: "flex:1;",
                                        div { style: "font:600 13px var(--ui);color:var(--text);", "Sync with OS" }
                                        div { style: "font-size:11.5px;color:var(--dim2);margin-top:3px;", "Match your system light/dark appearance automatically." }
                                    }
                                    div { class: if sync_os { "toggle on" } else { "toggle" }, div { class: "knob" } }
                                }
                                div { class: "settings-divider" }
                                div { style: "font:600 13px var(--ui);color:var(--text);margin:16px 0 12px;", "Theme" }
                                if sync_os {
                                    div { style: "margin-bottom:12px;font-size:11.5px;color:var(--dim);", "Following your system appearance ({os_label}). Turn off Sync with OS to choose a theme." }
                                }
                                div { class: "theme-grid", style: "{grid_style}",
                                    for t in crate::theme::registry() {
                                        {theme_card(state, t, &active_id)}
                                    }
                                }
                            },
                            SettingsCat::DataDisplay => rsx! {
                                div { style: "font:600 13px var(--ui);color:var(--text);margin-bottom:12px;", "Row density" }
                                div { class: "seg-row",
                                    for (val, label) in [(false, "Comfortable"), (true, "Compact")] {
                                        button {
                                            class: if density_compact == val { "seg-btn on" } else { "seg-btn" },
                                            onclick: move |_| dispatch(state, Action::SetDensity(val)),
                                            "{label}"
                                        }
                                    }
                                }
                                div { style: "font-size:11.5px;color:var(--dim2);margin-top:10px;", "Controls row height in the results grid and catalog." }
                                div { class: "settings-divider", style: "margin:22px 0;" }
                                div { class: "settings-row", style: "cursor:pointer;",
                                    onclick: move |_| dispatch(state, Action::ToggleZebra),
                                    div { style: "flex:1;",
                                        div { style: "font:600 13px var(--ui);color:var(--text);", "Alternating row colours" }
                                        div { style: "font-size:11.5px;color:var(--dim2);margin-top:3px;", "Shade every other row in the results grid for easier scanning." }
                                    }
                                    div { class: if zebra { "toggle on" } else { "toggle" }, div { class: "knob" } }
                                }
                                div { class: "settings-divider", style: "margin:22px 0;" }
                                div { style: "font:600 13px var(--ui);color:var(--text);margin-bottom:3px;", "Default row limit" }
                                div { style: "font-size:11.5px;color:var(--dim2);margin-bottom:12px;", "New query tabs are generated with this LIMIT so a stray SELECT * can't pull a whole file into memory." }
                                div { class: "seg-row",
                                    for (val, label) in [(100usize, "100"), (1000, "1,000"), (10000, "10,000"), (0, "No limit")] {
                                        button {
                                            class: if row_limit == val { "seg-btn on" } else { "seg-btn" },
                                            onclick: move |_| dispatch(state, Action::SetRowLimit(val)),
                                            "{label}"
                                        }
                                    }
                                }
                            },
                            SettingsCat::System => rsx! {
                                div { class: "settings-sublabel", "STARTUP" }
                                div { class: "settings-row", style: "cursor:pointer;",
                                    onclick: move |_| dispatch(state, Action::ToggleReopenStartup),
                                    div { style: "flex:1;",
                                        div { style: "font:600 13px var(--ui);color:var(--text);", "Reopen last project on startup" }
                                        div { style: "font-size:11.5px;color:var(--dim2);margin-top:3px;", "Jump straight back into the project you had open." }
                                    }
                                    div { class: if reopen { "toggle on" } else { "toggle" }, div { class: "knob" } }
                                }
                                div { class: "settings-divider", style: "margin:22px 0;" }
                                div { class: "settings-sublabel", "PROJECTS" }
                                div { style: "font:600 13px var(--ui);color:var(--text);margin-bottom:3px;", "Default project directory" }
                                div { style: "font-size:11.5px;color:var(--dim2);margin-bottom:10px;", "Preselected in the Open dialog. Leave blank to use your last location." }
                                div { class: "row", style: "gap:8px;margin-bottom:22px;",
                                    input { class: "text-input", style: "flex:1;font-family:var(--mono);", value: "{default_dir}", placeholder: "~/data",
                                        onchange: move |e| dispatch(state, Action::SetDefaultProjectDir(e.value())) }
                                    button { class: "mini-btn", style: "width:38px;height:34px;", title: "Choose…",
                                        onclick: move |_| { spawn(async move {
                                            if let Some(h) = rfd::AsyncFileDialog::new().pick_folder().await {
                                                let p = h.path().to_string_lossy().into_owned();
                                                dispatch(state, Action::SetDefaultProjectDir(p));
                                            }
                                        }); },
                                        {icons::folder(15)}
                                    }
                                }
                                div { style: "font:600 13px var(--ui);color:var(--text);margin-bottom:4px;", "Opening a project" }
                                div { style: "font-size:11.5px;color:var(--dim2);margin-bottom:12px;", "When you open a project from a window that already has one, where should it open?" }
                                div { class: "seg-row",
                                    for (val, label) in [("ask", "Ask every time"), ("this", "This window"), ("new", "New window")] {
                                        button {
                                            class: if open_pref == val { "seg-btn on" } else { "seg-btn" },
                                            onclick: move |_| dispatch(state, Action::SetOpenPref(val.to_string())),
                                            "{label}"
                                        }
                                    }
                                }
                                div { class: "settings-divider", style: "margin:22px 0;" }
                                div { class: "settings-sublabel", "SAFETY" }
                                div { class: "settings-row", style: "cursor:pointer;",
                                    onclick: move |_| dispatch(state, Action::ToggleConfirmClose),
                                    div { style: "flex:1;",
                                        div { style: "font:600 13px var(--ui);color:var(--text);", "Confirm before closing a window with a running query" }
                                        div { style: "font-size:11.5px;color:var(--dim2);margin-top:3px;", "Asks only when a scan is in flight — silent otherwise." }
                                    }
                                    div { class: if confirm_close { "toggle on" } else { "toggle" }, div { class: "knob" } }
                                }
                            },
                            SettingsCat::Keymap => rsx! {
                                div { style: "font-size:11.5px;color:var(--dim2);margin-bottom:16px;", "Read-only. ⌘ shortcuts also respond to Ctrl." }
                                div { class: "keymap-box",
                                    for (label, desc, keys) in KEYMAP {
                                        div { class: "keymap-row",
                                            div { style: "flex:1;min-width:0;",
                                                div { style: "font:500 12.5px var(--ui);color:var(--text);", "{label}" }
                                                div { style: "font-size:11px;color:var(--dim2);margin-top:2px;", "{desc}" }
                                            }
                                            div { class: "row", style: "gap:5px;flex:none;",
                                                for cap in keys.iter() {
                                                    span { class: "keycap", "{cap}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            },
                        }
                    }
                }
        }
    }
}

/// One theme preview card in the Appearance grid — a mini mockup rendered in the
/// theme's own colours, plus name / source badge / active check.
fn theme_card(state: Signal<AppState>, t: &crate::theme::ResolvedTheme, active_id: &str) -> Element {
    let id = t.id.clone();
    let name = t.name.clone();
    let active = t.id == active_id;
    // Preview colours pulled from the theme itself.
    let p0 = t.color("bg").to_string();
    let p1 = t.color("panel").to_string();
    let p2 = t.color("dim2").to_string();
    let p3 = t.color("accent").to_string();
    let source = match t.source {
        crate::theme::Source::Builtin => "Built-in",
        crate::theme::Source::User => "User",
        crate::theme::Source::Plugin => "Plugin",
    };
    let ring = if active { "var(--accent)" } else { "var(--line2)" };
    let ringw = if active { "2px" } else { "1px" };
    rsx! {
        button {
            class: "theme-card",
            style: "border:{ringw} solid {ring};",
            onclick: move |_| dispatch(state, Action::SetTheme(id.clone())),
            // mini mockup
            div { style: "height:78px;display:flex;flex-direction:column;background:{p0};",
                div { style: "height:16px;background:{p1};display:flex;align-items:center;padding:0 8px;gap:4px;",
                    span { style: "width:5px;height:5px;border-radius:50%;background:{p2};" }
                    span { style: "width:34px;height:4px;border-radius:2px;background:{p2};" }
                }
                div { style: "flex:1;display:flex;gap:6px;padding:8px;",
                    div { style: "width:26px;border-radius:4px;background:{p1};" }
                    div { style: "flex:1;display:flex;flex-direction:column;gap:4px;",
                        span { style: "width:70%;height:5px;border-radius:3px;background:{p3};" }
                        span { style: "width:45%;height:5px;border-radius:3px;background:{p2};" }
                        span { style: "width:55%;height:5px;border-radius:3px;background:{p2};" }
                    }
                }
            }
            div { class: "theme-cardfoot",
                span { style: "font:500 12.5px var(--ui);color:var(--text);", "{name}" }
                span { class: "theme-src", "{source}" }
                div { style: "flex:1;" }
                if active { span { style: "color:var(--accent);display:flex;", {icons::check(15)} } }
            }
        }
    }
}
