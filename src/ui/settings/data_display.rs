//! Settings ▸ Data display page — row density, zebra striping, default column width,
//! default row limit. Width + row limit are free-form numeric fields (design24): the
//! audience is engineers who'd rather type the exact value than pick an adjective.

use dioxus::prelude::*;

use crate::ui::components::{Caption, Segment, SegmentOption, Strong, TextInput, Toggle};

#[component]
pub(super) fn DataDisplay() -> Element {
    let mut draft = use_context::<super::SettingsCtx>().draft;
    let d = draft.read();
    let density_compact = d.density_compact;
    let zebra = d.zebra;
    let row_limit = d.row_limit;
    let col_width = d.default_col_width as i64;
    drop(d);
    rsx! {
        super::Anchor { id: "density",
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
        }

        div { class: "settings-divider", style: "margin:var(--sp-6) 0;" }
        super::Anchor { id: "zebra",
            div { class: "settings-row",
                div { style: "flex:1;",
                    Strong { style: "display:block;", "Alternating row colors" }
                    Caption { style: "display:block;color:var(--dim2);margin-top:var(--sp-1);", "Shade every other row in the results grid for easier scanning." }
                }
                Toggle {
                    on: zebra,
                    on_toggle: move |_| { let v = !zebra; draft.write().zebra = v; },
                }
            }
        }

        div { class: "settings-divider", style: "margin:var(--sp-6) 0;" }
        super::Anchor { id: "col-width",
            Strong { style: "display:block;margin-bottom:var(--sp-1);", "Default column width" }
            Caption { style: "display:block;color:var(--dim2);margin-bottom:var(--sp-4);max-width:460px;", "Starting width for result-grid columns before you resize them. Drag a column's edge to override it for that column, or double-click the edge to auto-fit." }
            div { class: "row", style: "gap:var(--sp-3);align-items:center;",
                TextInput {
                    value: "{col_width}",
                    mono: true,
                    width: 130,
                    oninput: move |_| {},
                    onchange: move |v: String| {
                        if let Ok(n) = v.trim().parse::<f64>() {
                            draft.write().default_col_width = n.max(40.0);
                        }
                    },
                }
                Caption { style: "color:var(--dim2);", "px" }
            }
        }

        div { class: "settings-divider", style: "margin:var(--sp-6) 0;" }
        super::Anchor { id: "row-limit",
            Strong { style: "display:block;margin-bottom:var(--sp-1);", "Default row limit" }
            Caption { style: "display:block;color:var(--dim2);margin-bottom:var(--sp-4);max-width:460px;", "New queries are generated with this LIMIT so a stray SELECT * can't pull a whole file into memory. Set to 0 for no limit." }
            div { class: "row", style: "gap:var(--sp-3);align-items:center;",
                TextInput {
                    value: "{row_limit}",
                    mono: true,
                    width: 130,
                    oninput: move |_| {},
                    onchange: move |v: String| {
                        if let Ok(n) = v.trim().parse::<usize>() {
                            draft.write().row_limit = n;
                        }
                    },
                }
                Caption { style: "color:var(--dim2);", "rows" }
            }
        }
    }
}
