//! Column sort (P2-13 / Dioxus Rz6): the per-run sort intent the header chevrons cycle.
//! `ResultsBody` owns the state (so it resets with every press, like the page number and the
//! find state) and folds it into the snapshot read: the engine applies `ORDER BY` over the
//! **whole snapshot** at page-read (nulls last, real Arrow-type ordering — `PageSpec.sort` is
//! part of the cache key, so a revisited direction settles from cache). The chevron UI lives
//! on the header cells (`datagrid/header.rs`).

use freya::prelude::*;

use super::selection::Selection;

/// The sort intent for one settled Run, threaded as **struct-field props** to the grid's
/// header cells — known shallow consumers. Column identity is the schema **index** (the
/// header's own key — names can collide across `SELECT` aliases); the results pane resolves
/// it to the column *name* only at the engine boundary.
#[derive(Clone, Copy, PartialEq)]
pub struct SortState {
    /// `(column index, ascending)` — `None` = snapshot order.
    pub by: State<Option<(usize, bool)>>,
    /// The pane's 1-based page: a sort re-orders the whole snapshot, so every cycle jumps
    /// back to page 1 (the old page number indexes a different cut).
    page: State<usize>,
    /// The page-local selection: a re-sort reshuffles the rows under it — the old indices
    /// would silently point at *different* cells (the pager-jump invariant), so cycling
    /// clears it.
    sel: State<Selection>,
}

impl SortState {
    /// Hook: a fresh intent — unsorted.
    pub fn use_new(page: State<usize>, sel: State<Selection>) -> Self {
        Self { by: use_state(|| None), page, sel }
    }

    /// The chevron press for column `ci`: advance the cycle, clear the page-local selection,
    /// and jump back to page 1.
    pub fn cycle(self, ci: usize) {
        let mut by = self.by;
        let next = next(*by.peek(), ci);
        by.set(next);
        let mut sel = self.sel;
        if *sel.peek() != Selection::None {
            sel.set(Selection::None);
        }
        let mut page = self.page;
        if *page.peek() != 1 {
            page.set(1);
        }
    }

    /// Column `ci`'s direction if it is the sorted one — `Some(ascending)`. Subscribes
    /// (`.read()`), so a header cell re-renders its chevron on any cycle.
    pub fn dir(&self, ci: usize) -> Option<bool> {
        (*self.by.read()).and_then(|(c, asc)| (c == ci).then_some(asc))
    }
}

/// The Rz6 cycle: unsorted → `ci` asc → `ci` desc → clear; a different column starts asc.
fn next(cur: Option<(usize, bool)>, ci: usize) -> Option<(usize, bool)> {
    match cur {
        Some((c, true)) if c == ci => Some((ci, false)),
        Some((c, false)) if c == ci => None,
        _ => Some((ci, true)),
    }
}

#[cfg(test)]
mod tests {
    use super::next;

    #[test]
    fn cycles_asc_desc_clear() {
        let s = next(None, 2);
        assert_eq!(s, Some((2, true)));
        let s = next(s, 2);
        assert_eq!(s, Some((2, false)));
        assert_eq!(next(s, 2), None);
    }

    #[test]
    fn another_column_restarts_at_asc() {
        assert_eq!(next(Some((2, true)), 5), Some((5, true)));
        assert_eq!(next(Some((2, false)), 5), Some((5, true)));
    }
}
