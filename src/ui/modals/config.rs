//! Table Config modal: sources, format, live scan, Hive partitioning.
use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::overlays::ConfigTarget;
use crate::state::{AppState, ConfigForm};
use crate::ui::components::{
    Button, ButtonVariant, Eyebrow, Icon, IconButton, IconButtonVariant, MonoValue, Path,
    Segment, SegmentOption, Select, SelectOption, Spacer, TextInput, Toggle, WinGeom, Window,
};
use crate::ui::icons::{IconName, IconSize};

// ---------------------------------------------------------------------------
// Table config
// ---------------------------------------------------------------------------

/// Set a config source path and, if the table name is still blank, default it
/// from the chosen file/folder's name. When the path is a single file with a
/// recognised extension, the format is auto-detected from it.
fn set_source(mut draft: Signal<ConfigForm>, state: Signal<AppState>, idx: usize, path: String) {
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
    let mut d = draft.write();
    if let Some(slot) = d.sources.get_mut(idx) {
        *slot = path;
    }
    if d.name.trim().is_empty() {
        if let Some(st) = stem {
            d.name = st;
        }
    }
    if let Some(fmt) = fmt {
        d.format = fmt.to_string();
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
fn browse_source(draft: Signal<ConfigForm>, state: Signal<AppState>, idx: usize) {
    use futures::StreamExt;
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    let (tx, mut rx) = futures::channel::mpsc::unbounded::<Option<String>>();
    spawn(async move {
        if let Some(Some(path)) = rx.next().await {
            set_source(draft, state, idx, path);
            rescan(draft, state);
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
fn browse_source(draft: Signal<ConfigForm>, state: Signal<AppState>, idx: usize) {
    spawn(async move {
        if let Some(handle) = rfd::AsyncFileDialog::new().pick_file().await {
            set_source(
                draft,
                state,
                idx,
                handle.path().to_string_lossy().into_owned(),
            );
            rescan(draft, state);
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
fn rescan(mut draft: Signal<ConfigForm>, state: Signal<AppState>) {
    let (paths, format, hive_on, base) = {
        let d = draft.read();
        let s = state.read();
        (
            d.sources.clone(),
            d.format.clone(),
            d.hive_on,
            crate::action::catalog::project_dir(&s),
        )
    };
    let r = crate::action::catalog::scan_sources(&paths, &format, base.as_deref());
    let mut d = draft.write();
    d.scanning = false;
    d.all_dirs = r.all_dirs;
    d.file_count = r.file_count;
    d.scan_error = r.error;
    d.detected_parts = r.partition_keys;
    // Partitioning only makes sense over directories. If the paths are no longer
    // all-dirs, force it off. If it's on, adopt newly-detected keys — but only
    // when the *set* of keys changed, so the user's type picks survive an
    // unrelated rescan.
    if !r.all_dirs {
        d.hive_on = false;
        d.part_cols.clear();
    } else if hive_on {
        let same_keys = d
            .part_cols
            .iter()
            .map(|(k, _)| k.clone())
            .eq(d.detected_parts.iter().map(|(k, _)| k.clone()));
        if !same_keys {
            d.part_cols = d.detected_parts.clone();
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
/// renders the window only when open, handing the modal its target (`New` or
/// `Edit`). Closing / a successful register clears the store (`close_config`).
#[component]
pub fn ConfigHost() -> Element {
    match crate::overlays::OVERLAYS.resolve().read().config.clone() {
        Some(target) => rsx! {
            ConfigModal { target, on_close: move |_| crate::overlays::close_config() }
        },
        None => rsx! {},
    }
}

/// Seed a fresh working draft from the target: blank for `New`, a *copy* of the
/// project table for `Edit`. The project store is never touched.
fn seed_draft(target: &ConfigTarget) -> ConfigForm {
    match target {
        ConfigTarget::New => ConfigForm::default(),
        ConfigTarget::Edit(name) => {
            let store = crate::project::store();
            let p = store.read();
            match p.tables.iter().find(|t| &t.name == name) {
                Some(t) => ConfigForm {
                    editing: Some(t.name.clone()),
                    name: t.name.clone(),
                    format: t.format.clone(),
                    sources: if t.sources.is_empty() {
                        vec![String::new()]
                    } else {
                        t.sources.clone()
                    },
                    hive_on: !t.partition_cols.is_empty(),
                    part_cols: t.partition_cols.clone(),
                    ..ConfigForm::default()
                },
                None => ConfigForm::default(),
            }
        }
    }
}

#[component]
pub fn ConfigModal(target: ConfigTarget, on_close: EventHandler<()>) -> Element {
    let state = use_context::<Signal<AppState>>();
    // The working copy is component-local; the project store stays immutable until
    // a successful register. Seed it from the target once, on mount.
    let mut draft = use_signal(move || seed_draft(&target));
    let d = draft.read();
    let editing = d.editing.is_some();
    let name = d.name.clone();
    let format = d.format.clone();
    let sources = d.sources.clone();
    let hive_on = d.hive_on;
    let part_cols = d.part_cols.clone();
    let all_dirs = d.all_dirs;
    let file_count = d.file_count;
    let scanning = d.scanning;
    let scan_error = d.scan_error.clone();
    drop(d);
    // A failed engine register is surfaced inline via the store (window stays open).
    let reg_err = crate::overlays::OVERLAYS
        .resolve()
        .read()
        .config_err
        .clone();

    // Scan the sources once when the modal opens (validates pre-filled edit paths).
    use_hook(move || rescan(draft, state));

    let title = if editing {
        "Configure table"
    } else {
        "New external table"
    };
    let confirm_label = if editing {
        "Save changes"
    } else {
        "Create table"
    };
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
            icon: IconName::CubeLines, icon_size: IconSize::Md,
            init: WinGeom::new(300.0, 96.0, 620.0, 600.0),
            min_w: 520.0,
            min_h: 420.0,
            footer: rsx! {
                Spacer {}
                Button { variant: ButtonVariant::Secondary, onclick: move |_| on_close.call(()), "Cancel" }
                Button {
                    variant: ButtonVariant::Primary,
                    disabled: !form_ready,
                    icon: IconName::Check, icon_size: IconSize::Sm,
                    onclick: move |_| { if form_ready { dispatch(state, Action::RegisterTable(draft())); } },
                    "{confirm_label}"
                }
            },
            div { class: "modal-body ps-scroll",
                    div { class: "row", style: "gap:var(--sp-4);margin-bottom:var(--sp-5);align-items:flex-end;",
                        div { style: "flex:1;",
                            Eyebrow { class: "field-label", "TABLE NAME" }
                            TextInput { value: "{name}", placeholder: "my_table", mono: true,
                                oninput: move |v| draft.write().name = v }
                        }
                        div {
                            Eyebrow { class: "field-label", "FORMAT" }
                            Select {
                                value: format.clone(),
                                width: 128,
                                options: vec![
                                    SelectOption::new("parquet", "parquet"),
                                    SelectOption::new("csv", "csv"),
                                    SelectOption::new("json", "json"),
                                    SelectOption::new("arrow", "arrow"),
                                ],
                                on_select: move |v: String| { draft.write().format = v; rescan(draft, state); },
                            }
                        }
                    }

                    div { class: "row", style: "justify-content:space-between;margin-bottom:var(--sp-3);",
                        Eyebrow { class: "sec-label", "SOURCE PATHS" }
                        Path { style:"color:var(--faint);", "file · directory · recursive glob" }
                    }
                    div { style: "display:flex;flex-direction:column;gap:var(--sp-3);",
                        for (idx, src) in sources.iter().cloned().enumerate() {
                            div { class: "src-row",
                                TextInput { value: "{src}", mono: true, grow: true,
                                    oninput: move |v| { let mut w = draft.write(); if let Some(p) = w.sources.get_mut(idx) { *p = v; } },
                                    onchange: move |_v| rescan(draft, state) }
                                IconButton { icon: IconName::Folder, variant: IconButtonVariant::Toolbar, title: "Browse — file or folder…",
                                    onclick: move |_| browse_source(draft, state, idx),
                                }
                                // At least one path is required, so the last remaining
                                // row has no remove button.
                                if !single_path {
                                    IconButton { icon: IconName::Minus, icon_size: IconSize::Xs, variant: IconButtonVariant::Danger, title: "Remove path",
                                        onclick: move |_| { { let mut w = draft.write(); if w.sources.len() > 1 { w.sources.remove(idx); } } rescan(draft, state); },
                                    }
                                }
                            }
                        }
                    }
                    Button { variant: ButtonVariant::Ghost, icon: IconName::Plus, icon_size: IconSize::Xs,
                        onclick: move |_| { draft.write().sources.push(String::new()); rescan(draft, state); },
                        "Add path"
                    }

                    // validation status
                    if scanning {
                        div { class: "status-run",
                            Icon { name: IconName::Clock, size: IconSize::Sm }
                            Path { "Scanning source paths…" }
                        }
                    } else if let Some(err) = reg_err.clone() {
                        div { class: "status-err",
                            Icon { name: IconName::Alert, size: IconSize::Sm, color: "var(--red2)" }
                            div {
                                MonoValue { style: "display:block;color:var(--red);", "Registration failed" }
                                Path { style:"display:block;color:#d99;margin-top:var(--sp-1);", "{err}" }
                            }
                        }
                    } else if let Some(err) = scan_error.clone() {
                        div { class: "status-err",
                            Icon { name: IconName::Alert, size: IconSize::Sm, color: "var(--red2)" }
                            div {
                                MonoValue { style: "display:block;color:var(--red);", "Sources don't validate" }
                                Path { style:"display:block;color:#d99;margin-top:var(--sp-1);", "{err}" }
                            }
                        }
                    } else if form_ready {
                        div { class: "status-ok",
                            Icon { name: IconName::Check, size: IconSize::Sm }
                            Path { "{ready_msg}" }
                        }
                    } else {
                        div { class: "status-wait",
                            Icon { name: IconName::Info, size: IconSize::Sm, color: "var(--dim3)" }
                            Path { "{incomplete_msg}" }
                        }
                    }

                    // hive partitioning — only available when every path is a directory
                    div { class: "hive-box",
                        Toggle {
                            on: hive_on,
                            avail: all_dirs && !hive_on,
                            disabled: !all_dirs,
                            sub: "{hive_sub}",
                            on_toggle: move |v| {
                                let mut w = draft.write();
                                if !w.all_dirs { return; }
                                w.hive_on = v;
                                if v {
                                    w.part_cols = w.detected_parts.clone();
                                } else {
                                    w.part_cols.clear();
                                }
                            },
                            "Hive-style partitioning"
                        }
                        if hive_on {
                            if part_cols.is_empty() {
                                Path { style:"display:block;margin-top:var(--sp-4);",
                                    "No key=value partition directories were found under these paths."
                                }
                            } else {
                                div { style: "margin-top:var(--sp-4);display:flex;flex-direction:column;gap:var(--sp-3);",
                                    for (pidx, (pname, ptype)) in part_cols.iter().cloned().enumerate() {
                                        div { class: "row", style: "gap:var(--sp-4);",
                                            span { class: "row", style: "width:90px;flex:none;gap:var(--sp-3);color:var(--accent);", Icon { name: IconName::Branch, size: IconSize::Xs } MonoValue { style: "color:var(--accent);", "{pname}" } }
                                            Segment {
                                                value: ptype.clone(),
                                                compact: true,
                                                on_select: move |v: String| { let mut w = draft.write(); if let Some(pc) = w.part_cols.get_mut(pidx) { pc.1 = v; } },
                                                options: vec![
                                                    SegmentOption::new("Utf8", "Utf8"),
                                                    SegmentOption::new("Int32", "Int32"),
                                                    SegmentOption::new("Int64", "Int64"),
                                                    SegmentOption::new("Date", "Date"),
                                                ],
                                            }
                                        }
                                    }
                                }
                                if part_warn {
                                    div { class: "part-warn",
                                        Icon { name: IconName::Alert, size: IconSize::Xs, color: "var(--orange)" }
                                        Path { "Partition values are inferred as strings — WHERE year = 2024 needs a cast unless you set Int/Date." }
                                    }
                                }
                            }
                        }
                    }
                }

        }
    }
}
