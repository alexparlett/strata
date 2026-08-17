//! Find-in-results (P2-09 / Dioxus U6): the per-run find state and the **page-local** filter
//! it applies — Dioxus parity, no engine traffic: the filter narrows the rows of the resolved
//! page in hand, while paging keeps walking the unfiltered snapshot. `ResultsBody` owns the
//! state (so it resets with every press, like the page number) and takes its view of the page
//! it resolved through the [`PageMemo`], so the grid *and* the status bar's selection
//! aggregate see the same filtered rows; the popover UI lives on the toolbar's Search button
//! (`toolbar.rs`), and ⌘F / Esc attach on the grid root (`datagrid`).
//!
//! Both O(page) steps — resolving a page and filtering it — run behind that memo, and the
//! match itself never allocates (see [`contains_lowercased`]): the filter sits on the
//! keystroke path of a box that can be scanning 1000 rows × N columns.

use std::cell::RefCell;
use std::rc::Rc;

use freya::prelude::*;
use strata_core::util::contains_lowercased;

use super::datagrid::GridData;
use crate::apps::project::query::PageSpec;

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
        Self {
            open: use_state(|| false),
            query: use_state(String::new),
        }
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
/// position). Cloning is two `Rc` bumps — the [`PageMemo`] hands the same view back to every
/// render that didn't change the filter.
#[derive(Clone)]
pub struct FindView {
    pub data: Rc<GridData>,
    pub row_nums: Option<Rc<Vec<usize>>>,
}

/// Filter one page down to the rows where **any** cell's display text contains the needle,
/// case-insensitively — the Dioxus row predicate. Surviving rows keep their original absolute
/// row numbers (`row_base` + page position + 1), so the gutter shows gaps rather than
/// renumbering. `None` (an empty/whitespace query) passes the page through untouched.
///
/// Scanning is allocation-free ([`contains_lowercased`]); the surviving rows are cloned into
/// the narrowed page, which is why this runs behind the [`PageMemo`] rather than on every
/// render of the results body.
pub fn filter_page(needle: Option<&str>, data: &Rc<GridData>, row_base: usize) -> FindView {
    let Some(needle) = needle else {
        return FindView {
            data: data.clone(),
            row_nums: None,
        };
    };
    let mut rows = Vec::new();
    let mut nums = Vec::new();
    for (i, row) in data.rows.iter().enumerate() {
        if row.iter().any(|c| contains_lowercased(&c.text, needle)) {
            rows.push(row.clone());
            nums.push(row_base + i + 1);
        }
    }
    FindView {
        data: Rc::new(GridData::from_page(
            data.columns.clone(),
            rows,
            data.batch.clone(),
        )),
        row_nums: Some(Rc::new(nums)),
    }
}

/// What defines the rows in hand — the [`PageMemo`]'s cache key.
///
/// A page's content is a pure function of *which cut of which snapshot* it is: the Run's own
/// page 1 rides in the settled outcome and cannot change for the life of a press (the results
/// body is keyed by the press's nonce), and every other cut is a read of an **immutable**
/// snapshot, identified by the very [`PageSpec`] that read is cached under. Nothing else moves
/// the rows, so this is the whole key.
#[derive(Clone, PartialEq)]
pub enum PageKey {
    /// The Run's own page 1 — page 1 at the Run's page size, unsorted.
    Run,
    /// A read of the snapshot: `(snapshot, page, page size, sort)`.
    Snapshot(PageSpec),
}

/// The results body's page memo: the page in hand, and the find view over it, each rebuilt
/// only when *its own* inputs change.
///
/// Why memoize at all: `ResultsBody` re-renders for plenty of reasons that don't move the rows
/// (the Table/Chart toggle, the tab's request channel, the pager's own state), and both steps
/// are O(page) — resolving a page clones every cell of it, filtering scans every cell of it.
/// At the 1000-row page size the pager offers, paying either per render is felt.
///
/// Why not Freya's `use_memo`: it recomputes *asynchronously*, off the `State`s read inside its
/// closure. The page here is derived during render from freya-query readers, and a page
/// captured in a closure would go stale exactly like a `VirtualScrollView` builder's captures.
/// So this is a plain synchronous single-entry cache, consulted where the page is resolved.
#[derive(Clone)]
pub struct PageMemo(Rc<RefCell<MemoInner>>);

#[derive(Default)]
struct MemoInner {
    /// The Run's own page 1, built on first use — the settled outcome behind it is fixed for
    /// the life of the press.
    run: Option<Rc<GridData>>,
    /// The page last resolved, and the find view last taken of it.
    page: Option<Entry>,
}

struct Entry {
    key: PageKey,
    data: Rc<GridData>,
    /// The needle + row base that produced `view` from `data`.
    needle: Option<String>,
    row_base: usize,
    view: FindView,
}

/// Hook: this press's page memo. One per mounted results body — which is keyed by the press's
/// nonce, so a new Run starts with an empty one.
pub fn use_page_memo() -> PageMemo {
    use_hook(|| PageMemo(Rc::new(RefCell::new(MemoInner::default()))))
}

impl PageMemo {
    /// The Run's own page 1, built once: it is the settled outcome's rows, which cannot change
    /// while this body lives.
    pub fn run_page(&self, build: impl FnOnce() -> Rc<GridData>) -> Rc<GridData> {
        self.0.borrow_mut().run.get_or_insert_with(build).clone()
    }

