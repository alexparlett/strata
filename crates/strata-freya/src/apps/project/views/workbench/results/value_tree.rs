//! The nested-cell view's **tree model** (P2-25): which nodes are open, and the flat list of rows
//! that follows from it.
//!
//! Freya's `Tree` is virtualized over a flat list of visible rows, which is what lets this be lazy:
//! [`TreeModel::rows`] walks only the paths that are **open**, asking
//! [`value_tree::cell_children`](strata_core::engine::value_tree::cell_children) for each one's
//! children as it goes. A closed node costs nothing but the row that names it, so a 19,311-key
//! object left shut is one row, and opened is one paged read.
//!
//! Paging is part of the model rather than a control bolted on top. A container is shown
//! [`PAGE`] entries at a time and, when there are more, a **`… N more` row** takes the place of the
//! rest; pressing it raises that node's window by another page. The row exists because the
//! alternative is a tree that silently stops — and the count is free (`cell_len` reads an offset).

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use freya::components::Disclosure;
use strata_core::engine::value_tree::{cell_children, cell_len, cell_root, NodeValue, ValueNode};
use strata_core::engine::RecordBatch;
use strata_core::util::{fmt_int, plural_noun};

/// Entries revealed per press, and shown initially. Generous because a row is cheap once the tree
/// is virtualized — the cost of a wide container is the *read*, which is paged, not the rows.
pub const PAGE: usize = 100;

/// A node's address: the entry index at each level, from the cell down. Empty is the cell itself.
pub type Path = Vec<usize>;

/// One row on screen.
#[derive(Clone, PartialEq)]
pub struct TreeRow {
    pub path: Path,
    pub depth: usize,
    pub kind: RowKind,
}

#[derive(Clone, PartialEq)]
pub enum RowKind {
    /// A value: its key (or index, for a list item), type and what it holds.
    Node {
        node: ValueNode,
        disclosure: Disclosure,
    },
    /// The tail of a container showing only part of its entries: `… 19,211 more keys`. Pressing it
    /// widens that container's window by [`PAGE`].
    ///
    /// The container it belongs to is the row's own [`TreeRow::path`] — a tail sits *at* its
    /// container's path rather than under it, so a second copy of that path would be one more
    /// thing to keep in step.
    More { left: usize, label: String },
}

/// What the cell view holds while it is open: the batch the modal was opened on, plus which nodes
/// are expanded and how far each is paged.
///
/// The **batch is carried, not the grid**, which keeps P2-12's rule that the modal is a snapshot: a
/// later filter or page flip cannot retarget it, because the arrays it reads are the ones it was
/// opened with. Cloning a `RecordBatch` is an `Arc` bump per column, so this is a handle, not a copy.
#[derive(Clone)]
pub struct TreeModel {
    pub batch: RecordBatch,
    pub col: usize,
    pub row: usize,
    /// Open nodes. The cell itself (the empty path) starts open, or the tree would show one shut row.
    open: Rc<Vec<Path>>,
    /// How many entries each container has revealed, when it is more than [`PAGE`].
    shown: Rc<HashMap<Path, usize>>,
}

