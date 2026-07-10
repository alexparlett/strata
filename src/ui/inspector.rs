//! Right column inspector: type, stats (over the current result), nested
//! fields, completeness.

use std::collections::HashSet;

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::AppState;
use crate::ui::components::{
    Dot, Eyebrow, IconButton, IconButtonVariant, Meta, MonoValue, Path, Prose, Readout,
};
use crate::ui::icons::{IconName, IconSize};
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
                    Eyebrow { class: "sec-label", "COLUMN INSPECTOR" }
                    IconButton { icon: IconName::Close, icon_size: IconSize::Xs, variant: IconButtonVariant::Ghost, title: "Close inspector",
                        onclick: move |_| dispatch(state, Action::CloseInspector), }
                }
                Prose { style: "padding:var(--sp-6) var(--sp-4);color:var(--dim2);", "Select a column to inspect." }
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
    let active_id = crate::session::active_id();
    let result = crate::runs::RUNS
        .resolve()
        .get(active_id)
        .and_then(|e| e.read().result.clone());
    if let Some(res) = result.as_ref() {
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

    let dot = kind.dot_color();
    let tcls = kind.text_class();
    let nested = kind.is_nested();

    rsx! {
        aside { class: "ps-inspector ps-scroll", style: "width:{width}px;",
            div { class: "insp-head",
                Eyebrow { class: "sec-label", "COLUMN INSPECTOR" }
                IconButton { icon: IconName::Close, icon_size: IconSize::Xs, variant: IconButtonVariant::Ghost, title: "Close inspector",
                    onclick: move |_| dispatch(state, Action::CloseInspector), }
            }

            div { class: "insp-title",
                div { class: "row", style: "gap:var(--sp-3);",
                    Dot { color: "{dot}", square: true, size: 8 }
                    MonoValue { class: "insp-name", "{colname}" }
                }
                div { class: "row", style: "gap:var(--sp-3);margin-top:var(--sp-3);",
                    Meta { class: "{tcls} insp-dtype", "{dtype}" }
                    Path { "from {table}" }
                }
            }

            div { class: "insp-stats",
                {stat("Rows", &rows.to_string(), "var(--text)")}
                {stat("Nulls", &format!("{nulls} · {null_pct}%"), if nulls > 0 { "var(--orange)" } else { "var(--text)" })}
                {stat("Distinct", &ndist.to_string(), "var(--text)")}
                {stat("Type", &dtype, "var(--accent)")}
            }

            if nested {
                div { class: "insp-note", Path { "Nested column — expand values in the results grid (click a cell) or use get_field / unnest to project fields." } }
                if !children.is_empty() {
                    div { class: "insp-section",
                        Eyebrow { class: "sec-label", style: "margin-bottom:var(--sp-3);", "NESTED FIELDS" }
                        div { class: "nested-box",
                            for f in children.iter() {
                                div { class: "nested-field",
                                    Dot { color: "{f.kind.dot_color()}", square: true, size: 6 }
                                    Readout { class: "fname", "{f.name}" }
                                    Meta { class: "ftype {f.kind.text_class()}", "{f.dtype}" }
                                }
                            }
                        }
                    }
                }
            }

            div { class: "insp-section",
                div { class: "row", style: "justify-content:space-between;margin-bottom:var(--sp-3);",
                    Meta { style: "color:var(--dim);", "Completeness" }
                    Meta { style: "color:var(--text2);", "{fill}%" }
                }
                div { class: "fill-track", div { class: "fill-bar", style: "width:{fill}%;" } }
            }

            if is_num && !nums.is_empty() {
                div { style: "padding:var(--sp-3) var(--sp-4) var(--sp-5);",
                    div { style: "height:4px;border-radius:var(--r-xs);background:linear-gradient(90deg,var(--line),var(--accent),var(--line));margin:var(--sp-3) 0 var(--sp-3);" }
                    div { class: "row", style: "justify-content:space-between;",
                        Meta { "min {min}" }
                        Meta { "max {max}" }
                    }
                }
            }
        }
    }
}

fn stat(label: &str, value: &str, color: &str) -> Element {
    rsx! {
        div { class: "insp-stat",
            Meta { class: "k", "{label}" }
            MonoValue { class: "v", style: "color:{color};", "{value}" }
        }
    }
}