    /// The find view of the page `key` identifies: `build` runs only when the cut changed,
    /// [`filter_page`] only when the cut, the needle or the row base did.
    pub fn view(
        &self,
        key: PageKey,
        build: impl FnOnce() -> Rc<GridData>,
        needle: Option<&str>,
        row_base: usize,
    ) -> FindView {
        let mut inner = self.0.borrow_mut();
        let entry = match inner.page.take() {
            Some(mut e) if e.key == key => {
                if e.needle.as_deref() != needle || e.row_base != row_base {
                    e.view = filter_page(needle, &e.data, row_base);
                    e.needle = needle.map(str::to_owned);
                    e.row_base = row_base;
                }
                e
            }
            _ => {
                let data = build();
                let view = filter_page(needle, &data, row_base);
                Entry {
                    key,
                    data,
                    needle: needle.map(str::to_owned),
                    row_base,
                    view,
                }
            }
        };
        let view = entry.view.clone();
        inner.page = Some(entry);
        view
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::arrow::datatypes::{DataType, Field};
    use strata_arrow::{column_info, RecordBatch, Schema};
    use strata_model::{Cell, SnapshotId};

    use super::*;

    fn page() -> Rc<GridData> {
        let col = |name: &str| column_info(&Field::new(name, DataType::Utf8, true));
        let cell = |text: &str| Cell {
            text: text.into(),
            null: false,
        };
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
        let view = filter_page(Some("alpha"), &page(), 0);
        assert_eq!(view.data.rows.len(), 2);
        assert_eq!(view.data.rows[0][0].text, "Alpha");
        assert_eq!(view.data.rows[1][0].text, "gamma");
        assert_eq!(view.data.columns.len(), 2);
    }

    #[test]
    fn survivors_keep_their_absolute_row_numbers() {
        let view = filter_page(Some("alpha"), &page(), 100);
        assert_eq!(view.row_nums.as_deref(), Some(&vec![101, 103]));
    }

    #[test]
    fn no_matches_is_an_empty_page() {
        let view = filter_page(Some("zzz"), &page(), 0);
        assert!(view.data.rows.is_empty());
        assert_eq!(view.row_nums.as_deref(), Some(&vec![]));
    }

    /// The predicate's Unicode edge cases live with it in `strata_core::util`; what belongs
    /// here is that the *filter* sees a match through them — a needle beginning inside the
    /// two-char lowercase expansion of 'İ' still narrows the page.
    #[test]
    fn the_filter_sees_a_mid_expansion_match() {
        let col = column_info(&Field::new("a", DataType::Utf8, true));
        let data = Rc::new(GridData {
            columns: vec![col],
            rows: vec![
                vec![Cell {
                    text: "İstanbul".into(),
                    null: false,
                }],
                vec![Cell {
                    text: "Ankara".into(),
                    null: false,
                }],
            ],
            batch: RecordBatch::new_empty(Arc::new(Schema::empty())),
        });
        let view = filter_page(Some("\u{307}stanbul"), &data, 0);
        assert_eq!(view.row_nums.as_deref(), Some(&vec![1]));
    }

    #[test]
    fn the_filter_narrows_non_ascii_rows() {
        let col = column_info(&Field::new("a", DataType::Utf8, true));
        let cell = |text: &str| Cell {
            text: text.into(),
            null: false,
        };
        let data = Rc::new(GridData {
            columns: vec![col],
            rows: vec![
                vec![cell("CAFÉ")],
                vec![cell("cafe")],
                vec![cell("Café Noir")],
            ],
            batch: RecordBatch::new_empty(Arc::new(Schema::empty())),
        });
        let view = filter_page(Some("café"), &data, 0);
        assert_eq!(view.data.rows.len(), 2);
        assert_eq!(view.row_nums.as_deref(), Some(&vec![1, 3]));
    }

    fn memo() -> PageMemo {
        PageMemo(Rc::new(RefCell::new(MemoInner::default())))
    }

    fn spec(page: usize) -> PageSpec {
        PageSpec {
            snapshot: SnapshotId(1),
            page,
            page_size: 100,
            sort: None,
        }
    }

    #[test]
    fn the_memo_resolves_a_page_once_per_cut() {
        let memo = memo();
        let builds = std::cell::Cell::new(0);
        let build = || {
            builds.set(builds.get() + 1);
            page()
        };

        let first = memo.view(PageKey::Run, build, None, 0);
        assert_eq!(builds.get(), 1);
        let again = memo.view(PageKey::Run, build, None, 0);
        assert_eq!(builds.get(), 1);
        assert!(Rc::ptr_eq(&first.data, &again.data));

        memo.view(PageKey::Snapshot(spec(2)), build, None, 100);
        assert_eq!(builds.get(), 2);
        memo.view(PageKey::Run, build, None, 0);
        assert_eq!(builds.get(), 3);
    }

    #[test]
    fn the_memo_refilters_the_page_it_already_has() {
        let memo = memo();
        let builds = std::cell::Cell::new(0);
        let build = || {
            builds.set(builds.get() + 1);
            page()
        };

        let all = memo.view(PageKey::Run, build, None, 0);
        assert_eq!(all.data.rows.len(), 3);
        let hit = memo.view(PageKey::Run, build, Some("alpha"), 0);
        assert_eq!(builds.get(), 1);
        assert_eq!(hit.data.rows.len(), 2);
        assert_eq!(hit.row_nums.as_deref(), Some(&vec![1, 3]));
        let rebased = memo.view(PageKey::Run, build, Some("alpha"), 100);
        assert_eq!(builds.get(), 1);
        assert_eq!(rebased.row_nums.as_deref(), Some(&vec![101, 103]));
    }

    #[test]
    fn the_memo_builds_the_run_page_once() {
        let memo = memo();
        let builds = std::cell::Cell::new(0);
        let build = || {
            builds.set(builds.get() + 1);
            page()
        };
        let first = memo.run_page(build);
        let again = memo.run_page(build);
        assert_eq!(builds.get(), 1);
        assert!(Rc::ptr_eq(&first, &again));
    }
}
