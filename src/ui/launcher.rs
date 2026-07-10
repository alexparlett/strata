//! Welcome / launcher window (B8). A standalone window (its own `VirtualDom`),
//! opened only when "Close project" closes the last project window. Reads recent
//! projects straight from the per-machine config store; "Open" / a recent row
//! spawns a project window and closes the launcher.

use dioxus::prelude::*;

use crate::config::{self, RecentProject};
use crate::ui::components::{
    Button, ButtonVariant, Eyebrow, Icon, IconButton, IconButtonVariant, Path, Prose, SearchBar,
    Spacer, Strong, Title,
};
use crate::ui::icons::{IconName, IconSize};

/// Up-to-two-letter initials for a project avatar (word/`_`/`-` boundaries).
fn initials(name: &str) -> String {
    let parts: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .filter(|s| !s.is_empty())
        .collect();
    let s: String = match parts.as_slice() {
        [] => name.chars().take(2).collect(),
        [one] => one.chars().take(2).collect(),
        [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
    };
    s.to_uppercase()
}

/// A stable avatar colour for a project name (hash → fixed palette).
fn avatar_color(name: &str) -> &'static str {
    const PALETTE: [&str; 6] = [
        "#7ee787", "#ffa657", "#79c0ff", "#d2a8ff", "#f778ba", "#56d4bc",
    ];
    let sum: u32 = name.bytes().map(|b| b as u32).sum();
    PALETTE[(sum as usize) % PALETTE.len()]
}

