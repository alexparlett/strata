//! Table Config modal: sources, format, live scan, Hive partitioning.
use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, CfgStatus};
use crate::ui::components::{WinGeom, Window};
use crate::ui::icons;

// ---------------------------------------------------------------------------
// Table config
// ---------------------------------------------------------------------------

/// Set a config source path and, if the table name is still blank, default it
/// from the chosen file/folder's name. When the path is a single file with a
/// recognised extension, the format is auto-detected from it.
fn set_source(mut state: Signal<AppState>, idx: usize, path: String) {
    // Store paths inside the project folder relative to it, so the project stays
    // portable; anything outside stays absolute.
    let base = {
        let s = state.read();
        crate::action::catalog::project_dir(&s)
    };
    let path = crate::action::catalog::relativize(base.as_deref(), &path);
    let stem = std::path::Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned());
    let fmt = detect_format(&path);
    let mut s = state.write();
    if let Some(slot) = s.cfg.sources.get_mut(idx) {
        *slot = path;
    }
    if s.cfg.name.trim().is_empty() {
        if let Some(st) = stem {
            s.cfg.name = st;
        }
    }
    if let Some(fmt) = fmt {
        s.cfg.format = fmt.to_string();
    }
}

/// Map a file extension to one of the supported table formats (parquet / csv /
/// json / arrow). Returns `None` for directories, globs, or unknown extensions
/// so the current format selection is left untouched.
fn detect_format(path: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(path)
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase();
    Some(match ext.as_str() {
        "parquet" | "pq" => "parquet",
        "csv" | "tsv" => "csv",
        "json" | "ndjson" | "jsonl" => "json",
        "arrow" | "feather" | "ipc" => "arrow",
        _ => return None,
    })
}

/// Open a native picker for a source path that accepts **either** a file or a
/// directory. rfd's dialog is file-only or folder-only, so on macOS we drive
/// `NSOpenPanel` directly with both `canChooseFiles` and `canChooseDirectories`.
/// The completion handler runs on the main thread (non-blocking `begin…`, so it
/// never re-enters the renderer mid-borrow); it forwards the chosen path over a
/// channel to a Dioxus task, which applies it through a signal write so the UI
/// re-renders.
#[cfg(target_os = "macos")]
fn browse_source(state: Signal<AppState>, idx: usize) {
    use futures::StreamExt;
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    let (tx, mut rx) = futures::channel::mpsc::unbounded::<Option<String>>();
    spawn(async move {
        if let Some(Some(path)) = rx.next().await {
            set_source(state, idx, path);
            rescan(state);
        }
    });

    unsafe {
        let panel: *mut Object = msg_send![class!(NSOpenPanel), openPanel];
        let _: () = msg_send![panel, setCanChooseFiles: true];
        let _: () = msg_send![panel, setCanChooseDirectories: true];
        let _: () = msg_send![panel, setAllowsMultipleSelection: false];
        let handler = block::ConcreteBlock::new(move |resp: i64| {
            // NSModalResponseOK == 1
            let path = if resp == 1 {
                unsafe {
                    let url: *mut Object = msg_send![panel, URL];
                    if url.is_null() {
                        None
                    } else {
                        let ns: *mut Object = msg_send![url, path];
                        let c: *const std::os::raw::c_char = msg_send![ns, UTF8String];
                        if c.is_null() {
                            None
                        } else {
                            Some(std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned())
                        }
                    }
                }
            } else {
                None
            };
            let _ = tx.unbounded_send(path);
        });
        let handler = handler.copy();
        let _: () = msg_send![panel, beginWithCompletionHandler: &*handler];
    }
}

/// Non-macOS fallback: rfd can't offer a combined file/folder dialog, so pick a
/// file (directory paths and globs can still be typed).
#[cfg(not(target_os = "macos"))]
fn browse_source(state: Signal<AppState>, idx: usize) {
    spawn(async move {
        if let Some(handle) = rfd::AsyncFileDialog::new().pick_file().await {
            set_source(state, idx, handle.path().to_string_lossy().into_owned());
            rescan(state);
        }
    });
}

