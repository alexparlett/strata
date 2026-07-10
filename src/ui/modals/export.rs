//! Export modal: format cards, options, live preview + size estimate.
use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, ExportForm};
use crate::ui::components::{
    Body, Button, ButtonVariant, Caption, Control, Eyebrow, Icon, Meta, MonoValue, NumberStepper,
    Prose, Readout, Segment, SegmentOption, Select, SelectOption, Spacer, TextInput, Toggle,
    WinGeom, Window,
};
use crate::ui::icons::{IconName, IconSize};

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

/// Format selector — a grid of selectable cards (icon + name + blurb). Kept local to the
/// export modal for now; promote to a shared `CardSelect` if a second card-select appears.
#[component]
fn FormatCards(value: String, on_select: EventHandler<String>) -> Element {
    rsx! {
        div { class: "fmt-grid",
            for (id, label, desc) in [("csv", "CSV", "delimited text"), ("json", "JSON", "ndjson records"), ("parquet", "Parquet", "columnar"), ("arrow", "Arrow", "IPC"), ("clipboard", "Clipboard", "copy as text")] {
                button {
                    class: if value == id { "fmt-card on" } else { "fmt-card" },
                    onclick: move |_| on_select.call(id.to_string()),
                    Icon { name: IconName::Download, size: IconSize::Sm }
                    Control { class: "fn", "{label}" }
                    Meta { class: "fd", "{desc}" }
                }
            }
        }
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
        crate::runs::RUNS
            .resolve()
            .get(id)
            .map(|e| {
                let run = e.read();
                let run_res = run.result.as_ref();
                let total = run_res.map(|r| r.total).unwrap_or(0);
                let cols: Vec<String> = run_res
                    .map(|r| r.columns.iter().map(|c| c.name.clone()).collect())
                    .unwrap_or_default();
                (total, cols)
            })
            .unwrap_or((0, Vec::new()))
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
            icon: IconName::Download, icon_size: IconSize::Md,
            init: WinGeom::new(200.0, 84.0, 820.0, 640.0),
            min_w: 640.0,
            min_h: 420.0,
            footer: rsx! {
                Spacer {}
                Button { variant: ButtonVariant::Secondary, onclick: move |_| on_close.call(()), "Cancel" }
                Button { variant: ButtonVariant::Primary, icon: IconName::Download, icon_size: IconSize::Sm,
                    onclick: move |_| dispatch(state, Action::RunExport(export())),
                    if is_clip { "Copy" } else { "Export" }
                }
            },
            div { class: "modal-body ps-scroll",
                    Eyebrow { class: "sec-label", style: "margin-bottom:10px;", "FORMAT" }
                    FormatCards { value: fmt.clone(), on_select: move |v| { export.write().format = v; } }

                    // ROWS TO EXPORT
                    Eyebrow { class: "field-label", style: "margin-top:16px;", "ROWS TO EXPORT" }
                    Segment {
                        value: ex.scope.clone(),
                        on_select: move |v: String| { export.write().scope = v; },
                        options: vec![
                            SegmentOption::new("all", format!("All ({total})")),
                            SegmentOption::new("page", "This page"),
                        ],
                    }

                    // OPTIONS (format-swapped)
                    Eyebrow { class: "field-label", style: "margin-top:16px;", "OPTIONS" }
                    {
                        match fmt.as_str() {
                            "csv" => rsx! {
                                div { style: "padding:2px 0 8px;",
                                    Toggle { on: ex.csv_header, on_toggle: move |v| export.write().csv_header = v, "Include header row" }
                                }
                                div { class: "row", style: "gap:8px;margin-bottom:8px;align-items:center;",
                                    Caption { class: "opt-lbl", "Delimiter" }
                                    Segment {
                                        value: ex.csv_delim.clone(),
                                        on_select: move |v: String| { export.write().csv_delim = v; },
                                        options: vec![
                                            SegmentOption::new("comma", "Comma"),
                                            SegmentOption::new("tab", "Tab"),
                                            SegmentOption::new("semicolon", "Semicolon"),
                                            SegmentOption::new("pipe", "Pipe"),
                                        ],
                                    }
                                }
                                div { class: "row", style: "gap:8px;align-items:center;",
                                    Caption { class: "opt-lbl", "Null as" }
                                    Segment {
                                        value: ex.csv_null.clone(),
                                        on_select: move |v: String| { export.write().csv_null = v; },
                                        options: vec![
                                            SegmentOption::new("empty", "(empty)"),
                                            SegmentOption::new("null", "NULL"),
                                            SegmentOption::new("nan", "NaN"),
                                        ],
                                    }
                                }
                            },
                            "parquet" => rsx! {
                                div { class: "row", style: "gap:8px;align-items:center;",
                                    Caption { class: "opt-lbl", "Compression" }
                                    Select {
                                        value: ex.pq_compression.clone(),
                                        width: 140,
                                        options: vec![
                                            SelectOption::new("zstd", "zstd"),
                                            SelectOption::new("snappy", "snappy"),
                                            SelectOption::new("gzip", "gzip"),
                                            SelectOption::new("brotli", "brotli"),
                                            SelectOption::new("lz4", "lz4"),
                                            SelectOption::new("none", "none"),
                                        ],
                                        on_select: move |v: String| { export.write().pq_compression = v; },
                                    }
                                }
                                if matches!(ex.pq_compression.as_str(), "zstd" | "gzip" | "brotli") {
                                    div { class: "row", style: "gap:8px;margin-top:8px;align-items:center;",
                                        Caption { class: "opt-lbl", "Level" }
                                        NumberStepper { value: ex.pq_level as i64, min: 1, max: 22, width: 96,
                                            on_change: move |v: i64| export.write().pq_level = v as u32 }
                                    }
                                }
                            },
                            "json" => rsx! { Prose { style: "color:var(--dim2);", "Newline-delimited JSON — one record per line." } },
                            "arrow" => rsx! { Prose { style: "color:var(--dim2);", "Arrow IPC file — no write options." } },
                            _ => rsx! {
                                div { class: "row", style: "gap:8px;align-items:center;",
                                    Caption { class: "opt-lbl", "Copy as" }
                                    Segment {
                                        value: ex.clip_format.clone(),
                                        on_select: move |v: String| { export.write().clip_format = v; },
                                        options: vec![
                                            SegmentOption::new("markdown", "Markdown"),
                                            SegmentOption::new("tsv", "TSV"),
                                            SegmentOption::new("csv", "CSV"),
                                            SegmentOption::new("json", "JSON"),
                                        ],
                                    }
                                }
                            },
                        }
                    }

                    // PARTITION BY (file formats only)
                    if !is_clip {
                        Eyebrow { class: "field-label", style: "margin-top:16px;", "PARTITION BY (optional)" }
                        if cols.is_empty() {
                            Prose { style: "color:var(--faint);", "Run a query to choose partition columns." }
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
                                div { style: "padding:8px 0 0;",
                                    Toggle { on: ex.keep_partition, on_toggle: move |v| export.write().keep_partition = v, "Keep partition columns inside the files" }
                                }
                            }
                        }
                    }

                    // DESTINATION (file formats only)
                    if !is_clip {
                        Eyebrow { class: "field-label", style: "margin-top:16px;", "DESTINATION" }
                        TextInput { value: "{ex.name}", mono: true, width: 360,
                            oninput: move |v| export.write().name = v,
                            trailing: rsx! {
                                MonoValue { style: "padding:0 11px;color:var(--accent);border-left:1px solid var(--line2);align-self:stretch;display:flex;align-items:center;",
                                    if ex.partition_cols.is_empty() { "{ext}" } else { "/ (folder)" } }
                            },
                        }
                    }

                    // PREVIEW
                    div { class: "row", style: "margin-top:16px;justify-content:space-between;align-items:baseline;",
                        Eyebrow { class: "field-label", "PREVIEW" }
                        Meta { "est. {size_est}" }
                    }
                    Readout { class: "preview-pre ps-scroll", "{preview}" }
                }
        }
    }
}

/// Preview text (first few rows in the chosen format) + an estimated file size.
fn export_preview(_state: Signal<AppState>, ex: &ExportForm) -> (String, String) {
    let id = crate::session::active_id();
    let Some(entry) = crate::runs::RUNS.resolve().get(id) else {
        return (String::new(), String::new());
    };
    let run = entry.read();
    let Some(res) = run.result.as_ref() else {
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
