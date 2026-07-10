//! Settings modal: Appearance / Data display / System / Keymap.
use dioxus::prelude::*;

use crate::state::SettingsCat;
use crate::ui::components::{
    Body, Button, ButtonVariant, Caption, Eyebrow, Icon, IconButton, IconButtonVariant, Micro,
    Prose, Segment, SegmentOption, Spacer, Strong, TextInput, Toggle, WinGeom, Window,
};
use crate::ui::icons::{IconName, IconSize};

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Global shortcuts shown read-only in the Keymap category: (label, desc, keys).
const KEYMAP: &[(&str, &str, &[&str])] = &[
    (
        "Command palette",
        "Search tables, columns & commands",
        &["⌘", "K"],
    ),
    ("New query tab", "Open a fresh SQL tab", &["⌘", "T"]),
    (
        "Reopen closed tab",
        "Restore the last tab you closed",
        &["⇧", "⌘", "T"],
    ),
    ("Close tab", "Close the active query tab", &["⌘", "W"]),
    (
        "Save query",
        "Save the active query to the project",
        &["⌘", "S"],
    ),
    ("Run query", "Execute the current SQL", &["⌘", "↵"]),
    ("Settings", "Open this panel", &["⌘", ","]),
    (
        "Cycle windows",
        "Focus the next project window",
        &["⌘", "`"],
    ),
    ("Dismiss", "Close overlays & menus", &["Esc"]),
];

