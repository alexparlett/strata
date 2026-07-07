//! Export modal: format cards, options, live preview + size estimate.
use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, ExportForm};
use crate::ui::components::{WinGeom, Window};
use crate::ui::icons;

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// Always-mounted host for the Export window. Reads the overlay store and renders
/// the window only when open. Triggers (results toolbar, palette) call
/// `overlays::open_export`; `run_export` closes it via `overlays::close_export`.
#[component]
pub fn ExportHost() -> Element {
    if !crate::overlays::OVERLAYS.resolve().read().export {
        return rsx! {};
    }
    rsx! {
        ExportModal { on_close: move |_| crate::overlays::close_export() }
    }
}

#[component]
pub fn ExportModal(on_close: EventHandler<()>) -> Element {
    let state = use_context::<Signal<AppState>>();
    // Form state is component-local — reset each time the window opens. The
    // `RunExport` action carries a snapshot, so `AppState` never holds it.
    let mut export = use_signal(ExportForm::default);
    let ex = export();
    let (total, cols) = {
        let id = crate::session::active_id();
        let runs = crate::runs::RUNS.read();
        let run_res = runs
            .get(&id)
            .and_then(|r| r.result.as_ref());
        let total = run_res.map(|r| r.total).unwrap_or(0);
        let cols: Vec<String> = run_res
            .map(|r| r.columns.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();
        (total, cols)
    };
    let (preview, size_est) = export_preview(state, &ex);
    let fmt = ex.format.clone();
    let is_clip = fmt == "clipboard";
    let ext = match fmt.as_str() {
        "csv" => ".csv",
        "json" => ".json",
        "parquet" => ".parquet",
        "arrow" => ".arrow",
        _ => "",
    };

    rsx! {
        Window {
            on_close: move |_| on_close.call(()),
            title: "Export results".to_string(),
            subtitle: format!("{total} rows · via COPY … TO"),
            icon: icons::download(16),
            init: WinGeom::new(200.0, 84.0, 820.0, 640.0),
            min_w: 640.0,
            min_h: 420.0,
            footer: rsx! {
                div { class: "spacer" }
                button { class: "btn", style: "height:34px;", onclick: move |_| on_close.call(()), "Cancel" }
                button { class: "btn accent", style: "height:34px;",
                    onclick: move |_| dispatch(state, Action::RunExport(export())),
                    {icons::download(14)}
                    if is_clip { "Copy" } else { "Export" }
                }
            },
            div { class: "modal-body ps-scroll",
                    div { class: "sec-label", style: "margin-bottom:10px;", "FORMAT" }
                    div { class: "fmt-grid",
                        for (id, label, desc) in [("csv", "CSV", "delimited text"), ("json", "JSON", "ndjson records"), ("parquet", "Parquet", "columnar"), ("arrow", "Arrow", "IPC"), ("clipboard", "Clipboard", "copy as text")] {
                            button {
                                class: if fmt == id { "fmt-card on" } else { "fmt-card" },
                                onclick: move |_| { export.write().format = id.to_string(); },
                                {icons::download(15)}
                                span { class: "fn", "{label}" }
                                span { class: "fd", "{desc}" }
                            }
                        }
                    }

                    // ROWS TO EXPORT
                    div { class: "field-label", style: "margin-top:16px;", "ROWS TO EXPORT" }
                    div { class: "row", style: "gap:6px;",
                        for (val, lbl) in [("all", format!("All ({total})")), ("page", "This page".to_string())] {
                            button { class: if ex.scope == val { "seg on" } else { "seg" },
                                onclick: move |_| { export.write().scope = val.to_string(); }, "{lbl}" }
                        }
                    }

                    // OPTIONS (format-swapped)
                    div { class: "field-label", style: "margin-top:16px;", "OPTIONS" }
                    {
                        match fmt.as_str() {
                            "csv" => rsx! {
                                div { class: "row", style: "gap:11px;cursor:pointer;padding:2px 0 8px;",
                                    onclick: move |_| { let mut w = export.write(); w.csv_header = !w.csv_header; },
                                    div { class: if ex.csv_header { "toggle on" } else { "toggle" }, div { class: "knob" } }
                                    span { style: "font-size:12px;color:var(--text3);", "Include header row" }
                                }
                                div { class: "row", style: "gap:8px;margin-bottom:8px;align-items:center;",
                                    span { class: "opt-lbl", "Delimiter" }
                                    for (v, l) in [("comma", "Comma"), ("tab", "Tab"), ("semicolon", "Semicolon"), ("pipe", "Pipe")] {
                                        button { class: if ex.csv_delim == v { "seg on" } else { "seg" }, onclick: move |_| { export.write().csv_delim = v.to_string(); }, "{l}" }
                                    }
                                }
                                div { class: "row", style: "gap:8px;align-items:center;",
                                    span { class: "opt-lbl", "Null as" }
                                    for (v, l) in [("empty", "(empty)"), ("null", "NULL"), ("nan", "NaN")] {
                                        button { class: if ex.csv_null == v { "seg on" } else { "seg" }, onclick: move |_| { export.write().csv_null = v.to_string(); }, "{l}" }
                                    }
                                }
                            },
                            "parquet" => rsx! {
                                div { class: "row", style: "gap:8px;align-items:center;flex-wrap:wrap;",
                                    span { class: "opt-lbl", "Compression" }
                                    for (v, l) in [("zstd", "zstd"), ("snappy", "snappy"), ("gzip", "gzip"), ("brotli", "brotli"), ("lz4", "lz4"), ("none", "none")] {
                                        button { class: if ex.pq_compression == v { "seg on" } else { "seg" }, onclick: move |_| { export.write().pq_compression = v.to_string(); }, "{l}" }
                                    }
                                }
                                if matches!(ex.pq_compression.as_str(), "zstd" | "gzip" | "brotli") {
                                    div { class: "row", style: "gap:8px;margin-top:8px;align-items:center;",
                                        span { class: "opt-lbl", "Level" }
                                        input { class: "input mono", r#type: "number", style: "width:70px;height:28px;padding:0 8px;", value: "{ex.pq_level}",
                                            oninput: move |e| { if let Ok(n) = e.value().parse::<u32>() { export.write().pq_level = n; } } }
                                    }
                                }
                            },
                            "json" => rsx! { div { style: "font-size:12px;color:var(--dim2);", "Newline-delimited JSON — one record per line." } },
                            "arrow" => rsx! { div { style: "font-size:12px;color:var(--dim2);", "Arrow IPC file — no write options." } },
                            _ => rsx! {
                                div { class: "row", style: "gap:8px;align-items:center;",
                                    span { class: "opt-lbl", "Copy as" }
                                    for (v, l) in [("markdown", "Markdown"), ("tsv", "TSV"), ("csv", "CSV"), ("json", "JSON")] {
                                        button { class: if ex.clip_format == v { "seg on" } else { "seg" }, onclick: move |_| { export.write().clip_format = v.to_string(); }, "{l}" }
                                    }
                                }
                            },
                        }
                    }

                    // PARTITION BY (file formats only)
                    if !is_clip {
                        div { class: "field-label", style: "margin-top:16px;", "PARTITION BY (optional)" }
                        if cols.is_empty() {
                            div { style: "font-size:12px;color:var(--faint);", "Run a query to choose partition columns." }
                        } else {
                            div { class: "row", style: "gap:6px;flex-wrap:wrap;",
                                for col in cols.iter().cloned() {
                                    {
                                        let order = ex.partition_cols.iter().position(|c| c == &col);
                                        let label = match order { Some(i) => format!("{}  {}", i + 1, col), None => col.clone() };
                                        let on = order.is_some();
                                        let colc = col.clone();
                                        rsx! {
                                            button { class: if on { "seg on" } else { "seg" },
                                                onclick: move |_| {
                                                    let mut w = export.write();
                                                    let pc = &mut w.partition_cols;
                                                    if let Some(i) = pc.iter().position(|c| c == &colc) { pc.remove(i); } else { pc.push(colc.clone()); }
                                                },
                                                "{label}"
                                            }
                                        }
                                    }
                                }
                            }
                            if !ex.partition_cols.is_empty() {
                                div { class: "row", style: "gap:11px;cursor:pointer;padding:8px 0 0;",
                                    onclick: move |_| { let mut w = export.write(); w.keep_partition = !w.keep_partition; },
                                    div { class: if ex.keep_partition { "toggle on" } else { "toggle" }, div { class: "knob" } }
                                    span { style: "font-size:12px;color:var(--text3);", "Keep partition columns inside the files" }
                                }
                            }
                        }
                    }

                    // DESTINATION (file formats only)
                    if !is_clip {
                        div { class: "field-label", style: "margin-top:16px;", "DESTINATION" }
                        div { style: "display:flex;align-items:center;height:34px;background:var(--bg);border:1px solid var(--line2);border-radius:9px;overflow:hidden;max-width:360px;",
                            input { class: "input mono", style: "padding:0 11px;", value: "{ex.name}",
                                oninput: move |e| export.write().name = e.value() }
                            span { class: "mono", style: "padding:0 11px;color:var(--accent);border-left:1px solid var(--line2);height:100%;display:flex;align-items:center;",
                                if ex.partition_cols.is_empty() { "{ext}" } else { "/ (folder)" } }
                        }
                    }

                    // PREVIEW
                    div { class: "row", style: "margin-top:16px;justify-content:space-between;align-items:baseline;",
                        span { class: "field-label", "PREVIEW" }
                        span { class: "mono", style: "font-size:11px;color:var(--dim2);", "est. {size_est}" }
                    }
                    pre { class: "preview-pre ps-scroll", "{preview}" }
                }
        }
    }
}

/// Preview text (first few rows in the chosen format) + an estimated file size.
fn export_preview(_state: Signal<AppState>, ex: &ExportForm) -> (String, String) {
    let id = crate::session::active_id();
    let runs = crate::runs::RUNS.read();
    let Some(res) = runs
        .get(&id)
        .and_then(|r| r.result.as_ref())
    else {
        return (String::new(), String::new());
    };
    // Effective format for preview (clipboard uses its sub-format).
    let eff = if ex.format == "clipboard" {
        ex.clip_format.as_str()
    } else {
        ex.format.as_str()
    };
    let cols: Vec<&str> = res.columns.iter().map(|c| c.name.as_str()).collect();
    let est = estimate_size(res, eff, ex);

    // Columnar formats: show a schema summary, not rows.
    if eff == "parquet" || eff == "arrow" {
        let mut out = format!(
            "{} · {} columns\n",
            if eff == "parquet" {
                "Parquet"
            } else {
                "Arrow IPC"
            },
            res.columns.len()
        );
        for c in &res.columns {
            out.push_str(&format!("  {}: {}\n", c.name, c.dtype));
        }
        return (out, est);
    }

    let take = res.rows.iter().take(5);
    let sep = if eff == "tsv" { '\t' } else { ',' };
    let text = match eff {
        "json" => {
            let mut out = String::new();
            for r in take {
                let obj: Vec<String> = r
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        format!(
                            "\"{}\": {}",
                            cols.get(i).unwrap_or(&""),
                            if c.null {
                                "null".into()
                            } else {
                                format!("\"{}\"", c.text)
                            }
                        )
                    })
                    .collect();
                out.push_str(&format!("{{{}}}\n", obj.join(", ")));
            }
            out
        }
        "markdown" => {
            let mut out = format!("| {} |\n", cols.join(" | "));
            out.push_str(&format!("|{}\n", " --- |".repeat(cols.len())));
            for r in take {
                let line: Vec<String> = r
                    .iter()
                    .map(|c| {
                        if c.null {
                            String::new()
                        } else {
                            c.text.clone()
                        }
                    })
                    .collect();
                out.push_str(&format!("| {} |\n", line.join(" | ")));
            }
            out
        }
        _ => {
            // csv / tsv
            let mut out = cols.join(&sep.to_string());
            out.push('\n');
            for r in take {
                let line: Vec<String> = r
                    .iter()
                    .map(|c| {
                        if c.null {
                            String::new()
                        } else {
                            c.text.clone()
                        }
                    })
                    .collect();
                out.push_str(&line.join(&sep.to_string()));
                out.push('\n');
            }
            out
        }
    };
    (text, est)
}

