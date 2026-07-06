//! Welcome / launcher window (B8). A standalone window (its own `VirtualDom`),
//! opened only when "Close project" closes the last project window. Reads recent
//! projects straight from the per-machine config store; "Open" / a recent row
//! spawns a project window and closes the launcher.

use dioxus::prelude::*;

use crate::ui::icons;

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

    // Recents snapshot for this window (loaded once from the config store).
    let recents: Vec<(String, String)> = use_hook(|| {
        crate::config::load()
            .recent_projects
            .into_iter()
            .map(|r| (r.name, r.path))
            .collect()
    });
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
    let rows: Vec<(String, String)> = recents
        .iter()
        .filter(|(n, p)| f.is_empty() || n.to_lowercase().contains(&f) || p.to_lowercase().contains(&f))
        .cloned()
        .collect();

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
                    span { style: "font:600 13px var(--ui);color:var(--text3);", "Welcome to Strata" }
                }

                div { style: "flex:1;display:flex;min-height:0;",

                    // left rail — branding
                    div { style: "width:258px;flex:none;border-right:1px solid var(--line);padding:20px 14px;display:flex;flex-direction:column;background:var(--bg);",
                        div { style: "display:flex;align-items:center;gap:11px;padding:0 6px 22px;",
                            div { style: "width:40px;height:40px;border-radius:11px;overflow:hidden;display:flex;align-items:center;justify-content:center;",
                                {icons::strata_logo(40)}
                            }
                            div {
                                div { style: "font:700 15px var(--ui);color:var(--text);", "Strata" }
                                div { style: "font:400 11px var(--mono);color:var(--dim3);margin-top:1px;", {env!("CARGO_PKG_VERSION")} }
                            }
                        }
                        div { style: "display:flex;align-items:center;gap:10px;padding:9px 12px;border-radius:8px;background:var(--accent-soft);border-left:2px solid var(--accent);color:var(--accent);",
                            {icons::folder(15)}
                            span { style: "font:600 12.5px var(--ui);color:var(--text);", "Projects" }
                        }
                        div { style: "flex:1;" }
                    }

                    // right pane — search + Open + recents
                    div { style: "flex:1;min-width:0;display:flex;flex-direction:column;",
                        div { style: "display:flex;align-items:center;gap:22px;padding:20px 26px;flex:none;",
                            div { style: "flex:1;max-width:460px;display:flex;align-items:center;gap:10px;height:40px;padding:0 15px;background:var(--bg);border:1px solid var(--accent);border-radius:20px;color:var(--dim2);",
                                {icons::search(15)}
                                input {
                                    placeholder: "Search projects",
                                    value: "{filter}",
                                    oninput: move |e| filter.set(e.value()),
                                    style: "flex:1;min-width:0;background:transparent;border:none;outline:none;color:var(--text);font-family:inherit;font-size:13px;",
                                }
                            }
                            div { style: "flex:1;" }
                            button {
                                class: "launch-open",
                                // Async picker → new project window, then close the launcher.
                                onclick: move |_| {
                                    spawn(async move {
                                        if let Some(handle) = rfd::AsyncFileDialog::new().pick_folder().await {
                                            let path = crate::window::resolve_project_file(handle.path());
                                            crate::window::spawn_project_window(path.to_string_lossy().into_owned());
                                            dioxus::desktop::window().close();
                                        }
                                    });
                                },
                                {icons::folder(16)}
                                "Open folder…"
                            }
                        }

                        div { class: "ps-scroll", style: "flex:1;overflow-y:auto;padding:0 16px 16px;",
                            if rows.is_empty() {
                                div { style: "padding:60px 20px;text-align:center;color:var(--dim3);font-size:13px;",
                                    "No recent projects — click "
                                    span { style: "color:var(--accent);font-weight:600;", "Open folder…" }
                                    " to choose one."
                                }
                            }
                            for (name, path) in rows {
                                {
                                    let ini = initials(&name);
                                    let col = avatar_color(&name);
                                    let open_path = path.clone();
                                    rsx! {
                                        div {
                                            class: "launch-row",
                                            onclick: move |_| {
                                                crate::window::spawn_project_window(open_path.clone());
                                                dioxus::desktop::window().close();
                                            },
                                            span { style: "width:38px;height:38px;flex:none;border-radius:9px;background:{col};display:flex;align-items:center;justify-content:center;font:700 14px var(--ui);color:#08111a;", "{ini}" }
                                            div { style: "flex:1;min-width:0;",
                                                div { style: "font:500 14px var(--ui);color:var(--text);", "{name}" }
                                                div { style: "font:400 12px var(--mono);color:var(--dim2);margin-top:2px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;", "{path}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
