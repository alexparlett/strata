//! Find-in-results (P2-09 / Dioxus U6): the per-run find state and the **page-local** filter
//! it applies — Dioxus parity, no engine traffic: the filter narrows the rows of the resolved
//! page in hand, while paging keeps walking the unfiltered snapshot. `ResultsBody` owns the
//! state (so it resets with every press, like the page number) and applies [`filter_page`] to
//! the page it resolved, so the grid *and* the status bar's selection aggregate see the same
//! filtered rows; the popover UI lives on the toolbar's Search button (`toolbar.rs`), and ⌘F /
//! Esc attach on the grid root (`datagrid`).

use std::rc::Rc;

use freya::prelude::*;

use super::datagrid::GridData;

/// The find popover's state for one settled Run: the open flag + the live query. Threaded as
/// **struct-field props** to the grid (⌘F / Esc dispatch) and its toolbar (trigger + popover)
/// — known shallow consumers.
#[derive(Clone, Copy, PartialEq)]
pub struct FindState {
    pub open: State<bool>,
    pub query: State<String>,
}

impl FindState {
    /// Hook: a fresh find — closed, empty query.
    pub fn use_new() -> Self {
        Self { open: use_state(|| false), query: use_state(String::new) }
    }

    /// Close the popover **and clear the query** — every dismissal path (backdrop, Esc, the
    /// ✕, the trigger's toggle-off) funnels here so a stale filter never lingers on the grid
    /// (the Dioxus `set_results_find` rule).
    pub fn dismiss(mut self) {
        self.open.set(false);
        self.query.set(String::new());
    }

    /// The trigger / ⌘F toggle: open when closed, [`dismiss`](Self::dismiss) when open.
    pub fn toggle(self) {
        if *self.open.peek() {
            self.dismiss();
        } else {
            let mut open = self.open;
            open.set(true);
        }
    }

    /// The normalized needle — trimmed + lowercased, `None` when that leaves nothing.
    /// Subscribes (`.read()`), so the caller re-filters on every keystroke.
    pub fn needle(&self) -> Option<String> {
        let q = self.query.read().trim().to_lowercase();
        (!q.is_empty()).then_some(q)
    }
}

/// The find filter's view of one resolved page: the (possibly narrowed) grid data and the
/// surviving rows' absolute gutter numbers (`None` when unfiltered — the grid then numbers by
/// position).
pub struct FindView {
    pub data: Rc<GridData>,
    pub row_nums: Option<Rc<Vec<usize>>>,
}

/// Filter one page down to the rows where **any** cell's display text contains the needle,
/// case-insensitively — the Dioxus row predicate. Surviving rows keep their original absolute
/// row numbers (`row_base` + page position + 1), so the gutter shows gaps rather than
/// renumbering. `None` (an empty/whitespace query) passes the page through untouched.
pub fn filter_page(needle: Option<&str>, data: &Rc<GridData>, row_base: usize) -> FindView {
    let Some(needle) = needle else {
        return FindView { data: data.clone(), row_nums: None };
    };
    let mut rows = Vec::new();
    let mut nums = Vec::new();
    for (i, row) in data.rows.iter().enumerate() {
        if row.iter().any(|c| c.text.to_lowercase().contains(needle)) {
            rows.push(row.clone());
            nums.push(row_base + i + 1);
        }
    }
    FindView {
        // The unfiltered page batch rides along untouched: survivors map back to it through
        // `row_nums` (see `cell_view::page_batch_row`).
        data: Rc::new(GridData::from_page(data.columns.clone(), rows, data.batch.clone())),
        row_nums: Some(Rc::new(nums)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use strata_core::engine::{RecordBatch, Schema};
    use strata_model::{Cell, ColumnInfo, Kind};

    use super::*;

    fn page() -> Rc<GridData> {
        let col = |name: &str| ColumnInfo {
            name: name.into(),
            dtype: "t".into(),
            kind: Kind::Str,
            nullable: true,
            children: Vec::new(),
            stats: Vec::new(),
        };
        let cell = |text: &str| Cell { text: text.into(), null: false };
        Rc::new(GridData {
            columns: vec![col("a"), col("b")],
            rows: vec![
                vec![cell("Alpha"), cell("x")],
                vec![cell("beta"), cell("y")],
                vec![cell("gamma"), cell("ALPHABET")],
            ],
            batch: RecordBatch::new_empty(Arc::new(Schema::empty())),
        })
    }

    #[test]
    fn no_needle_passes_the_page_through() {
        let data = page();
        let view = filter_page(None, &data, 100);
        assert!(Rc::ptr_eq(&view.data, &data));
        assert!(view.row_nums.is_none());
    }

    #[test]
    fn matches_any_cell_case_insensitively() {
        // "alpha" hits row 0 (col a, "Alpha") and row 2 (col b, "ALPHABET").
        let view = filter_page(Some("alpha"), &page(), 0);
        assert_eq!(view.data.rows.len(), 2);
        assert_eq!(view.data.rows[0][0].text, "Alpha");
        assert_eq!(view.data.rows[1][0].text, "gamma");
        // Schema rides along for the grid's type colouring.
        assert_eq!(view.data.columns.len(), 2);
    }

    #[test]
    fn survivors_keep_their_absolute_row_numbers() {
        // Page 2 of 100/page: rows 101..=103; "alpha" survives rows 101 and 103.
        let view = filter_page(Some("alpha"), &page(), 100);
        assert_eq!(view.row_nums.as_deref(), Some(&vec![101, 103]));
    }

    #[test]
    fn no_matches_is_an_empty_page() {
        let view = filter_page(Some("zzz"), &page(), 0);
        assert!(view.data.rows.is_empty());
        assert_eq!(view.row_nums.as_deref(), Some(&vec![]));
    }
}