/// Rough exported-size estimate: avg text bytes/row × total rows, scaled by
/// compression for columnar formats. Approximate — for a UI hint only.
fn estimate_size(res: &crate::engine::QueryOutput, eff: &str, ex: &ExportForm) -> String {
    let sample = res.rows.len().min(20);
    if sample == 0 {
        return "0 B".into();
    }
    let mut bytes = 0usize;
    for row in res.rows.iter().take(sample) {
        for c in row {
            bytes += c.text.len() + 1;
        }
    }
    let avg = bytes / sample;
    let total_rows = if ex.scope == "page" {
        res.rows.len()
    } else {
        res.total
    };
    let raw = avg.saturating_mul(total_rows);
    let factor = if eff == "parquet" {
        match ex.pq_compression.as_str() {
            "none" => 0.6,
            "snappy" | "lz4" => 0.45,
            _ => 0.28,
        }
    } else if eff == "arrow" {
        0.7
    } else {
        1.0
    };
    human_bytes((raw as f64 * factor) as usize)
}

fn human_bytes(n: usize) -> String {
    let n = n as f64;
    if n < 1024.0 {
        format!("{} B", n as usize)
    } else if n < 1024.0 * 1024.0 {
        format!("{:.1} KB", n / 1024.0)
    } else if n < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", n / 1024.0 / 1024.0)
    } else {
        format!("{:.1} GB", n / 1024.0 / 1024.0 / 1024.0)
    }
}