/// Left-nav category icon.
fn settings_cat_icon(name: &str) -> Element {
    match name {
        "palette" => IconName::Palette.el(IconSize::Sm),
        "grid" => IconName::Grid.el(IconSize::Sm),
        "sliders" => IconName::Sliders.el(IconSize::Sm),
        "keyboard" => IconName::Keyboard.el(IconSize::Sm),
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
    if !crate::overlays::OVERLAYS.resolve().read().settings {
        return rsx! {};
    }
    rsx! {
        SettingsModal { on_close: move |_| crate::overlays::set_settings(false) }
    }
}

#[component]
pub fn SettingsModal(on_close: EventHandler<()>) -> Element {
    // The active category is transient UI state, local to this window.
    let mut cat_sig = use_signal(|| SettingsCat::Appearance);
    let cat = cat_sig();
    // Prefs come from the per-window settings store (read reactively); OS
    // appearance from its runtime signal. The mutators below call
    // `crate::settings::*`, which write the store *and* persist to the app config.
    let store = crate::settings::SETTINGS.resolve();
    let s = store.read();
    let theme_id = s.theme.clone();
    let sync_os = s.sync_os;
    let density_compact = s.density_compact;
    let zebra = s.zebra;
    let row_limit = s.row_limit;
    let reopen = s.reopen_on_startup;
    let default_dir = s.default_project_dir.clone();
    let open_pref = s.open_pref.clone();
    let confirm_close = s.confirm_close_running;
    drop(s);
    let os_dark = *crate::settings::OS_DARK.read();

    // When Sync-with-OS is on, the effective theme follows the system appearance
    // and the grid is disabled.
    let active_id = crate::theme::effective_id(&theme_id, sync_os, os_dark);
    let crumb = match cat {
        SettingsCat::Appearance => "Appearance",
        SettingsCat::DataDisplay => "Data display",
        SettingsCat::System => "System",
        SettingsCat::Keymap => "Keymap",
    };
    let grid_style = if sync_os {
        "opacity:.45;pointer-events:none;"
    } else {
        ""
    };
    let os_label = if os_dark { "dark" } else { "light" };

    rsx! {
        Window {
            on_close: move |_| on_close.call(()),
            title: "Settings".to_string(),
            subtitle: "appearance & behavior".to_string(),
            icon: IconName::Gear, icon_size: IconSize::Md,
            init: WinGeom::new(260.0, 90.0, 760.0, 600.0),
            min_w: 640.0,
            min_h: 440.0,
            footer: rsx! {
                Spacer {}
                Button { variant: ButtonVariant::Primary, onclick: move |_| on_close.call(()), "Done" }
            },
            div { class: "settings-body",
                    div { class: "settings-nav",
                        Eyebrow { class: "settings-navlabel", "SETTINGS" }
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
                                Body { "{label}" }
                            }
                        }
                    }
                    div { class: "settings-pane ps-scroll",
                        Prose { class: "settings-crumb",
                            "Settings " span { style: "color:var(--faint2);", "›" } " "
                            span { style: "color:var(--text3);", "{crumb}" }
                        }
                        match cat {
                            SettingsCat::Appearance => rsx! {
                                div { class: "settings-row", style: "margin-bottom:var(--sp-6);",
                                    div { style: "flex:1;",
                                        Strong { style: "display:block;", "Sync with OS" }
                                        Caption { style: "display:block;color:var(--dim2);margin-top:var(--sp-1);", "Match your system light/dark appearance automatically." }
                                    }
                                    Toggle { on: sync_os, on_toggle: move |_| crate::settings::toggle_sync_os() }
                                }
                                div { class: "settings-divider" }
                                Strong { style: "display:block;margin:var(--sp-5) 0 var(--sp-4);", "Theme" }
                                if sync_os {
                                    Caption { style: "display:block;margin-bottom:var(--sp-4);", "Following your system appearance ({os_label}). Turn off Sync with OS to choose a theme." }
                                }
                                div { class: "theme-grid", style: "{grid_style}",
                                    for t in crate::theme::registry() {
                                        {theme_card(t, &active_id)}
                                    }
                                }
                            },
                            SettingsCat::DataDisplay => rsx! {
                                Strong { style: "display:block;margin-bottom:var(--sp-4);", "Row density" }
                                Segment {
                                    value: if density_compact { "compact" } else { "comfortable" },
                                    on_select: move |v: String| crate::settings::set_density(v == "compact"),
                                    options: vec![
                                        SegmentOption::new("comfortable", "Comfortable"),
                                        SegmentOption::new("compact", "Compact"),
                                    ],
                                }
                                Caption { style: "display:block;color:var(--dim2);margin-top:var(--sp-4);", "Controls row height in the results grid and catalog." }
                                div { class: "settings-divider", style: "margin:var(--sp-6) 0;" }
                                div { class: "settings-row",
                                    div { style: "flex:1;",
                                        Strong { style: "display:block;", "Alternating row colours" }
                                        Caption { style: "display:block;color:var(--dim2);margin-top:var(--sp-1);", "Shade every other row in the results grid for easier scanning." }
                                    }
                                    Toggle { on: zebra, on_toggle: move |_| crate::settings::toggle_zebra() }
                                }
                                div { class: "settings-divider", style: "margin:var(--sp-6) 0;" }
                                Strong { style: "display:block;margin-bottom:var(--sp-1);", "Default row limit" }
                                Caption { style: "display:block;color:var(--dim2);margin-bottom:var(--sp-4);", "New query tabs are generated with this LIMIT so a stray SELECT * can't pull a whole file into memory." }
                                Segment {
                                    value: row_limit.to_string(),
                                    on_select: move |v: String| { if let Ok(n) = v.parse::<usize>() { crate::settings::set_row_limit(n); } },
                                    options: vec![
                                        SegmentOption::new("100", "100"),
                                        SegmentOption::new("1000", "1,000"),
                                        SegmentOption::new("10000", "10,000"),
                                        SegmentOption::new("0", "No limit"),
                                    ],
                                }
                            },
                            SettingsCat::System => rsx! {
                                Eyebrow { class: "settings-sublabel", "STARTUP" }
                                div { class: "settings-row",
                                    div { style: "flex:1;",
                                        Strong { style: "display:block;", "Reopen projects on startup" }
                                        Caption { style: "display:block;color:var(--dim2);margin-top:var(--sp-1);", "Reopen the projects you had open when you last quit." }
                                    }
                                    Toggle { on: reopen, on_toggle: move |_| crate::settings::toggle_reopen_startup() }
                                }
                                div { class: "settings-divider", style: "margin:var(--sp-6) 0;" }
                                Eyebrow { class: "settings-sublabel", "PROJECTS" }
                                Strong { style: "display:block;margin-bottom:var(--sp-1);", "Default project directory" }
                                Caption { style: "display:block;color:var(--dim2);margin-bottom:var(--sp-4);", "Preselected in the Open dialog. Leave blank to use your last location." }
                                div { class: "row", style: "gap:var(--sp-3);margin-bottom:var(--sp-6);",
                                    TextInput { value: "{default_dir}", mono: true, grow: true, placeholder: "~/data",
                                        // Commit on blur only (onchange), not per-keystroke — avoids
                                        // persisting a half-typed path. oninput is a no-op; the
                                        // browser shows live typing until blur.
                                        oninput: move |_| {},
                                        onchange: move |v| crate::settings::set_default_project_dir(v) }
                                    IconButton { icon: IconName::Folder, variant: IconButtonVariant::Toolbar, title: "Choose…",
                                        onclick: move |_| { spawn(async move {
                                            if let Some(h) = rfd::AsyncFileDialog::new().pick_folder().await {
                                                let p = h.path().to_string_lossy().into_owned();
                                                crate::settings::set_default_project_dir(p);
                                            }
                                        }); },
                                    }
                                }
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
                                        crate::settings::set_open_pref(p);
                                    },
                                    options: vec![
                                        SegmentOption::new("ask", "Ask every time"),
                                        SegmentOption::new("this", "This window"),
                                        SegmentOption::new("new", "New window"),
                                    ],
                                }
                                div { class: "settings-divider", style: "margin:var(--sp-6) 0;" }
                                Eyebrow { class: "settings-sublabel", "SAFETY" }
                                div { class: "settings-row",
                                    div { style: "flex:1;",
                                        Strong { style: "display:block;", "Confirm before closing a tab or window with a running query" }
                                        Caption { style: "display:block;color:var(--dim2);margin-top:var(--sp-1);", "Asks only when a scan is in flight — silent otherwise." }
                                    }
                                    Toggle { on: confirm_close, on_toggle: move |_| crate::settings::toggle_confirm_close() }
                                }
                            },
                            SettingsCat::Keymap => rsx! {
                                Caption { style: "display:block;color:var(--dim2);margin-bottom:var(--sp-5);", "Read-only. ⌘ shortcuts also respond to Ctrl." }
                                div { class: "keymap-box",
                                    for (label, desc, keys) in KEYMAP {
                                        div { class: "keymap-row",
                                            div { style: "flex:1;min-width:0;",
                                                Body { style: "display:block;", "{label}" }
                                                Caption { style: "display:block;color:var(--dim2);margin-top:var(--sp-1);", "{desc}" }
                                            }
                                            div { class: "row", style: "gap:var(--sp-2);flex:none;",
                                                for cap in keys.iter() {
                                                    Eyebrow { class: "keycap", "{cap}" }
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
fn theme_card(t: &crate::theme::ResolvedTheme, active_id: &str) -> Element {
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
            onclick: move |_| crate::settings::set_theme(id.clone()),
            // mini mockup
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