#[component]
pub fn LauncherRoot() -> Element {
    #[cfg(target_os = "macos")]
    use_hook(|| crate::window::paint_ns_background(0.039, 0.051, 0.071));

    // Recents live in a signal so pin / remove update the list in place; each
    // mutation writes the config store then reloads this signal from it.
    let recents = use_signal(|| crate::config::load().recent_projects);
    // The launcher has no project, so it reads the persisted theme from the
    // machine-global config (honouring Sync-with-OS) and injects it like a
    // project window does.
    let theme_css = use_hook(|| {
        let cfg = crate::config::load();
        let id = crate::theme::effective_id(
            &cfg.settings.theme,
            cfg.settings.sync_os,
            crate::theme::os_is_dark(),
        );
        crate::theme::css_for(&id)
    });

    let mut filter = use_signal(String::new);
    let f = filter.read().to_lowercase();
    let matched: Vec<RecentProject> = recents
        .read()
        .iter()
        .filter(|r| {
            f.is_empty() || r.name.to_lowercase().contains(&f) || r.path.to_lowercase().contains(&f)
        })
        .cloned()
        .collect();
    let none = matched.is_empty();
    let has_pinned = matched.iter().any(|r| r.pinned);
    let has_unpinned = matched.iter().any(|r| !r.pinned);
    let pinned: Vec<RecentProject> = matched.iter().filter(|r| r.pinned).cloned().collect();
    let unpinned: Vec<RecentProject> = matched.into_iter().filter(|r| !r.pinned).collect();

    rsx! {
        style { dangerous_inner_html: crate::CSS }
        div { style: "{theme_css}width:100vw;height:100vh;box-sizing:border-box;background:var(--panel);display:flex;font-family:var(--ui);",
            div { style: "width:100%;height:100%;background:var(--panel);overflow:hidden;display:flex;flex-direction:column;",

                // title bar — drag the window from here (the child webview covers
                // the native title bar). `prevent_default` stops the drag from
                // also starting a text selection.
                div {
                    onmousedown: move |e| { e.prevent_default(); dioxus::desktop::window().drag(); },
                    style: "height:46px;flex:none;display:flex;align-items:center;justify-content:center;border-bottom:1px solid var(--line);",
                    Strong { style: "color:var(--text3);", "Welcome to Strata" }
                }

                div { style: "flex:1;display:flex;min-height:0;",

                    // left rail — branding
                    div { style: "width:258px;flex:none;border-right:1px solid var(--line);padding:var(--sp-6) var(--sp-4);display:flex;flex-direction:column;background:var(--bg);",
                        div { style: "display:flex;align-items:center;gap:var(--sp-4);padding:0 var(--sp-3) var(--sp-6);",
                            div { style: "width:40px;height:40px;border-radius:var(--r-3);overflow:hidden;display:flex;align-items:center;justify-content:center;",
                                Icon { name: IconName::StrataLogo, size: IconSize::Px(40) }
                            }
                            div {
                                Title { "Strata" }
                                Path { style: "display:block;color:var(--dim3);margin-top:var(--sp-1);", {env!("CARGO_PKG_VERSION")} }
                            }
                        }
                        div { style: "display:flex;align-items:center;gap:var(--sp-4);padding:var(--sp-3) var(--sp-4);border-radius:var(--r-2);background:var(--accent-soft);border-left:2px solid var(--accent);color:var(--accent);",
                            Icon { name: IconName::Folder, size: IconSize::Sm }
                            Strong { "Projects" }
                        }
                        Spacer {}
                    }

                    // right pane — search + Open + recents
                    div { style: "flex:1;min-width:0;display:flex;flex-direction:column;",
                        div { style: "display:flex;align-items:center;gap:var(--sp-6);padding:var(--sp-6) var(--sp-6);flex:none;",
                            div { style: "flex:1;max-width:460px;display:flex;",
                                SearchBar {
                                    value: filter(),
                                    oninput: move |v| filter.set(v),
                                    placeholder: "Search projects",
                                    grow: true,
                                }
                            }
                            Spacer {}
                            Button {
                                variant: ButtonVariant::Ghost,
                                icon: IconName::Folder, icon_size: IconSize::Md,
                                // Async picker → new project window, then close the launcher.
                                onclick: move |_| {
                                    spawn(async move {
                                        if let Some(handle) = rfd::AsyncFileDialog::new().pick_folder().await {
                                            let path = crate::window::resolve_project_dir(handle.path());
                                            crate::window::spawn_project_window(path.to_string_lossy().into_owned());
                                            dioxus::desktop::window().close();
                                        }
                                    });
                                },
                                "Open folder…"
                            }
                        }

                        div { class: "ps-scroll", style: "flex:1;overflow-y:auto;padding:0 var(--sp-5) var(--sp-5);",
                            if none {
                                Prose { style: "padding:60px var(--sp-6);text-align:center;color:var(--dim3);",
                                    "No recent projects — click "
                                    Strong { style: "color:var(--accent);", "Open folder…" }
                                    " to choose one."
                                }
                            }
                            if has_pinned {
                                Eyebrow { class: "launch-lbl", "PINNED" }
                                for r in pinned {
                                    {project_row(r, recents)}
                                }
                            }
                            if has_pinned && has_unpinned {
                                Eyebrow { class: "launch-lbl", "RECENT" }
                            }
                            for r in unpinned {
                                {project_row(r, recents)}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One recent-project row: avatar + name (+ pin badge when pinned) + path, plus the
/// hover actions (pin / open-in-new-window / reveal / remove). Each action
/// `stop_propagation`s so it doesn't also fire the row's open-and-close click. Pin
/// and remove write the config store, then reload the `recents` signal from it.
fn project_row(r: RecentProject, mut recents: Signal<Vec<RecentProject>>) -> Element {
    let RecentProject {
        name, path, pinned, ..
    } = r;
    let ini = initials(&name);
    let col = avatar_color(&name);
    let (open_path, new_path, pin_path, rev_path, rm_path) = (
        path.clone(),
        path.clone(),
        path.clone(),
        path.clone(),
        path.clone(),
    );
    rsx! {
        div {
            class: "launch-row",
            onclick: move |_| {
                crate::window::spawn_project_window(open_path.clone());
                dioxus::desktop::window().close();
            },
            span { style: "width:38px;height:38px;flex:none;border-radius:var(--r-2);background:{col};display:flex;align-items:center;justify-content:center;", Title { style: "color:#08111a;", "{ini}" } }
            div { style: "flex:1;min-width:0;",
                div { style: "display:flex;align-items:center;gap:var(--sp-3);",
                    Title { "{name}" }
                    if pinned {
                        span { class: "pin-badge", Icon { name: IconName::Pin, size: IconSize::Xs } }
                    }
                }
                Path { style: "display:block;color:var(--dim2);margin-top:var(--sp-1);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;", "{path}" }
            }
            div { class: "row-actions",
                IconButton {
                    variant: IconButtonVariant::Toggle,
                    on: pinned,
                    icon: IconName::Pin,
                    title: if pinned { "Unpin" } else { "Pin" },
                    onclick: move |e: MouseEvent | {
                        e.stop_propagation();
                        let mut cfg = config::load();
                        cfg.set_pinned(&pin_path, !pinned);
                        config::save(&cfg);
                        recents.set(cfg.recent_projects);
                    },
                }
                IconButton {
                    variant: IconButtonVariant::Ghost,
                    icon: IconName::External,
                    title: "Open in new window",
                    onclick: move |e: MouseEvent| {
                        e.stop_propagation();
                        crate::window::spawn_project_window(new_path.clone());
                    },
                }
                IconButton {
                    variant: IconButtonVariant::Ghost,
                    icon: IconName::Folder,
                    title: "Reveal on disk",
                    onclick: move |e: MouseEvent| {
                        e.stop_propagation();
                        reveal(&rev_path);
                    },
                }
                IconButton {
                    variant: IconButtonVariant::Ghost,
                    icon: IconName::Trash,
                    title: "Remove from list",
                    onclick: move |e: MouseEvent| {
                        e.stop_propagation();
                        let mut cfg = config::load();
                        cfg.remove_recent(&rm_path);
                        config::save(&cfg);
                        recents.set(cfg.recent_projects);
                    },
                }
            }
        }
    }
}

/// Reveal a project's folder in the OS file manager. `path` is the `.strata` dir, so
/// we open its parent (the project folder itself).
fn reveal(path: &str) {
    let dir = std::path::Path::new(path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from(path));
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(&dir).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(&dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
}
