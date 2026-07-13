//! Settings ▸ Data display page — row density, zebra striping, default row limit.

use dioxus::prelude::*;

use crate::ui::components::{Caption, Segment, SegmentOption, Strong, Toggle};

#[component]
pub(super) fn DataDisplay() -> Element {
    let mut draft = use_context::<super::SettingsCtx>().draft;
    let d = draft.read();
    let density_compact = d.density_compact;
    let zebra = d.zebra;
    let row_limit = d.row_limit;
    drop(d);
    rsx! {
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
    }
}
