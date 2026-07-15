//! Clipboard copy — grid selection (Rz4) + single record (Rz5) — through crate::serialize.

use dioxus::prelude::*;

use crate::state::AppState;

/// Rz4 — copy the current grid selection to the clipboard: project the selected columns + take
/// the selected rows into a sub-`RecordBatch`, then serialize it via `crate::serialize`. TSV
/// (the ⌘C default) and the other formats all carry a header; indices are page-local.
pub fn copy_selection(_state: Signal<AppState>, fmt: crate::serialize::TextFormat) {
    use datafusion::arrow::array::{ArrayRef, RecordBatch, UInt32Array};

    let ws_id = crate::session::active_id();
    let sub: Option<(RecordBatch, usize, usize)> =
        crate::runs::RUNS.resolve().get(ws_id).and_then(|e| {
            let run = e.peek();
            let sel = run.sel.clone()?;
            let result = run.result.as_ref()?;
            let batch = run.page_batch.as_ref()?;
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
                    let (minr, maxr, minc, maxc) = sel.cell_bounds()?;
                    ((minr..=maxr).collect(), (minc..=maxc).collect())
                }
                crate::runs::Selection::Rows(rs) => (rs.clone(), (0..ncols).collect()),
                crate::runs::Selection::Cols(cs) => {
                    ((0..filtered_to_batch.len()).collect(), cs.clone())
                }
            };
            frows.sort_unstable();
            frows.dedup();
            frows.retain(|&r| r < filtered_to_batch.len());
            cols.sort_unstable();
            cols.dedup();
            cols.retain(|&c| c < ncols && c < batch.num_columns());
            if frows.is_empty() || cols.is_empty() {
                return None;
            }
            let batch_rows: Vec<u32> = frows.iter().map(|&i| filtered_to_batch[i] as u32).collect();
            let projected = batch.project(&cols).ok()?;
            let indices = UInt32Array::from(batch_rows);
            let taken: Vec<ArrayRef> = projected
                .columns()
                .iter()
                .map(|c| datafusion::arrow::compute::take(&**c, &indices, None))
                .collect::<Result<_, _>>()
                .ok()?;
            let sub = RecordBatch::try_new(projected.schema(), taken).ok()?;
            Some((sub, frows.len(), cols.len()))
        });

    if let Some((batch, _, _)) = sub {
        // All formats carry a header row / keys — consistent across TSV/CSV/JSON/Markdown so
        // a copied selection is always self-describing.
        let header = true;
        let mut clip = crate::serialize::ClipboardWriter::new();
        let _ = crate::serialize::write_batch(fmt, &batch, header, &mut clip)
            .map_err(|e| e.to_string())
            .and_then(|_| clip.commit());
    }
}

/// Rz5 — copy a single **record** (all columns of the page-local filtered row `row_idx`) to the
/// clipboard in `fmt`, from the record view's `⋯` menu. Like [`copy_selection`] but one full row:
/// map the filtered display index → page-`batch` row, `take` it into a one-row `RecordBatch`, and
/// serialize with a header.
pub fn copy_record(_state: Signal<AppState>, row_idx: usize, fmt: crate::serialize::TextFormat) {
    use datafusion::arrow::array::{ArrayRef, RecordBatch, UInt32Array};

    let ws_id = crate::session::active_id();
    let sub: Option<RecordBatch> = crate::runs::RUNS.resolve().get(ws_id).and_then(|e| {
        let run = e.peek();
        let result = run.result.as_ref()?;
        let batch = run.page_batch.as_ref()?;
        let search = run.result_search.to_lowercase();
        // The record index is into the *filtered* page; map it back to its batch row (the grid's
        // find-box filter is page-local, and `result.rows` shares the batch's row order).
        let batch_row = result
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                search.is_empty() || r.iter().any(|c| c.text.to_lowercase().contains(&search))
            })
            .nth(row_idx)
            .map(|(oi, _)| oi as u32)?;
        if batch_row as usize >= batch.num_rows() {
            return None;
        }
        let indices = UInt32Array::from(vec![batch_row]);
        let taken: Vec<ArrayRef> = batch
            .columns()
            .iter()
            .map(|c| datafusion::arrow::compute::take(&**c, &indices, None))
            .collect::<Result<_, _>>()
            .ok()?;
        RecordBatch::try_new(batch.schema(), taken).ok()
    });

    if let Some(batch) = sub {
        let mut clip = crate::serialize::ClipboardWriter::new();
        let _ = crate::serialize::write_batch(fmt, &batch, true, &mut clip)
            .map_err(|e| e.to_string())
            .and_then(|_| clip.commit());
    }
}
