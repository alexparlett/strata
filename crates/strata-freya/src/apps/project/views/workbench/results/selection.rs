//! Datagrid **selection** — the page-local cell / row / column model and its per-cell styling, ported
//! from the Dioxus `grid/selection.rs`. Pure data + a `Copy` controller ([`SelCtl`]) over shared
//! `State`s: the datagrid cells call the controller on pointer events and read the computed
//! [`SelStyle`] to paint. Freya pointer events carry no modifiers, so shift / ⌘ are tracked separately
//! (global key up/down) and read here.

use freya::prelude::*;

/// Page-local selection: a cell rectangle (anchor → focus), whole rows, or whole columns.
#[derive(Clone, PartialEq, Eq)]
pub enum Selection {
    None,
    /// Rectangle from anchor `(ar, ac)` to focus `(fr, fc)`.
    Cell { ar: usize, ac: usize, fr: usize, fc: usize },
    /// Whole rows, by row index.
    Rows(Vec<usize>),
    /// Whole columns, by column index.
    Cols(Vec<usize>),
}

impl Selection {
    /// Inclusive `(min_row, max_row, min_col, max_col)` of a `Cell` rectangle.
    pub fn cell_bounds(&self) -> Option<(usize, usize, usize, usize)> {
        match self {
            Selection::Cell { ar, ac, fr, fc } => {
                Some(((*ar).min(*fr), (*ar).max(*fr), (*ac).min(*fc), (*ac).max(*fc)))
            }
            _ => None,
        }
    }

    /// The rows of a `Rows` selection (else empty).
    pub fn rows(&self) -> &[usize] {
        match self {
            Selection::Rows(v) => v,
            _ => &[],
        }
    }

    /// The columns of a `Cols` selection (else empty).
    pub fn cols(&self) -> &[usize] {
        match self {
            Selection::Cols(v) => v,
            _ => &[],
        }
    }
}

/// Per-cell selection styling: whether the cell is filled, and which of its four outer edges carry the
/// 2px accent ring (a single focused cell rings all four).
#[derive(Clone, Copy, PartialEq, Default)]
pub struct SelStyle {
    pub top: bool,
    pub bot: bool,
    pub left: bool,
    pub right: bool,
}

/// The selection style for body cell `(r, c)` — a port of the Dioxus `cell_sel_style`. A column / row
/// selection tints the whole column / row (edges on the block's outer sides); a cell rectangle rings
/// its perimeter (all four edges for a single cell). `wrapping_sub(1)` at index 0 yields `usize::MAX`,
/// which is never in a set, so the outer edge lands correctly at 0.
pub fn cell_sel_style(
    bounds: Option<(usize, usize, usize, usize)>,
    rows: &[usize],
    cols: &[usize],
    r: usize,
    c: usize,
    last_row: usize,
    last_col: usize,
) -> SelStyle {
    if !cols.is_empty() {
        if cols.contains(&c) {
            SelStyle {
                top: r == 0,
                bot: r == last_row,
                left: !cols.contains(&c.wrapping_sub(1)),
                right: !cols.contains(&(c + 1)),
            }
        } else {
            SelStyle::default()
        }
    } else if !rows.is_empty() {
        if rows.contains(&r) {
            SelStyle {
                top: !rows.contains(&r.wrapping_sub(1)),
                bot: !rows.contains(&(r + 1)),
                left: c == 0,
                right: c == last_col,
            }
        } else {
            SelStyle::default()
        }
    } else if let Some((minr, maxr, minc, maxc)) = bounds {
        if r >= minr && r <= maxr && c >= minc && c <= maxc {
            SelStyle {
                top: r == minr,
                bot: r == maxr,
                left: c == minc,
                right: c == maxc,
            }
        } else {
            SelStyle::default()
        }
    } else {
        SelStyle::default()
    }
}

/// Which selection interaction a [`Cell`](super::datagrid) drives on primary mousedown.
#[derive(Clone, Copy, PartialEq)]
pub enum CellRole {
    /// A body data cell `(row, col)` — starts / paints a rectangle.
    Data(usize, usize),
    /// A row-number gutter at `row` — whole-row selection.
    Row(usize),
    /// The `#` corner — select-all.
    Corner,
    /// No selection interaction (e.g. the trailing filler).
    None,
}

