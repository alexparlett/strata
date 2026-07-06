//! Right column inspector: type, stats (over the current result), nested
//! fields, completeness.

use std::collections::HashSet;

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::AppState;
use crate::ui::icons;
use crate::util::Kind;

#[component]
pub fn Inspector() -> Element {
    let state = use_context::<Signal<AppState>>();
    let s = state.read();
    let width = s.inspector_w;

    let Some((table, colname)) = s.selected_col.clone() else {
        return rsx! {
            aside { class: "ps-inspector", style: "width:{width}px;",
                div { class: "insp-head",
                    span { class: "sec-label", "COLUMN INSPECTOR" }
                    button { class: "icon-btn plain", style: "width:24px;height:24px;",
                        onclick: move |_| dispatch(state, Action::CloseInspector), {icons::close(12)} }
                }
                div { style: "padding:24px 14px;color:var(--dim2);font-size:12px;", "Select a column to inspect." }
            }
        };
    };

    // column meta from catalog
    let colinfo = s
        .project
        .tables
        .iter()
        .find(|t| t.name == table)
        .and_then(|t| t.columns.iter().find(|c| c.name == colname).cloned())
        .or_else(|| {
            s.project
                .views
                .iter()
                .find(|v| v.name == table)
                .and_then(|v| v.columns.iter().find(|c| c.name == colname).cloned())
        });
    let kind = colinfo.as_ref().map(|c| c.kind).unwrap_or(Kind::Str);
    let dtype = colinfo
        .as_ref()
        .map(|c| c.dtype.clone())
        .unwrap_or_default();
    let children = colinfo.map(|c| c.children).unwrap_or_default();

    // stats over the current result
    let mut rows = 0usize;
    let mut nulls = 0usize;
    let mut distinct: HashSet<String> = HashSet::new();
    let mut nums: Vec<f64> = Vec::new();
    let active_id = s.active_tab_id();
    let runs = crate::runs::RUNS.read();
    if let Some(res) = active_id
        .and_then(|id| runs.get(&id))
        .and_then(|r| r.result.as_ref())
    {
        if let Some(ci) = res.columns.iter().position(|c| c.name == colname) {
            for r in &res.rows {
                if let Some(cell) = r.get(ci) {
                    rows += 1;
                    if cell.null {
                        nulls += 1;
                    } else {
                        distinct.insert(cell.text.clone());
                        if let Ok(v) = cell.text.parse::<f64>() {
                            nums.push(v);
                        }
                    }
                }
            }
        }
    }
    drop(s);

    let ndist = distinct.len();
    let null_pct = if rows > 0 {
        (nulls as f64 / rows as f64 * 100.0).round() as i64
    } else {
        0
    };
    let fill = if rows > 0 { 100 - null_pct } else { 100 };
    let is_num = kind == Kind::Num;
    let (min, max) = if nums.is_empty() {
        (0.0, 0.0)
    } else {
        (
            nums.iter().cloned().fold(f64::INFINITY, f64::min),
            nums.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        )
    };

    let dot = kind.dot_class();
    let tcls = kind.text_class();
    let nested = kind.is_nested();

    rsx! {
        aside { class: "ps-inspector ps-scroll", style: "width:{width}px;",
            div { class: "insp-head",
                span { class: "sec-label", "COLUMN INSPECTOR" }
                button { class: "icon-btn plain", style: "width:24px;height:24px;",
                    onclick: move |_| dispatch(state, Action::CloseInspector), {icons::close(12)} }
            }

            div { class: "insp-title",
                div { class: "row", style: "gap:8px;",
                    span { class: "dot {dot}", style: "width:9px;height:9px;border-radius:3px;" }
                    span { class: "insp-name", "{colname}" }
                }
                div { class: "row", style: "gap:8px;margin-top:6px;",
                    span { class: "mono {tcls}", style: "font-size:10.5px;background:rgba(255,255,255,.05);padding:2px 7px;border-radius:5px;", "{dtype}" }
                    span { class: "mono", style: "font-size:11px;color:var(--dim2);", "from {table}" }
                }
            }

            div { class: "insp-stats",
                {stat("Rows", &rows.to_string(), "var(--text)")}
                {stat("Nulls", &format!("{nulls} · {null_pct}%"), if nulls > 0 { "var(--orange)" } else { "var(--text)" })}
                {stat("Distinct", &ndist.to_string(), "var(--text)")}
                {stat("Type", &dtype, "var(--accent)")}
            }

            if nested {
                div { class: "insp-note", "Nested column — expand values in the results grid (click a cell) or use get_field / unnest to project fields." }
                if !children.is_empty() {
                    div { class: "insp-section",
                        div { class: "sec-label", style: "margin-bottom:8px;", "NESTED FIELDS" }
                        div { class: "nested-box",
                            for f in children.iter() {
                                div { class: "nested-field",
                                    span { class: "dot {f.kind.dot_class()}" }
                                    span { class: "fname", "{f.name}" }
                                    span { class: "ftype {f.kind.text_class()}", "{f.dtype}" }
                                }
                            }
                        }
                    }
                }
            }

            div { class: "insp-section",
                div { class: "row", style: "justify-content:space-between;margin-bottom:6px;",
                    span { class: "mono", style: "font-size:10.5px;color:var(--dim);", "Completeness" }
                    span { class: "mono", style: "font-size:10.5px;color:var(--text2);", "{fill}%" }
                }
                div { class: "fill-track", div { class: "fill-bar", style: "width:{fill}%;" } }
            }

            if is_num && !nums.is_empty() {
                div { style: "padding:8px 14px 18px;",
                    div { style: "height:4px;border-radius:2px;background:linear-gradient(90deg,var(--line),var(--accent),var(--line));margin:8px 0 6px;" }
                    div { class: "row mono", style: "justify-content:space-between;font-size:10px;color:var(--dim2);",
                        span { "min {min}" }
                        span { "max {max}" }
                    }
                }
            }
        }
    }
}

fn stat(label: &str, value: &str, color: &str) -> Element {
    rsx! {
        div { class: "insp-stat",
            div { class: "k", "{label}" }
            div { class: "v", style: "color:{color};", "{value}" }
        }
    }
}
