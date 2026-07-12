//! Grid **selection** state + geometry — the page-local cell/row/column selection model
//! (`sel_*`), ⌘A `select_all_active_grid`, column auto-fit, and the per-cell selection tint /
//! edge-border styling (`cell_sel_style`). Pure logic over `crate::runs` (no UI). Split out of
//! the grid module; the `PrettyJsonWriter`-style press-target flag stays in the parent.

use std::collections::HashSet;

use crate::runs::Selection;
use crate::session::WorkspaceId;

pub(super) fn sel_cell_start(ws: WorkspaceId, r: usize, c: usize) {
    crate::runs::edit(ws, |run| {
        run.sel = Some(Selection::Cell {
            ar: r,
            ac: c,
            fr: r,
            fc: c,
        })
    });
}

/// Move the focus corner of a cell rectangle (drag / ⇧-click), starting one if absent.
pub(super) fn sel_cell_to(ws: WorkspaceId, r: usize, c: usize) {
    crate::runs::edit(ws, |run| {
        let (ar, ac) = match run.sel {
            Some(Selection::Cell { ar, ac, .. }) => (ar, ac),
            _ => (r, c),
        };
        run.sel = Some(Selection::Cell {
            ar,
            ac,
            fr: r,
            fc: c,
        });
    });
}

/// Select row `i`, Excel-style. Plain click selects only it; `add` (⌘/Ctrl-click) toggles it
/// in/out of the multi-selection (this is how one row is removed from a group); `extend`
/// (shift-click) selects the contiguous range from the anchor to `i`.
pub(super) fn sel_row(ws: WorkspaceId, i: usize, add: bool, extend: bool) {
    crate::runs::edit(ws, |run| {
        let is_rows = matches!(run.sel, Some(Selection::Rows(_)));
        // Shift-click → contiguous range from the anchor (only within an existing row
        // selection; otherwise it falls through to a plain select). The anchor is left where
        // it was so successive shift-clicks re-extend from the same point.
        if extend {
            if let (true, Some(a)) = (is_rows, run.sel_anchor) {
                let (lo, hi) = if a <= i { (a, i) } else { (i, a) };
                run.sel = Some(Selection::Rows((lo..=hi).collect()));
                return;
            }
        }
        if add {
            let mut rows = match &run.sel {
                Some(Selection::Rows(v)) => v.clone(),
                _ => Vec::new(),
            };
            match rows.iter().position(|&x| x == i) {
                Some(p) => {
                    rows.remove(p);
                }
                None => rows.push(i),
            }
            run.sel = (!rows.is_empty()).then_some(Selection::Rows(rows));
        } else {
            run.sel = Some(Selection::Rows(vec![i]));
        }
        run.sel_anchor = Some(i);
    });
}

/// Select column `ci`, Excel-style. Plain click selects only it; `add` (⌘/Ctrl-click) toggles
/// it in/out of the multi-selection; `extend` (shift-click) selects the contiguous range
/// from the anchor to `ci`.
pub(super) fn sel_col(ws: WorkspaceId, ci: usize, add: bool, extend: bool) {
    crate::runs::edit(ws, |run| {
        let is_cols = matches!(run.sel, Some(Selection::Cols(_)));
        if extend {
            if let (true, Some(a)) = (is_cols, run.sel_anchor) {
                let (lo, hi) = if a <= ci { (a, ci) } else { (ci, a) };
                run.sel = Some(Selection::Cols((lo..=hi).collect()));
                return;
            }
        }
        if add {
            let mut cols = match &run.sel {
                Some(Selection::Cols(v)) => v.clone(),
                _ => Vec::new(),
            };
            match cols.iter().position(|&x| x == ci) {
                Some(p) => {
                    cols.remove(p);
                }
                None => cols.push(ci),
            }
            run.sel = (!cols.is_empty()).then_some(Selection::Cols(cols));
        } else {
            run.sel = Some(Selection::Cols(vec![ci]));
        }
        run.sel_anchor = Some(ci);
    });
}

/// Select every cell on the active tab's current result page. The Edit menu routes ⌘A
/// here when the grid holds the Select All scope; dims are recomputed from the run (the
/// menu handler has no component scope) to match the grid's page-local filtering.
pub(crate) fn select_all_active_grid() {
    let ws = crate::session::active_id();
    if ws == 0 {
        return;
    }
    crate::runs::edit_existing(ws, |run| {
        let search = run.result_search.to_lowercase();
        let dims = run.result.as_ref().map(|result| {
            let nrows = result
                .rows
                .iter()
                .filter(|r| {
                    search.is_empty() || r.iter().any(|c| c.text.to_lowercase().contains(&search))
                })
                .count();
            (nrows, result.columns.len())
        });
        if let Some((nrows, ncols)) = dims {
            if nrows > 0 && ncols > 0 {
                run.sel = Some(Selection::Cell {
                    ar: 0,
                    ac: 0,
                    fr: nrows - 1,
                    fc: ncols - 1,
                });
            }
        }
    });
}