/// Shared selection state + the pointer / keyboard mutators — the Dioxus `sel_*` fns as methods on a
/// `Copy` handle. `nrows` / `ncols` are the grid dimensions (for select-all).
#[derive(Clone, Copy, PartialEq)]
pub struct SelCtl {
    pub sel: State<Selection>,
    pub anchor: State<Option<usize>>,
    pub drag: State<bool>,
    pub shift: State<bool>,
    pub meta: State<bool>,
    pub nrows: usize,
    pub ncols: usize,
}

impl SelCtl {
    /// Primary mousedown on a body cell: start a rectangle (or extend it if shift is held), and begin a
    /// drag-paint.
    pub fn cell_down(mut self, r: usize, c: usize) {
        if *self.shift.peek() {
            self.cell_to(r, c);
        } else {
            self.sel.set(Selection::Cell { ar: r, ac: c, fr: r, fc: c });
        }
        self.drag.set(true);
    }

    /// Move the rectangle's focus corner (shift-click / drag), starting one if absent.
    pub fn cell_to(mut self, r: usize, c: usize) {
        let (ar, ac) = match &*self.sel.peek() {
            Selection::Cell { ar, ac, .. } => (*ar, *ac),
            _ => (r, c),
        };
        self.sel.set(Selection::Cell { ar, ac, fr: r, fc: c });
    }

    /// Drag-paint: extend the rectangle to `(r, c)` while a drag is active.
    pub fn cell_paint(self, r: usize, c: usize) {
        if *self.drag.peek() {
            self.cell_to(r, c);
        }
    }

    /// Select row `i` (gutter click): plain = only it, ⌘/Ctrl = toggle, shift = range from the anchor.
    pub fn row(mut self, i: usize) {
        let add = *self.meta.peek();
        let extend = *self.shift.peek();
        let is_rows = matches!(&*self.sel.peek(), Selection::Rows(_));
        if extend {
            if let (true, Some(a)) = (is_rows, *self.anchor.peek()) {
                let (lo, hi) = if a <= i { (a, i) } else { (i, a) };
                self.sel.set(Selection::Rows((lo..=hi).collect()));
                return;
            }
        }
        if add {
            let mut rows = match &*self.sel.peek() {
                Selection::Rows(v) => v.clone(),
                _ => Vec::new(),
            };
            match rows.iter().position(|&x| x == i) {
                Some(p) => {
                    rows.remove(p);
                }
                None => rows.push(i),
            }
            self.sel.set((!rows.is_empty()).then_some(Selection::Rows(rows)).unwrap_or(Selection::None));
        } else {
            self.sel.set(Selection::Rows(vec![i]));
        }
        self.anchor.set(Some(i));
    }

    /// Select column `ci` (header click): plain / ⌘-toggle / shift-range, mirroring [`row`](Self::row).
    pub fn col(mut self, ci: usize) {
        let add = *self.meta.peek();
        let extend = *self.shift.peek();
        let is_cols = matches!(&*self.sel.peek(), Selection::Cols(_));
        if extend {
            if let (true, Some(a)) = (is_cols, *self.anchor.peek()) {
                let (lo, hi) = if a <= ci { (a, ci) } else { (ci, a) };
                self.sel.set(Selection::Cols((lo..=hi).collect()));
                return;
            }
        }
        if add {
            let mut cols = match &*self.sel.peek() {
                Selection::Cols(v) => v.clone(),
                _ => Vec::new(),
            };
            match cols.iter().position(|&x| x == ci) {
                Some(p) => {
                    cols.remove(p);
                }
                None => cols.push(ci),
            }
            self.sel.set((!cols.is_empty()).then_some(Selection::Cols(cols)).unwrap_or(Selection::None));
        } else {
            self.sel.set(Selection::Cols(vec![ci]));
        }
        self.anchor.set(Some(ci));
    }

    /// Select every cell (the `#` corner).
    pub fn all(mut self) {
        if self.nrows > 0 && self.ncols > 0 {
            self.sel.set(Selection::Cell {
                ar: 0,
                ac: 0,
                fr: self.nrows - 1,
                fc: self.ncols - 1,
            });
        }
    }

    /// Clear the selection (click-off / Esc).
    pub fn clear(mut self) {
        self.sel.set(Selection::None);
        self.anchor.set(None);
    }

    /// End a drag-paint (pointer released anywhere).
    pub fn end_drag(mut self) {
        self.drag.set(false);
    }
}
