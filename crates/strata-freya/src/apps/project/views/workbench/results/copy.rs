//! The shared **results-copy** capability (P2-11 / Dioxus Rz4): resolve the grid's
//! selection against the current page, serialize it with the core writers
//! (`strata_core::engine::serialize` — TSV / CSV / JSON / Markdown, headers on, nested
//! cells as real JSON), and land the text on the system clipboard via
//! `freya::clipboard` — the same per-window copypasta provider the text inputs use, so
//! there is exactly one clipboard stack in the app.
//!
//! Consumers: the grid's right-click [`copy_menu`] and focused ⌘C (TSV), and the record
//! view's Copy row as CSV / JSON buttons ([`copy_record_csv`] / [`copy_record_json`]) —
//! per the shared-mechanism rule, they all route through here rather than growing local
//! clipboard wiring.

use std::rc::Rc;

use freya::clipboard::Clipboard;
use freya::prelude::*;

use strata_core::config::Command;
use strata_core::engine::serialize::{row_pretty_json, write_selection, TextFormat};

use super::cell_view::page_batch_row;
use super::datagrid::GridData;
use super::selection::Selection;
use crate::apps::project::views::workbench::tab_bar::menu::{menu_row, HINT_MENU_WIDTH};
use crate::components::typography::Prose;

/// Resolve the selection to sorted, in-page **display** rows + columns: a cell rectangle
/// is its bounds, whole rows carry every column, whole columns every row. `None` when
/// there is nothing to copy. Row/column picks are stored in click order and can outlive
/// a page flip, so they're sorted and bounds-checked here.
fn resolve(sel: &Selection, nrows: usize, ncols: usize) -> Option<(Vec<usize>, Vec<usize>)> {
    let (rows, cols) = match sel {
        Selection::None => return None,
        Selection::Cell { .. } => {
            let (minr, maxr, minc, maxc) = sel.cell_bounds()?;
            ((minr..=maxr).collect(), (minc..=maxc).collect())
        }
        Selection::Rows(rows) => {
            let mut rows = rows.clone();
            rows.sort_unstable();
            (rows, (0..ncols).collect())
        }
        Selection::Cols(cols) => {
            let mut cols = cols.clone();
            cols.sort_unstable();
            ((0..nrows).collect(), cols)
        }
    };
    let rows: Vec<usize> = rows.into_iter().filter(|&r| r < nrows).collect();
    let cols: Vec<usize> = cols.into_iter().filter(|&c| c < ncols).collect();
    (!rows.is_empty() && !cols.is_empty()).then_some((rows, cols))
}

/// Serialize the selection in `fmt` and commit it to the clipboard. Display rows map back
/// to page-batch rows through [`page_batch_row`], so a find-filtered page copies the rows
/// the user actually sees. Returns whether there *was* a selection — the focused ⌘C
/// declines on `false`, leaving the press unconsumed.
pub fn copy_selection(
    fmt: TextFormat,
    data: &GridData,
    row_nums: Option<&[usize]>,
    row_base: usize,
    sel: &Selection,
) -> bool {
    let Some((rows, cols)) = resolve(sel, data.rows.len(), data.columns.len()) else {
        return false;
    };
    let batch_rows: Vec<u32> = rows
        .iter()
        .map(|&r| page_batch_row(row_nums, row_base, r) as u32)
        .collect();
    let mut buf = Vec::new();
    match write_selection(fmt, &data.batch, &batch_rows, &cols, true, &mut buf) {
        Ok(()) => commit(buf),
        Err(err) => tracing::warn!("results copy failed to serialize: {err}"),
    }
    true
}

/// The record view's **Copy row as CSV**: header + the one page-batch row, all columns.
pub fn copy_record_csv(data: &GridData, batch_row: usize) {
    let cols: Vec<usize> = (0..data.columns.len()).collect();
    let mut buf = Vec::new();
    match write_selection(TextFormat::Csv, &data.batch, &[batch_row as u32], &cols, true, &mut buf) {
        Ok(()) => commit(buf),
        Err(err) => tracing::warn!("record CSV copy failed to serialize: {err}"),
    }
}

/// The record view's **Copy row as JSON**: the bare `{column: value}` object (nulls
/// explicit), per the canvas `buildRowJSON` — not `write_selection`'s array-of-objects.
pub fn copy_record_json(data: &GridData, batch_row: usize) {
    match row_pretty_json(&data.batch, batch_row) {
        Some(json) => commit(json.into_bytes()),
        None => tracing::warn!("record JSON copy failed to serialize"),
    }
}

/// Land serialized bytes on the system clipboard, warning (not erroring UI-side) on the
/// rare failure — copy is fire-and-forget.
fn commit(buf: Vec<u8>) {
    match String::from_utf8(buf) {
        Ok(text) => {
            if let Err(err) = Clipboard::set(text) {
                tracing::warn!("clipboard write failed: {err:?}");
            }
        }
        Err(err) => tracing::warn!("results copy produced non-utf8 output: {err}"),
    }
}

/// The grid's right-click **copy menu**: the four formats, with the keymap-derived ⌘C
/// hint on the TSV row (Copy = TSV). Built from a cell's secondary-press handler (no
/// hooks); each action reads the **live** selection at press time — the menu can't go
/// stale against a selection change between open and pick.
pub fn copy_menu(
    data: Rc<GridData>,
    row_nums: Option<Rc<Vec<usize>>>,
    row_base: usize,
    sel: State<Selection>,
) -> Menu {
    let item = |label: Element, fmt: TextFormat| {
        let data = data.clone();
        let row_nums = row_nums.clone();
        MenuButton::new()
            .on_press(move |_| {
                copy_selection(
                    fmt,
                    &data,
                    row_nums.as_ref().map(|n| n.as_slice()),
                    row_base,
                    &sel.peek(),
                );
                ContextMenu::close();
            })
            .child(label)
    };
    Menu::new()
        .min_width(Size::px(HINT_MENU_WIDTH))
        .child(item(menu_row("Copy as TSV", Command::Copy).into_element(), TextFormat::Tsv))
        .child(item(Prose::new("Copy as CSV").into_element(), TextFormat::Csv))
        .child(item(Prose::new("Copy as JSON").into_element(), TextFormat::Json))
        .child(item(Prose::new("Copy as Markdown").into_element(), TextFormat::Markdown))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The selection → (rows, cols) resolution over a 3×2 page: rectangle bounds, whole
    /// rows/cols completion, sorting of click-order picks, and out-of-page pruning.
    #[test]
    fn resolve_covers_every_selection_shape() {
        assert_eq!(resolve(&Selection::None, 3, 2), None);
        assert_eq!(
            resolve(&Selection::Cell { ar: 2, ac: 1, fr: 1, fc: 0 }, 3, 2),
            Some((vec![1, 2], vec![0, 1]))
        );
        assert_eq!(
            resolve(&Selection::Rows(vec![2, 0]), 3, 2),
            Some((vec![0, 2], vec![0, 1]))
        );
        assert_eq!(
            resolve(&Selection::Cols(vec![1]), 3, 2),
            Some((vec![0, 1, 2], vec![1]))
        );
    }

    #[test]
    fn resolve_prunes_rows_that_outlived_the_page() {
        // A row pick from a longer page: the stale index drops, the valid one survives.
        assert_eq!(resolve(&Selection::Rows(vec![7, 1]), 3, 2), Some((vec![1], vec![0, 1])));
        // Nothing left in-page → nothing to copy.
        assert_eq!(resolve(&Selection::Rows(vec![7]), 3, 2), None);
    }
}
