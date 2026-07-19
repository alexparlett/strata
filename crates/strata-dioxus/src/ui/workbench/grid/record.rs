//! The record (row-detail) view (Rz5) — `RecordDialog` + its copy menu. Split out of the grid
//! module; reads the run directly and reuses `crate::serialize` for the nested pretty-JSON + copy.

use dioxus::prelude::*;
use dioxus_code::{Code, SourceCode};

use crate::action::{dispatch, Action};
use crate::engine::serialize::TextFormat;
use crate::model::Cell;
use crate::session::WorkspaceId;
use crate::ui::components::{
    Dialog, DropdownMenu, Icon, IconButton, IconButtonVariant, MenuItem, Meta, MonoValue, Readout,
    RectAlign, Spacer,
};
use crate::ui::icons::{IconName, IconSize};

/// The record (row-detail) view (Rz5) — a workspace-local modal showing one row as a **key → value**
/// card, with page-local prev/next navigation and a `⋯` menu to copy the record in any format. It
/// reads the run directly (result + filter), rebuilding the same filtered page the grid shows, so
/// `idx` (a page-local filtered row index) matches the double-clicked gutter row without prop clones.
#[component]
pub fn RecordDialog(ws_id: WorkspaceId, idx: Signal<Option<usize>>) -> Element {
    let mut idx = idx;

    let Some(entry) = crate::runs::RUNS.resolve().get(ws_id) else {
        return rsx! {};
    };
    let run = entry.read();
    let Some(result) = run.result.clone() else {
        return rsx! {};
    };
    let page_batch = run.page_batch.clone();
    let search = run.result_search.to_lowercase();
    let base = run.page.saturating_sub(1) * run.page_size;
    drop(run);

    let total = result.total;
    // (name, arrow dtype, type-text class, value cell class, nested?). The key shows the name over
    // its type (type-coloured); values are coloured like the grid, nested ones shown as a block.
    let cols: Vec<(String, String, &'static str, &'static str, bool)> = result
        .columns
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                c.dtype.clone(),
                c.kind.text_class(),
                c.kind.cell_class(),
                c.kind.is_nested(),
            )
        })
        .collect();
    // Same filtered page the grid renders (find-box filter is page-local), numbered globally.
    let rows: Vec<(usize, Vec<Cell>)> = result
        .rows
        .iter()
        .enumerate()
        .filter(|(_, r)| search.is_empty() || r.iter().any(|c| c.text.to_lowercase().contains(&search)))
        .map(|(i, r)| (base + i + 1, r.clone()))
        .collect();

    let n = rows.len();
    if n == 0 {
        // The filter emptied the page out from under us — nothing to render (don't mutate the
        // signal mid-render); the record reappears if the rows come back.
        return rsx! {};
    }
    let cur = idx().unwrap_or(0).min(n - 1);
    let (rownum, cells) = rows[cur].clone();
    // Page-batch row for this record = its original (unfiltered) page index = rownum - base - 1.
    let batch_row = rownum.saturating_sub(base + 1);

    rsx! {
        Dialog { on_close: move |_| idx.set(None), card_class: "modal record-modal".to_string(), z: 64,
            div { class: "record-head",
                MonoValue { class: "record-title", "Row {rownum} of {total}" }
                Spacer {}
                IconButton {
                    icon: IconName::ChevronUp, variant: IconButtonVariant::Ghost, title: "Previous row",
                    disabled: cur == 0,
                    onclick: move |_| if cur > 0 { idx.set(Some(cur - 1)); },
                }
                IconButton {
                    icon: IconName::ChevronDown, variant: IconButtonVariant::Ghost, title: "Next row",
                    disabled: cur + 1 >= n,
                    onclick: move |_| if cur + 1 < n { idx.set(Some(cur + 1)); },
                }
                DropdownMenu {
                    class: "icon-btn plain", style: "width:28px;height:28px;", title: "Copy record",
                    align: RectAlign::BOTTOM_END, width: 190,
                    trigger: rsx! { Icon { name: IconName::Dots, size: IconSize::Sm } },
                    {record_copy_items(cur)}
                }
                IconButton {
                    icon: IconName::Close, variant: IconButtonVariant::Ghost, title: "Close",
                    onclick: move |_| idx.set(None),
                }
            }
            div { class: "record-body ps-scroll",
                for (ci, (name, dtype, tclass, cclass, nested)) in cols.iter().cloned().enumerate() {
                    div { class: "record-row",
                        div { class: "record-key",
                            MonoValue { class: "record-name", "{name}" }
                            Meta { class: format!("record-type {tclass}"), "{dtype}" }
                        }
                        {
                            match cells.get(ci) {
                                Some(c) if c.null => rsx! { Meta { class: "record-val null", "NULL" } },
                                // Nested (struct/list/map) → pretty JSON of the value (arrow-json +
                                // serde_json indent), in a recessed box. Falls back to the display
                                // text if the page batch isn't available.
                                Some(c) if nested => {
                                    let json = page_batch
                                        .as_ref()
                                        .and_then(|b| crate::engine::serialize::cell_pretty_json(b, ci, batch_row))
                                        .unwrap_or_else(|| c.text.clone());
                                    rsx! {
                                        div { class: "record-val record-nested",
                                            Code {
                                                src: SourceCode::new(crate::ui::lang("json"), json),
                                                theme: crate::ui::code_theme(),
                                            }
                                        }
                                    }
                                },
                                Some(c) => rsx! {
                                    Readout {
                                        class: format!("record-val {cclass}"),
                                        "{c.text}"
                                    }
                                },
                                None => rsx! { Readout { class: "record-val", "" } },
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The four "Copy as …" rows for the record `⋯` menu — copies row `row` (all columns) in each
/// format via [`Action::CopyRecord`]. The `DropdownMenu` closes itself on any row click.
fn record_copy_items(row: usize) -> Element {
    rsx! {
        MenuItem { label: "Copy as TSV".to_string(), onclick: move |_| dispatch(Action::CopyRecord(row, TextFormat::Tsv)) }
        MenuItem { label: "Copy as CSV".to_string(), onclick: move |_| dispatch(Action::CopyRecord(row, TextFormat::Csv)) }
        MenuItem { label: "Copy as JSON".to_string(), onclick: move |_| dispatch(Action::CopyRecord(row, TextFormat::Json)) }
        MenuItem { label: "Copy as Markdown".to_string(), onclick: move |_| dispatch(Action::CopyRecord(row, TextFormat::Markdown)) }
    }
}