pub(super) fn sel_clear(ws: WorkspaceId) {
    crate::runs::edit(ws, |run| {
        run.sel = None;
        run.sel_anchor = None;
    });
}

/// Select every cell on the page (the `#` corner header, spreadsheet select-all). Uses the
/// grid's already-computed `nrows`/`ncols` so it matches exactly what's rendered. Deselect
/// is via clicking off / Esc — matching common spreadsheets.
pub(super) fn sel_all_cells(ws: WorkspaceId, nrows: usize, ncols: usize) {
    if nrows > 0 && ncols > 0 {
        crate::runs::edit(ws, |run| {
            run.sel = Some(Selection::Cell {
                ar: 0,
                ac: 0,
                fr: nrows - 1,
                fc: ncols - 1,
            });
        });
    }
}

// ── column resize (V20) ──

/// Auto-fit column `ci` to the widest value on the current page (grip double-click):
/// `width = clamp(maxLen*7.6 + 28, 64, 520)`, where `maxLen` is the longest of the header
/// name (+3 for the affordance) and each page row's cell text. A char-count estimate — no
/// off-thread text metrics available — matching the V20 prototype.
pub(super) fn col_autofit(ws: WorkspaceId, ci: usize) {
    crate::runs::edit(ws, |run| {
        let w = run.result.as_ref().and_then(|result| {
            let col = result.columns.get(ci)?;
            let mut max_len = col.name.chars().count() + 3;
            for row in &result.rows {
                if let Some(cell) = row.get(ci) {
                    max_len = max_len.max(cell.text.chars().count());
                }
            }
            Some((max_len as f64 * 7.6 + 28.0).clamp(64.0, 520.0))
        });
        if let Some(w) = w {
            run.col_widths.insert(ci, w);
        }
    });
}

// ── selection rendering ──

/// Accent inset box-shadow *value* for whichever outer edges of the selection region this
/// cell sits on (a 2px border around the rectangle; full ring for a single focused cell).
/// Returns `none` when off every edge, so the property is always set explicitly.
fn edge_shadow(top: bool, bot: bool, left: bool, right: bool, focus: bool) -> String {
    let mut sh: Vec<&str> = Vec::new();
    if top {
        sh.push("inset 0 2px 0 var(--accent)");
    }
    if bot {
        sh.push("inset 0 -2px 0 var(--accent)");
    }
    if left {
        sh.push("inset 2px 0 0 var(--accent)");
    }
    if right {
        sh.push("inset -2px 0 0 var(--accent)");
    }
    if focus {
        sh.push("inset 0 0 0 2px var(--accent)");
    }
    if sh.is_empty() {
        "none".to_string()
    } else {
        sh.join(",")
    }
}

/// The inline selection style for cell `(i, ci)`. **Always** emits `background` +
/// `box-shadow` (transparent / none when unselected): Dioxus's per-property style diffing
/// doesn't clear a property that merely vanishes from the string, so a lingering edge
/// highlight would otherwise stay after deselect.
pub(super) fn cell_sel_style(
    bounds: &Option<(usize, usize, usize, usize)>,
    rows: &HashSet<usize>,
    cols: &HashSet<usize>,
    i: usize,
    ci: usize,
    last_row: usize,
    last_col: usize,
) -> String {
    // `checked_sub(1).unwrap_or(MAX)` → the neighbour "before index 0" is never in a set, so
    // the outer edge lands correctly at index 0.
    let (selected, shadow) = if !cols.is_empty() {
        if cols.contains(&ci) {
            (
                true,
                edge_shadow(
                    i == 0,
                    i == last_row,
                    !cols.contains(&ci.checked_sub(1).unwrap_or(usize::MAX)),
                    !cols.contains(&(ci + 1)),
                    false,
                ),
            )
        } else {
            (false, "none".to_string())
        }
    } else if !rows.is_empty() {
        if rows.contains(&i) {
            (
                true,
                edge_shadow(
                    !rows.contains(&i.checked_sub(1).unwrap_or(usize::MAX)),
                    !rows.contains(&(i + 1)),
                    ci == 0,
                    ci == last_col,
                    false,
                ),
            )
        } else {
            (false, "none".to_string())
        }
    } else if let Some((minr, maxr, minc, maxc)) = *bounds {
        if i >= minr && i <= maxr && ci >= minc && ci <= maxc {
            let single = minr == maxr && minc == maxc;
            (
                true,
                edge_shadow(i == minr, i == maxr, ci == minc, ci == maxc, single),
            )
        } else {
            (false, "none".to_string())
        }
    } else {
        (false, "none".to_string())
    };
    let bg = if selected { "var(--c-sel)" } else { "transparent" };
    format!("background:{bg};box-shadow:{shadow};")
}

