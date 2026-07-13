//! The Settings **window** (W1): a standalone OS window — its own `VirtualDom` —
//! spawned single-instance via `crate::window::spawn_settings_window` (header /
//! launcher gear, ⌘,, File menu).
//!
//! **Draft / save model.** Every non-theme control edits a *local* `draft` copy of
//! the settings; nothing lands in the other windows until **Save**, which commits
//! the draft as `crate::settings`' applied settings and persists to the app config.
//! **Cancel** (and the OS close button) discards the draft.
//!
//! **Theme is the exception** — it previews **live**: picking a theme (or toggling
//! Sync-with-OS) writes the shared *live* theme immediately, so every open window
//! re-themes at once. It's still only written to disk on Save; Cancel/close reverts
//! the preview to the committed theme (see [`crate::settings`]).
//!
//! Categories: Appearance / Data display / System / Keymap (Engine +
//! settings-search + keymap rebinding are W2/W3/W4).

use dioxus::desktop::{use_window, use_wry_event_handler};
use dioxus::prelude::*;

use crate::config::Settings;
use crate::state::SettingsCat;
use crate::ui::components::{
    Body, Button, ButtonVariant, Caption, Eyebrow, Icon, IconButton, IconButtonVariant, Micro,
    Prose, Segment, SegmentOption, Spacer, Strong, TextInput, Toggle,
};
use crate::ui::icons::{IconName, IconSize};

/// Root class: on macOS the transparent titlebar puts the native traffic lights
/// top-left, so the titlebar row gets extra left padding there.
#[cfg(target_os = "macos")]
const SETTINGS_CLASS: &str = "settings-root mac";
#[cfg(not(target_os = "macos"))]
const SETTINGS_CLASS: &str = "settings-root";

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

