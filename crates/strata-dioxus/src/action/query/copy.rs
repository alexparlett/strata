//! Clipboard copy — grid selection (Rz4) + single record (Rz5). Interprets the `runs`
//! selection into row/column indices; the actual batch slicing + serialization lives in
//! `crate::serialize` (this layer decides *what* to copy, not how to cut a `RecordBatch`).

use dioxus::prelude::*;

/// Rz4 — copy the current grid selection to the clipboard in `fmt`. Maps the selection
/// (page-local, over the search-filtered display rows) to page-`batch` row indices and
/// column indices, then hands both to `serialize::write_selection`. All formats carry a
/// header, so a copied selection is self-describing.
pub fn copy_selection(fmt: crate::engine::serialize::TextFormat) {
    let ws_id = crate::session::active_id();
    let Some(entry) = crate::runs::RUNS.resolve().get(ws_id) else {
        return;
    };
    let run = entry.peek();
    let (Some(sel), Some(result), Some(batch)) =
        (run.sel.clone(), run.result.as_ref(), run.page_batch.as_ref())
    else {
        return;
    };
    let search = run.result_search.to_lowercase();
    // Map each filtered *display* row index → its page-`batch` row index (the grid's
    // search filter is page-local; `result.rows` and the batch share row order).
    let filtered_to_batch: Vec<usize> = result
        .rows
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            search.is_empty() || r.iter().any(|c| c.text.to_lowercase().contains(&search))
        })
        .map(|(oi, _)| oi)
        .collect();
    let ncols = result.columns.len();
    let (mut frows, mut cols): (Vec<usize>, Vec<usize>) = match &sel {
        crate::runs::Selection::Cell { .. } => {
            let Some((minr, maxr, minc, maxc)) = sel.cell_bounds() else {
                return;
            };
            ((minr..=maxr).collect(), (minc..=maxc).collect())
        }
        crate::runs::Selection::Rows(rs) => (rs.clone(), (0..ncols).collect()),
        crate::runs::Selection::Cols(cs) => ((0..filtered_to_batch.len()).collect(), cs.clone()),
    };
    frows.sort_unstable();
    frows.dedup();
    frows.retain(|&r| r < filtered_to_batch.len());
    cols.sort_unstable();
    cols.dedup();
    cols.retain(|&c| c < ncols && c < batch.num_columns());
    if frows.is_empty() || cols.is_empty() {
        return;
    }
    let rows: Vec<u32> = frows.iter().map(|&i| filtered_to_batch[i] as u32).collect();
    write_to_clipboard(fmt, batch, &rows, &cols);
}

/// Rz5 — copy a single **record** (all columns of the page-local filtered row `row_idx`)
/// to the clipboard in `fmt`, from the record view's `⋯` menu. One row, every column.
pub fn copy_record(row_idx: usize, fmt: crate::engine::serialize::TextFormat) {
    let ws_id = crate::session::active_id();
    let Some(entry) = crate::runs::RUNS.resolve().get(ws_id) else {
        return;
    };
    let run = entry.peek();
    let (Some(result), Some(batch)) = (run.result.as_ref(), run.page_batch.as_ref()) else {
        return;
    };
    let search = run.result_search.to_lowercase();
    // The record index is into the *filtered* page; map it back to its batch row.
    let batch_row = result
        .rows
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            search.is_empty() || r.iter().any(|c| c.text.to_lowercase().contains(&search))
        })
        .nth(row_idx)
        .map(|(oi, _)| oi as u32);
    let Some(batch_row) = batch_row else { return };
    if batch_row as usize >= batch.num_rows() {
        return;
    }
    let cols: Vec<usize> = (0..batch.num_columns()).collect();
    write_to_clipboard(fmt, batch, &[batch_row], &cols);
}

/// Slice `rows`×`cols` out of `batch` onto the clipboard as `fmt`, with a header. The
/// arrow slicing is `serialize`'s job; this only wires the result to the clipboard.
fn write_to_clipboard(
    fmt: crate::engine::serialize::TextFormat,
    batch: &datafusion::arrow::record_batch::RecordBatch,
    rows: &[u32],
    cols: &[usize],
) {
    let mut clip = crate::engine::serialize::ClipboardWriter::new();
    let _ = crate::engine::serialize::write_selection(fmt, batch, rows, cols, true, &mut clip)
        .map_err(|e| e.to_string())
        .and_then(|_| clip.commit());
}