/// Identity is the cell it reads plus what is open — never the rows, which are derived.
impl PartialEq for TreeModel {
    fn eq(&self, other: &Self) -> bool {
        self.col == other.col
            && self.row == other.row
            && self.open == other.open
            && self.shown == other.shown
            // By **identity**, as `GridData` compares its batch: clones of one batch share their
            // arrays, so a pointer compare says "the same data" without walking it. A column count
            // does not — two different batches of equal width compare equal, and since Freya skips
            // re-rendering a scope whose props are equal, a model repointed at such a batch would
            // keep showing the old one's rows. No call site reaches that today (the modal's
            // backdrop covers the grid, so the open slot cannot go `Some` → `Some`), which is
            // exactly why it should not rest on that staying true.
            && self.batch.num_columns() == other.batch.num_columns()
            && self
                .batch
                .columns()
                .iter()
                .zip(other.batch.columns())
                .all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

impl TreeModel {
    pub fn new(batch: RecordBatch, col: usize, row: usize) -> Self {
        Self {
            batch,
            col,
            row,
            open: Rc::new(vec![Vec::new()]),
            shown: Rc::new(HashMap::new()),
        }
    }

    pub fn is_open(&self, path: &Path) -> bool {
        self.open.contains(path)
    }

    /// Open or close a node. Closing **forgets how far it was paged**: reopening a container you
    /// scrolled deep into and finding it still 3,000 rows long is not what closing it asked for.
    pub fn toggle(&mut self, path: &Path) {
        let mut open = (*self.open).clone();
        if let Some(at) = open.iter().position(|p| p == path) {
            open.remove(at);
            let mut shown = (*self.shown).clone();
            shown.remove(path);
            self.shown = Rc::new(shown);
        } else {
            open.push(path.clone());
        }
        self.open = Rc::new(open);
    }

    /// Reveal another [`PAGE`] entries of `path`.
    pub fn reveal_more(&mut self, path: &Path) {
        let mut shown = (*self.shown).clone();
        let entry = shown.entry(path.clone()).or_insert(PAGE);
        *entry += PAGE;
        self.shown = Rc::new(shown);
    }

    fn window(&self, path: &Path) -> usize {
        self.shown.get(path).copied().unwrap_or(PAGE)
    }

    /// Every visible row, in order. Walks only what is open.
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut out = Vec::new();
        self.walk(&mut Vec::new(), 0, &mut out);
        out
    }

    fn walk(&self, path: &mut Path, depth: usize, out: &mut Vec<TreeRow>) {
        let window = self.window(path);
        let Some(children) = cell_children(&self.batch, self.col, self.row, path, 0, window) else {
            return;
        };
        // What this container's entries are called, taken from a child already in hand — a named
        // entry is a key, an anonymous one a list item. Matches the record view's sampled text, so
        // the two surfaces count the same things by the same names.
        let unit = match children.first().map(|c| c.key.is_some()) {
            Some(true) => "key",
            _ => "item",
        };
        let shown = children.len();
        for child in children {
            path.push(child.index);
            let openable = child.value.expandable();
            let open = openable && self.is_open(path);
            out.push(TreeRow {
                path: path.clone(),
                depth,
                kind: RowKind::Node {
                    disclosure: if openable {
                        Disclosure::from_expanded(open)
                    } else {
                        Disclosure::Leaf
                    },
                    node: child,
                },
            });
            if open {
                self.walk(path, depth + 1, out);
            }
            path.pop();
        }
        // Only measure the container once **it** filled its window — an offset read is cheap, but a
        // container smaller than one page cannot have a tail. `out` counts the whole tree, so it is
        // not the quantity that answers this.
        if shown >= window {
            if let Some(total) = cell_len(&self.batch, self.col, self.row, path) {
                if total > window {
                    let left = total - window;
                    out.push(TreeRow {
                        path: path.clone(),
                        depth,
                        kind: RowKind::More {
                            left,
                            label: format!(
                                "… {} more {}",
                                fmt_int(left as u64),
                                plural_noun(left, unit)
                            ),
                        },
                    });
                }
            }
        }
    }
}

/// A leaf's text for the row, and whether it is a null (which the row dims, as the grid does).
pub fn leaf_text(value: &NodeValue) -> Option<(&str, bool)> {
    match value {
        NodeValue::Leaf(text) => Some((text.as_str(), false)),
        NodeValue::Null => Some(("NULL", true)),
        NodeValue::Nest(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::arrow::array::{ArrayRef, Int32Array, ListArray, StringArray, StructArray};
    use datafusion::arrow::buffer::OffsetBuffer;
    use datafusion::arrow::datatypes::{DataType, Field, Fields, Schema};

    use super::*;

    /// `{ "attrs": { "plan": "pro", "tags": [0 .. n) } }`.
    fn batch(items: i32) -> RecordBatch {
        let item = Arc::new(Field::new("item", DataType::Int32, true));
        let tags = ListArray::new(
            item.clone(),
            OffsetBuffer::new(vec![0, items].into()),
            Arc::new(Int32Array::from((0..items).collect::<Vec<_>>())),
            None,
        );
        let fields = Fields::from(vec![
            Field::new("plan", DataType::Utf8, true),
            Field::new("tags", DataType::List(item), true),
        ]);
        let attrs = StructArray::new(
            fields.clone(),
            vec![
                Arc::new(StringArray::from(vec!["pro"])) as ArrayRef,
                Arc::new(tags) as ArrayRef,
            ],
            None,
        );
        let schema = Schema::new(vec![Field::new("attrs", DataType::Struct(fields), true)]);
        RecordBatch::try_new(Arc::new(schema), vec![Arc::new(attrs)]).unwrap()
    }

    fn keys(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match &r.kind {
                RowKind::Node { node, .. } => node
                    .key
                    .clone()
                    .unwrap_or_else(|| format!("[{}]", node.index)),
                RowKind::More { label, .. } => label.clone(),
            })
            .collect()
    }

    /// A closed container is one row. That is the whole reason the tree is affordable.
    #[test]
    fn a_closed_node_costs_one_row() {
        let model = TreeModel::new(batch(3), 0, 0);
        assert_eq!(keys(&model.rows()), vec!["plan", "tags"]);
    }

    #[test]
    fn opening_a_node_reveals_its_children_indented() {
        let mut model = TreeModel::new(batch(3), 0, 0);
        model.toggle(&vec![1]);
        let rows = model.rows();
        assert_eq!(keys(&rows), vec!["plan", "tags", "[0]", "[1]", "[2]"]);
        assert_eq!(
            rows.iter().map(|r| r.depth).collect::<Vec<_>>(),
            vec![0, 0, 1, 1, 1]
        );
    }

    /// A leaf never opens, however hard it is asked.
    #[test]
    fn a_leaf_does_not_open() {
        let mut model = TreeModel::new(batch(3), 0, 0);
        model.toggle(&vec![0]);
        assert_eq!(keys(&model.rows()), vec!["plan", "tags"]);
    }

    /// Past a page, the tail row appears and says how many are left — and pressing it widens the
    /// window rather than opening anything.
    #[test]
    fn a_wide_container_pages_with_a_tail_row() {
        let mut model = TreeModel::new(batch(PAGE as i32 * 2 + 5), 0, 0);
        model.toggle(&vec![1]);
        let rows = model.rows();
        assert_eq!(rows.len(), 2 + PAGE + 1, "two roots, a page, and the tail");
        let last = keys(&rows).pop().unwrap();
        assert_eq!(last, format!("… {} more items", fmt_int(105)));

        model.reveal_more(&vec![1]);
        let rows = model.rows();
        assert_eq!(rows.len(), 2 + PAGE * 2 + 1);
        assert_eq!(keys(&rows).pop().unwrap(), "… 5 more items");

        model.reveal_more(&vec![1]);
        let rows = model.rows();
        assert_eq!(rows.len(), 2 + PAGE * 2 + 5, "no tail once it all fits");
    }

    /// Closing forgets the paging: reopening a container you had scrolled deep into should not
    /// still be thousands of rows long.
    #[test]
    fn closing_a_node_forgets_how_far_it_was_paged() {
        let mut model = TreeModel::new(batch(PAGE as i32 * 2), 0, 0);
        model.toggle(&vec![1]);
        model.reveal_more(&vec![1]);
        assert_eq!(model.rows().len(), 2 + PAGE * 2);
        model.toggle(&vec![1]);
        model.toggle(&vec![1]);
        assert_eq!(model.rows().len(), 2 + PAGE + 1);
    }
}