/// The Settings window root. Holds a local draft of the settings, previews the
/// theme live across all windows, and renders the titlebar + category nav/pane +
/// Cancel/Save footer.
#[component]
pub fn SettingsRoot() -> Element {
    let win = use_window();
    let win_id = win.id();
    // Wire this window into the shared settings context (seed once + reactive theme
    // css), and register it so a repeat open focuses this window instead of
    // spawning a duplicate.
    let theme_css = crate::settings::use_settings();
    use_hook(crate::window::register_settings_window);
    // macOS: dark NSWindow background so a resize doesn't flash white.
    #[cfg(target_os = "macos")]
    use_hook(|| crate::window::paint_ns_background(0.043, 0.055, 0.075));
    // macOS: pin this window above the window that opened it (native child window),
    // so the opener can't cover it and closing the opener closes it too.
    #[cfg(target_os = "macos")]
    use_hook(crate::window::attach_settings_to_owner);

    use_wry_event_handler(move |event, _| {
        use dioxus::desktop::tao::event::{Event as TaoEvent, WindowEvent};
        if let TaoEvent::WindowEvent {
            window_id, event, ..
        } = event
        {
            if *window_id != win_id {
                return;
            }
            match event {
                // Follow the OS light/dark switch live (Sync-with-OS, no restart).
                WindowEvent::ThemeChanged(theme) => {
                    use dioxus::desktop::tao::window::Theme;
                    crate::settings::set_os_dark(*theme == Theme::Dark);
                }
                // Track focus so app-menu commands don't misroute to a background
                // project window while Settings is up front (S11 menu routing).
                WindowEvent::Focused(f) => crate::window::note_focused(win_id, *f),
                _ => {}
            }
        }
    });

    // On close (Cancel button, OS close button, or app quit) discard any live theme
    // preview — a no-op after Save, which already committed the theme. Also release
    // the single-window slot.
    use_drop(|| {
        crate::settings::revert_theme_preview();
        crate::window::unregister_settings_window();
    });

    // The active category is transient UI state, local to this window.
    let mut cat_sig = use_signal(|| SettingsCat::Appearance);
    let cat = cat_sig();

    // Local draft, seeded from the committed settings. Every control edits this;
    // Save commits + persists it, Cancel/close discards it. (Theme edits ALSO
    // preview live across windows — see the theme handlers below.)
    let mut draft = use_signal(crate::settings::snapshot);
    let d = draft.read();
    let theme_id = d.theme.clone();
    let sync_os = d.sync_os;
    let density_compact = d.density_compact;
    let zebra = d.zebra;
    let row_limit = d.row_limit;
    let reopen = d.reopen_on_startup;
    let default_dir = d.default_project_dir.clone();
    let open_pref = d.open_pref;
    let confirm_close = d.confirm_close_running;
    drop(d);
    let os_dark = *crate::settings::OS_DARK.read();

    // The window's own chrome follows the LIVE theme (via `use_settings` above, so a
    // preview shows here too); the theme grid's active card follows the draft.
    let density = if density_compact { "compact" } else { "comfortable" };

    // When Sync-with-OS is on, the effective theme follows the system appearance
    // and the theme grid is disabled.
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

    // Footer actions. Cancel just closes (the drop handler reverts the preview);
    // Save commits + persists the draft, then closes.
    let win_cancel = win.clone();
    let win_save = win.clone();

    rsx! {
        style { dangerous_inner_html: crate::CSS }
        div {
            class: "{SETTINGS_CLASS}",
            style: "{theme_css}",
            "data-density": "{density}",
            // Titlebar (native traffic lights sit to the left of this on macOS).
            // The webview covers the native title bar, so drag the window from the
            // titlebar background — same as the project header. `prevent_default`
            // suppresses drag-selection.
            div { class: "settings-titlebar",
                onmousedown: move |e| { e.prevent_default(); dioxus::desktop::window().drag(); },
                div { class: "settings-tb-badge", Icon { name: IconName::Gear, size: IconSize::Sm } }
                span { class: "settings-tb-title", "Settings" }
            }
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
                                Toggle {
                                    on: sync_os,
                                    on_toggle: move |_| {
                                        let v = !sync_os;
                                        draft.write().sync_os = v;
                                        crate::settings::preview_sync_os(v);
                                    },
                                }
                            }
                            div { class: "settings-divider" }
                            Strong { style: "display:block;margin:var(--sp-5) 0 var(--sp-4);", "Theme" }
                            if sync_os {
                                Caption { style: "display:block;margin-bottom:var(--sp-4);", "Following your system appearance ({os_label}). Turn off Sync with OS to choose a theme." }
                            }
                            div { class: "theme-grid", style: "{grid_style}",
                                for t in crate::theme::registry() {
                                    {theme_card(t, &active_id, draft)}
                                }
                            }
                        },
                        SettingsCat::DataDisplay => rsx! {
                            Strong { style: "display:block;margin-bottom:var(--sp-4);", "Row density" }
                            Segment {
                                value: if density_compact { "compact" } else { "comfortable" },
                                on_select: move |v: String| { draft.write().density_compact = v == "compact"; },
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
                                Toggle {
                                    on: zebra,
                                    on_toggle: move |_| { let v = !zebra; draft.write().zebra = v; },
                                }
                            }
                            div { class: "settings-divider", style: "margin:var(--sp-6) 0;" }
                            Strong { style: "display:block;margin-bottom:var(--sp-1);", "Default row limit" }
                            Caption { style: "display:block;color:var(--dim2);margin-bottom:var(--sp-4);", "New query tabs are generated with this LIMIT so a stray SELECT * can't pull a whole file into memory." }
                            Segment {
                                value: row_limit.to_string(),
                                on_select: move |v: String| { if let Ok(n) = v.parse::<usize>() { draft.write().row_limit = n; } },
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
                                Toggle {
                                    on: reopen,
                                    on_toggle: move |_| { let v = !reopen; draft.write().reopen_on_startup = v; },
                                }
                            }
                            div { class: "settings-divider", style: "margin:var(--sp-6) 0;" }
                            Eyebrow { class: "settings-sublabel", "PROJECTS" }
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
                            div { class: "settings-divider", style: "margin:var(--sp-6) 0;" }
                            Eyebrow { class: "settings-sublabel", "SAFETY" }
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
                        },
                        SettingsCat::Keymap => rsx! {
                            Caption { style: "display:block;color:var(--dim2);margin-bottom:var(--sp-5);", "Read-only. ⌘ shortcuts also respond to Ctrl." }
                            div { class: "keymap-box",
                                for cmd in crate::keymap::ALL_COMMANDS {
                                    {keymap_row(cmd)}
                                }
                            }
                        },
                    }
                }
            }
            // Footer — Cancel discards the draft (drop reverts the live theme
            // preview); Save commits + persists it and applies to every window.
            div { class: "settings-foot",
                Spacer {}
                Button { variant: ButtonVariant::Ghost, onclick: move |_| win_cancel.close(), "Cancel" }
                Button {
                    variant: ButtonVariant::Primary,
                    onclick: move |_| {
                        crate::settings::save_draft(draft.peek().clone());
                        win_save.close();
                    },
                    "Save"
                }
            }
        }
    }
}

/// One read-only row in the Keymap category — a command's label + description and
/// its live, override-aware chord, rendered straight from `crate::keymap` so the
/// list can't drift from the real bindings saved in `Settings::keybinds`.
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

/// One theme preview card in the Appearance grid — a mini mockup rendered in the
/// theme's own colours, plus name / source badge / active check. Clicking it edits
/// the draft's theme **and** previews it live across every window.
fn theme_card(t: &crate::theme::ResolvedTheme, active_id: &str, mut draft: Signal<Settings>) -> Element {
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