/// Scan the current source paths off the UI thread and fold the result into the
/// config modal's state. Awaitable so callers (event handlers and the picker
/// task) can run it without nesting `spawn`s. The heavy filesystem walk runs on
/// a plain OS thread; the result comes back over a channel and is applied via a
/// signal write so the modal re-renders.
/// Scan the current source paths and fold the result into the modal state:
/// file/format validity, whether every path is a directory (→ partitioning), and
/// detected Hive keys. The walk is bounded (20k files) so it runs synchronously
/// — `all_dirs`/errors update in the same render turn as the edit, so the UI
/// (e.g. the partition toggle brightening once it's available) reacts at once.
fn rescan(mut state: Signal<AppState>) {
    let (paths, format, hive_on, base) = {
        let s = state.read();
        (
            s.cfg.sources.clone(),
            s.cfg.format.clone(),
            s.cfg.hive_on,
            crate::action::catalog::project_dir(&s),
        )
    };
    let r = crate::action::catalog::scan_sources(&paths, &format, base.as_deref());
    let mut s = state.write();
    s.cfg.scanning = false;
    s.cfg.all_dirs = r.all_dirs;
    s.cfg.file_count = r.file_count;
    s.cfg.scan_error = r.error;
    s.cfg.detected_parts = r.partition_keys;
    // Partitioning only makes sense over directories. If the paths are no longer
    // all-dirs, force it off. If it's on, adopt newly-detected keys — but only
    // when the *set* of keys changed, so the user's type picks survive an
    // unrelated rescan.
    if !r.all_dirs {
        s.cfg.hive_on = false;
        s.cfg.part_cols.clear();
    } else if hive_on {
        let same_keys = s
            .cfg
            .part_cols
            .iter()
            .map(|(k, _)| k.clone())
            .eq(s.cfg.detected_parts.iter().map(|(k, _)| k.clone()));
        if !same_keys {
            s.cfg.part_cols = s.cfg.detected_parts.clone();
        }
    }
}

/// A valid table identifier: starts with a letter or `_`, then letters, digits
/// or `_`.
fn valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Always-mounted host for the table-config window. Reads the overlay store and
/// renders the window only when open. Triggers dispatch `OpenConfigNew` /
/// `OpenConfigEdit` (which set up `AppState.cfg`, then open the store); the
/// `Registered` event closes it via `overlays::close_config`.
#[component]
pub fn ConfigHost() -> Element {
    if !crate::overlays::OVERLAYS.read().config {
        return rsx! {};
    }
    rsx! {
        ConfigModal { on_close: move |_| crate::overlays::close_config() }
    }
}

#[component]
pub fn ConfigModal(on_close: EventHandler<()>) -> Element {
    let mut state = use_context::<Signal<AppState>>();
    let s = state.read();
    let editing = s.cfg.editing.is_some();
    let name = s.cfg.name.clone();
    let format = s.cfg.format.clone();
    let fmt_open = s.cfg.fmt_open;
    let sources = s.cfg.sources.clone();
    let hive_on = s.cfg.hive_on;
    let part_cols = s.cfg.part_cols.clone();
    let status = s.cfg.status;
    let error = s.cfg.error.clone();
    let all_dirs = s.cfg.all_dirs;
    let file_count = s.cfg.file_count;
    let scanning = s.cfg.scanning;
    let scan_error = s.cfg.scan_error.clone();
    drop(s);

    // Scan the sources once when the modal opens (validates pre-filled edit paths).
    use_hook(|| rescan(state));

    let title = if editing { "Configure table" } else { "New external table" };
    let confirm_label = if editing { "Save changes" } else { "Create table" };
    let part_warn = part_cols.iter().any(|(_, t)| t == "Utf8");
    let single_path = sources.len() <= 1;

    // Live form validity. The name must be a valid identifier, at least one real
    // path must be present (placeholder text doesn't count), and the scan must
    // not have flagged a problem. Drives the status line and the Create button.
    let has_name = !name.trim().is_empty();
    let name_ok = valid_ident(name.trim());
    let has_path = sources.iter().any(|p| !p.trim().is_empty());
    let form_ready = name_ok && has_path && scan_error.is_none() && !scanning;
    let incomplete_msg = if !has_name {
        "Enter a table name."
    } else if !name_ok {
        "Table name must start with a letter or _, then letters, digits or _."
    } else if !has_path {
        "Add at least one source path."
    } else {
        "Choose the sources for this table."
    };
    let ready_msg = if file_count > 0 {
        format!(
            "Ready — {} file{} matched.",
            file_count,
            if file_count == 1 { "" } else { "s" }
        )
    } else {
        "Ready — one table over your selected paths.".to_string()
    };
    let hive_sub = if !all_dirs {
        "available only when every source path is a directory"
    } else if hive_on {
        "detected from key=value directories · confirm the types below"
    } else {
        "scan the directories for key=value partitions"
    };

    rsx! {
        Window {
            on_close: move |_| on_close.call(()),
            title: title.to_string(),
            subtitle: "one table over any mix of files, directories & globs".to_string(),
            icon: icons::cube_lines(16),
            init: WinGeom::new(300.0, 96.0, 620.0, 600.0),
            min_w: 520.0,
            min_h: 420.0,
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", style: "height:34px;", onclick: move |_| on_close.call(()), "Cancel" }
                button {
                    class: "btn accent",
                    style: "height:34px;",
                    disabled: !form_ready,
                    onclick: move |_| { if form_ready { dispatch(state, Action::ConfirmConfig); } },
                    {icons::check(14)} "{confirm_label}"
                }
            },
            div { class: "modal-body ps-scroll",
                    div { class: "row", style: "gap:14px;margin-bottom:18px;align-items:flex-end;",
                        div { style: "flex:1;",
                            div { class: "field-label", "TABLE NAME" }
                            input { class: "text-input", value: "{name}", placeholder: "my_table",
                                oninput: move |e| state.write().cfg.name = e.value() }
                        }
                        div { style: "position:relative;",
                            div { class: "field-label", "FORMAT" }
                            button { class: "btn", style: "width:128px;height:34px;justify-content:space-between;",
                                onclick: move |_| { let mut w = state.write(); w.cfg.fmt_open = !w.cfg.fmt_open; },
                                "{format}" {icons::chevron_down(12)}
                            }
                            if fmt_open {
                                div { class: "menu", style: "position:absolute;top:60px;left:0;width:128px;z-index:5;",
                                    for f in ["parquet", "csv", "json", "arrow"] {
                                        button { class: "menu-item mono", style: "font-size:11.5px;",
                                            onclick: move |_| { { let mut w = state.write(); w.cfg.format = f.to_string(); w.cfg.fmt_open = false; } rescan(state); },
                                            "{f}"
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "row", style: "justify-content:space-between;margin-bottom:8px;",
                        span { class: "sec-label", "SOURCE PATHS" }
                        span { class: "mono", style: "font-size:10px;color:var(--faint);", "file · directory · recursive glob" }
                    }
                    div { style: "display:flex;flex-direction:column;gap:7px;",
                        for (idx, src) in sources.iter().cloned().enumerate() {
                            div { class: "src-row",
                                input { class: "src-input", value: "{src}", placeholder: "/data/2024/  ·  /archive/**/*.parquet",
                                    oninput: move |e| { let mut w = state.write(); if let Some(p) = w.cfg.sources.get_mut(idx) { *p = e.value(); } },
                                    onchange: move |_| rescan(state) }
                                span { class: "src-count", "" }
                                button { class: "mini-btn", style: "width:30px;height:32px;", title: "Browse — file or folder…",
                                    onclick: move |_| browse_source(state, idx),
                                    {icons::folder(15)}
                                }
                                // At least one path is required, so the last remaining
                                // row has no remove button.
                                if !single_path {
                                    button { class: "mini-btn danger", style: "width:28px;height:32px;", title: "Remove path",
                                        onclick: move |_| { { let mut w = state.write(); if w.cfg.sources.len() > 1 { w.cfg.sources.remove(idx); } } rescan(state); },
                                        {icons::minus(12)}
                                    }
                                }
                            }
                        }
                    }
                    button { class: "add-path", onclick: move |_| { state.write().cfg.sources.push(String::new()); rescan(state); },
                        {icons::plus(12)} "Add path"
                    }

                    // validation status
                    match status {
                        CfgStatus::Idle => if scanning { rsx! {
                            div { class: "status-run",
                                span { style: "display:flex;", {icons::clock(15)} }
                                span { "Scanning source paths…" }
                            }
                        } } else if let Some(err) = scan_error.clone() { rsx! {
                            div { class: "status-err",
                                span { style: "flex:none;color:var(--red2);", {icons::alert(15)} }
                                div {
                                    div { class: "mono", style: "font-weight:600;color:var(--red);", "Sources don't validate" }
                                    div { class: "mono", style: "font-size:11px;color:#d99;margin-top:2px;", "{err}" }
                                }
                            }
                        } } else if form_ready { rsx! {
                            div { class: "status-ok",
                                {icons::check(15)}
                                span { "{ready_msg}" }
                            }
                        } } else { rsx! {
                            div { class: "status-wait",
                                span { style: "flex:none;color:var(--dim3);", {icons::info(15)} }
                                span { "{incomplete_msg}" }
                            }
                        } },
                        CfgStatus::Validating => rsx! {
                            div { class: "status-run",
                                span { style: "display:flex;", {icons::clock(15)} }
                                span { "Reading files, inferring & validating schema…" }
                            }
                        },
                        CfgStatus::Error => rsx! {
                            div { class: "status-err",
                                span { style: "flex:none;color:var(--red2);", {icons::alert(15)} }
                                div {
                                    div { class: "mono", style: "font-weight:600;color:var(--red);", "Registration failed" }
                                    div { class: "mono", style: "font-size:11px;color:#d99;margin-top:2px;", "{error}" }
                                }
                            }
                        },
                    }

                    // hive partitioning — only available when every path is a directory
                    div { class: "hive-box",
                        div {
                            class: "row",
                            style: if all_dirs { "gap:11px;cursor:pointer;" } else { "gap:11px;opacity:.5;cursor:not-allowed;" },
                            onclick: move |_| {
                                let mut w = state.write();
                                if !w.cfg.all_dirs { return; }
                                w.cfg.hive_on = !w.cfg.hive_on;
                                if w.cfg.hive_on {
                                    w.cfg.part_cols = w.cfg.detected_parts.clone();
                                } else {
                                    w.cfg.part_cols.clear();
                                }
                            },
                            div { class: if hive_on { "toggle on" } else if all_dirs { "toggle avail" } else { "toggle" }, div { class: "knob" } }
                            div { style: "flex:1;",
                                div { style: if all_dirs { "font-size:12px;color:var(--text);" } else { "font-size:12px;color:var(--text2);" }, "Hive-style partitioning" }
                                div { class: "mono", style: "font-size:10px;color:var(--dim3);margin-top:1px;", "{hive_sub}" }
                            }
                        }
                        if hive_on {
                            if part_cols.is_empty() {
                                div { class: "mono", style: "margin-top:12px;font-size:11px;color:var(--dim2);",
                                    "No key=value partition directories were found under these paths."
                                }
                            } else {
                                div { style: "margin-top:12px;display:flex;flex-direction:column;gap:7px;",
                                    for (pidx, (pname, ptype)) in part_cols.iter().cloned().enumerate() {
                                        div { class: "row", style: "gap:10px;",
                                            span { class: "row mono", style: "width:90px;flex:none;gap:6px;color:var(--accent);font-size:12px;", {icons::branch(11)} "{pname}" }
                                            div { class: "row", style: "gap:3px;padding:3px;background:var(--elev);border:1px solid var(--line2);border-radius:7px;",
                                                for ty in ["Utf8", "Int32", "Int64", "Date"] {
                                                    button {
                                                        class: if ptype == ty { "part-type on" } else { "part-type" },
                                                        onclick: move |_| { let mut w = state.write(); if let Some(pc) = w.cfg.part_cols.get_mut(pidx) { pc.1 = ty.to_string(); } },
                                                        "{ty}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                if part_warn {
                                    div { class: "part-warn",
                                        span { style: "flex:none;color:var(--orange);", {icons::alert(12)} }
                                        span { "Partition values are inferred as strings — WHERE year = 2024 needs a cast unless you set Int/Date." }
                                    }
                                }
                            }
                        }
                    }
                }

        }
    }
}

