//! The **value tree** (P2-25): one nested cell as expandable nodes, read straight off the Arrow
//! arrays.
//!
//! No JSON. A tree already carries the structure that JSON's braces exist to express, so encoding
//! to text first would be work done only to be parsed back by the eye — and it would *lose* things:
//! a leaf would arrive quoted (`"2026-07-30T09:22:48"` for a timestamp), and the type of each node,
//! which the tree wants for its dtype badge, is in the Arrow schema rather than anywhere in the
//! JSON. So a leaf is formatted by the **same `ArrayFormatter` the grid formats a cell with**, and
//! clipped by the same [`clip`] — a value reads identically whether you meet it in the grid, the
//! record view or here. (The record view's sampled text preview,
//! [`serialize::cell_preview_json`](super::serialize::cell_preview_json), is still JSON: it is a
//! *document* excerpt, where the braces are the whole point.)
//!
//! Nothing is materialized before it is asked for. A node is addressed by a **path** of entry
//! indices, and [`cell_children`] resolves that path with O(1) Arrow slices, so opening one key of
//! a 19,311-key object costs the same as opening one key of a two-key object. Wide containers are
//! paged (`skip` / `take`) rather than returned whole, because the caller's tree is virtualized and
//! 19,311 rows it will not draw are 19,311 rows it should not be handed.
//!
//! Indices, not names, address a node: a duplicate or reordered key cannot mis-resolve a path, and
//! a list has no names at all.

use datafusion::arrow::array::{Array, ArrayRef, AsArray, RecordBatch};
use datafusion::arrow::datatypes::{DataType, FieldRef};
use datafusion::arrow::util::display::{ArrayFormatter, FormatOptions};
use strata_model::Kind;

use super::catalog::short_type;
use crate::util::{clip, DISPLAY_CHARS};

/// One row of the tree: a child of the node that was expanded.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueNode {
    /// A struct field name or a map key. `None` for a list item, whose row is titled by its index.
    pub key: Option<String>,
    /// Position within its parent — the path step that reaches this node.
    pub index: usize,
    /// The type, in the spelling the grid's header and the column inspector use
    /// (`catalog::short_type`), so one value cannot be described two ways.
    pub dtype: String,
    pub kind: Kind,
    pub value: NodeValue,
}

/// What a row shows beside its key, and whether it opens.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeValue {
    /// A scalar, formatted as the grid formats a cell and clipped to [`DISPLAY_CHARS`].
    Leaf(String),
    /// Null — its own variant rather than a `Leaf("")`, because the row dims it like the grid does
    /// and "null" and "empty string" are different facts.
    Null,
    /// An expandable container, and how many entries it holds.
    Nest(usize),
}

impl NodeValue {
    /// Whether this row can be opened. A container with no entries cannot — `{}` is a leaf as far
    /// as a reader is concerned.
    pub fn expandable(&self) -> bool {
        matches!(self, NodeValue::Nest(n) if *n > 0)
    }
}

/// A position inside a value: the field that describes it, the array that holds it, and the index
/// within that array.
#[derive(Clone)]
struct Cursor {
    field: FieldRef,
    array: ArrayRef,
    idx: usize,
}

/// The cell itself, as the tree's root row. `None` when the cell is out of range.
pub fn cell_root(batch: &RecordBatch, col: usize, row: usize) -> Option<ValueNode> {
    let cursor = cell_cursor(batch, col, row)?;
    Some(node(cursor.field.name().clone().into(), 0, &cursor))
}

/// The children of the node `path` reaches inside cell (`col`, `row`) — from `skip`, at most `take`.
///
/// `None` when the path does not resolve (a stale path after a page flip) or lands on something with
/// no children; an empty `Vec` when it resolves to a container with nothing in the requested window.
pub fn cell_children(
    batch: &RecordBatch,
    col: usize,
    row: usize,
    path: &[usize],
    skip: usize,
    take: usize,
) -> Option<Vec<ValueNode>> {
    let cursor = resolve(batch, col, row, path)?;
    let (keys, cursors) = entries(&cursor, skip, take)?;
    Some(
        cursors
            .into_iter()
            .zip(keys)
            .enumerate()
            .map(|(i, (c, key))| node(key, skip + i, &c))
            .collect(),
    )
}

/// How many entries the node at `path` holds — what a paging control needs, and cheap: every
/// container knows its own length without its contents being read.
pub fn cell_len(batch: &RecordBatch, col: usize, row: usize, path: &[usize]) -> Option<usize> {
    len(&resolve(batch, col, row, path)?)
}

/// The cursor for one cell of a batch.
fn cell_cursor(batch: &RecordBatch, col: usize, row: usize) -> Option<Cursor> {
    let field = batch.schema_ref().fields().get(col)?.clone();
    let array = batch.columns().get(col)?.clone();
    (row < array.len()).then_some(Cursor {
        field,
        array,
        idx: row,
    })
}

/// Walk `path` from the cell, one entry index per level.
fn resolve(batch: &RecordBatch, col: usize, row: usize, path: &[usize]) -> Option<Cursor> {
    let mut cursor = cell_cursor(batch, col, row)?;
    for &step in path {
        // One child at a time: `entries` does the type match once per level either way, and a
        // single-child window keeps a deep path O(depth) rather than O(depth x width).
        let (_, mut children) = entries(&cursor, step, 1)?;
        cursor = children.pop()?;
    }
    Some(cursor)
}

/// Describe one node: its type, and whether it is a leaf, a null or an openable container.
fn node(key: Option<String>, index: usize, cursor: &Cursor) -> ValueNode {
    let dtype = short_type(cursor.array.data_type());
    ValueNode {
        key,
        index,
        kind: Kind::from_arrow(&dtype),
        dtype,
        value: value_of(cursor),
    }
}

fn value_of(cursor: &Cursor) -> NodeValue {
    // `DataType::Null`'s nulls are logical, so `is_null` reports false for every index of it — the
    // same trap the preview's leaf encoding hit (see `serialize`).
    if cursor.array.is_null(cursor.idx) || matches!(cursor.array.data_type(), DataType::Null) {
        return NodeValue::Null;
    }
    match len(cursor) {
        Some(n) => NodeValue::Nest(n),
        None => NodeValue::Leaf(leaf_text(cursor)),
    }
}

/// A leaf as the grid would print it. `ArrayFormatter` is the grid's own formatter, so a number,
/// decimal or timestamp reads here exactly as it reads in a cell.
///
/// A **string is clipped without being copied first**. `ArrayFormatter` renders through `Display`,
/// so asking it for a 30MB text value materializes all 30MB only for `clip` to discard nearly all
/// of it — which is the unbounded materialization this whole module exists to avoid, reintroduced
/// one row at a time. Reading the `&str` straight off the array and clipping the borrow costs the
/// characters kept. Every other leaf is bounded by its own type.
fn leaf_text(cursor: &Cursor) -> String {
    if let Some(text) = utf8_value(cursor.array.as_ref(), cursor.idx) {
        return clip(text, DISPLAY_CHARS).into_owned();
    }
    let options = FormatOptions::default();
    match ArrayFormatter::try_new(cursor.array.as_ref(), &options) {
        Ok(formatter) => clip(&formatter.value(cursor.idx).to_string(), DISPLAY_CHARS).into_owned(),
        // A type arrow cannot format at all. Naming it beats an empty row that looks like a bug.
        Err(_) => format!("<unprintable {}>", cursor.array.data_type()),
    }
}

/// The value at `idx` as a `&str`, for the three UTF-8 array layouts — the one leaf whose size is
/// unbounded, so the one that must be clipped from a borrow rather than from a copy.
fn utf8_value(array: &dyn Array, idx: usize) -> Option<&str> {
    match array.data_type() {
        DataType::Utf8 => Some(array.as_string::<i32>().value(idx)),
        DataType::LargeUtf8 => Some(array.as_string::<i64>().value(idx)),
        DataType::Utf8View => Some(array.as_string_view().value(idx)),
        _ => None,
    }
}

/// Entry count for a container; `None` for a leaf. Every arm is metadata or an offset lookup — no
/// entry is touched, which is what makes a 19,311-key object as cheap to describe as a small one.
fn len(cursor: &Cursor) -> Option<usize> {
    let (array, idx) = (&cursor.array, cursor.idx);
    match array.data_type() {
        DataType::Struct(fields) => Some(fields.len()),
        DataType::List(_) => Some(array.as_list::<i32>().value_length(idx) as usize),
        DataType::LargeList(_) => Some(array.as_list::<i64>().value_length(idx) as usize),
        // A list *view* carries per-element sizes rather than offsets, so its length reads there.
        DataType::ListView(_) => Some(array.as_list_view::<i32>().value_sizes()[idx] as usize),
        DataType::LargeListView(_) => Some(array.as_list_view::<i64>().value_sizes()[idx] as usize),
        DataType::FixedSizeList(_, n) => Some(*n as usize),
        DataType::Map(..) => Some(array.as_map().value_length(idx) as usize),
        _ => None,
    }
}

/// A window of a container's entries: their keys (`None` for list items) and their cursors.
/// `None` for a leaf.
fn entries(
    cursor: &Cursor,
    skip: usize,
    take: usize,
) -> Option<(Vec<Option<String>>, Vec<Cursor>)> {
    let (array, idx) = (&cursor.array, cursor.idx);
    let total = len(cursor)?;
    let window = skip..total.min(skip.saturating_add(take));
    match array.data_type() {
        DataType::Struct(fields) => {
            // A struct's children share the parent's index space, so descending one costs a field
            // lookup and an `Arc` clone.
            let columns = array.as_struct().columns();
            let mut keys = Vec::new();
            let mut cursors = Vec::new();
            for i in window {
                let field = fields.get(i)?.clone();
                keys.push(Some(field.name().clone()));
                cursors.push(Cursor {
                    field,
                    array: columns.get(i)?.clone(),
                    idx,
                });
            }
            Some((keys, cursors))
        }
        DataType::Map(entries, _) => {
            let DataType::Struct(kv) = entries.data_type() else {
                return None;
            };
            let value_field = kv.get(1)?.clone();
            let pairs = array.as_map().value(idx);
            let (key_array, values) = (pairs.column(0).clone(), pairs.column(1).clone());
            let formatter =
                ArrayFormatter::try_new(key_array.as_ref(), &FormatOptions::default()).ok()?;
            let mut keys = Vec::new();
            let mut cursors = Vec::new();
            for i in window {
                keys.push(Some(formatter.value(i).to_string()));
                cursors.push(Cursor {
                    field: value_field.clone(),
                    array: values.clone(),
                    idx: i,
                });
            }
            Some((keys, cursors))
        }
        _ => {
            // Every list flavour: narrow to this cell's items once (an O(1) Arrow slice), then
            // index into them.
            let (field, items) = list_items(cursor)?;
            let cursors = window
                .clone()
                .map(|i| Cursor {
                    field: field.clone(),
                    array: items.clone(),
                    idx: i,
                })
                .collect();
            Some((window.map(|_| None).collect(), cursors))
        }
    }
}

/// This cell's list items as an array, with the field describing them.
fn list_items(cursor: &Cursor) -> Option<(FieldRef, ArrayRef)> {
    let (array, idx) = (&cursor.array, cursor.idx);
    match array.data_type() {
        DataType::List(f) => Some((f.clone(), array.as_list::<i32>().value(idx))),
        DataType::LargeList(f) => Some((f.clone(), array.as_list::<i64>().value(idx))),
        DataType::ListView(f) => Some((f.clone(), array.as_list_view::<i32>().value(idx))),
        DataType::LargeListView(f) => Some((f.clone(), array.as_list_view::<i64>().value(idx))),
        DataType::FixedSizeList(f, _) => Some((f.clone(), array.as_fixed_size_list().value(idx))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::arrow::array::{Int32Array, ListArray, MapArray, StringArray, StructArray};
    use datafusion::arrow::buffer::OffsetBuffer;
    use datafusion::arrow::datatypes::{Field, Fields, Schema};

    use super::*;

    /// `{ "attrs": { "plan": "pro", "tags": ["a", "b", "c"] } }` — a struct holding a scalar and a
    /// list, which is every descent the tree does bar a map.
    fn batch() -> RecordBatch {
        let item = Arc::new(Field::new("item", DataType::Utf8, true));
        let tags = ListArray::new(
            item.clone(),
            OffsetBuffer::new(vec![0, 3].into()),
            Arc::new(StringArray::from(vec!["a", "b", "c"])),
            None,
        );
        let fields = Fields::from(vec![
            Field::new("plan", DataType::Utf8, true),
            Field::new("tags", DataType::List(item), true),
            Field::new("seats", DataType::Int32, true),
        ]);
        let attrs = StructArray::new(
            fields.clone(),
            vec![
                Arc::new(StringArray::from(vec!["pro"])) as ArrayRef,
                Arc::new(tags) as ArrayRef,
                Arc::new(Int32Array::from(vec![None::<i32>])) as ArrayRef,
            ],
            None,
        );
        let schema = Schema::new(vec![Field::new("attrs", DataType::Struct(fields), true)]);
        RecordBatch::try_new(Arc::new(schema), vec![Arc::new(attrs)]).unwrap()
    }

    #[test]
    fn the_root_is_the_column_and_counts_its_keys() {
        let root = cell_root(&batch(), 0, 0).expect("a root");
        assert_eq!(root.key.as_deref(), Some("attrs"));
        assert_eq!(root.dtype, "Struct");
        assert_eq!(root.kind, Kind::Struct);
        assert_eq!(root.value, NodeValue::Nest(3));
        assert!(root.value.expandable());
    }

    /// The top level: a scalar leaf, a nested list that counts its items, and a null — each with
    /// the type spelling the grid's header uses.
    #[test]
    fn children_describe_leaves_nests_and_nulls() {
        let kids = cell_children(&batch(), 0, 0, &[], 0, 10).expect("children");
        assert_eq!(kids.len(), 3);

        assert_eq!(kids[0].key.as_deref(), Some("plan"));
        assert_eq!(kids[0].value, NodeValue::Leaf("pro".into()));
        assert_eq!(kids[0].kind, Kind::Str);
        assert!(!kids[0].value.expandable());

        assert_eq!(kids[1].key.as_deref(), Some("tags"));
        assert_eq!(kids[1].dtype, "List");
        assert_eq!(kids[1].value, NodeValue::Nest(3));

        // A null field is Null, not an empty leaf — the row dims it like a grid cell.
        assert_eq!(kids[2].key.as_deref(), Some("seats"));
        assert_eq!(kids[2].value, NodeValue::Null);
    }

    /// A leaf reads as the grid prints it — no JSON quoting, which is the point of not going
    /// through a serializer.
    #[test]
    fn a_string_leaf_is_unquoted() {
        let kids = cell_children(&batch(), 0, 0, &[], 0, 1).expect("children");
        assert_eq!(kids[0].value, NodeValue::Leaf("pro".into()));
    }

    /// Descending: list items have no key, and carry their index.
    #[test]
    fn list_items_are_indexed_not_named() {
        let kids = cell_children(&batch(), 0, 0, &[1], 0, 10).expect("the list's items");
        assert_eq!(kids.len(), 3);
        assert!(kids.iter().all(|k| k.key.is_none()));
        assert_eq!(
            kids.iter().map(|k| k.index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(kids[2].value, NodeValue::Leaf("c".into()));
    }

    /// Paging a container: the window is honoured and each node keeps its **absolute** index, or a
    /// path built from a second page would address the wrong entry.
    #[test]
    fn a_window_keeps_absolute_indices() {
        let kids = cell_children(&batch(), 0, 0, &[1], 1, 1).expect("one item");
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].index, 1);
        assert_eq!(kids[0].value, NodeValue::Leaf("b".into()));
        assert_eq!(cell_len(&batch(), 0, 0, &[1]), Some(3));
    }

    #[test]
    fn a_window_past_the_end_is_empty_rather_than_missing() {
        assert_eq!(cell_children(&batch(), 0, 0, &[1], 9, 5), Some(Vec::new()));
    }

    /// A leaf has no children, and a path that cannot resolve is `None` rather than a panic — a
    /// tree holding a path across a page flip will ask.
    #[test]
    fn a_leaf_and_a_stale_path_resolve_to_nothing() {
        assert_eq!(cell_children(&batch(), 0, 0, &[0], 0, 10), None);
        assert_eq!(cell_children(&batch(), 0, 0, &[9], 0, 10), None);
        assert_eq!(cell_children(&batch(), 0, 9, &[], 0, 10), None);
        assert_eq!(cell_children(&batch(), 9, 0, &[], 0, 10), None);
    }

    /// A map's rows are keyed by its keys, formatted the same way a leaf is.
    #[test]
    fn map_entries_are_keyed() {
        let kv = Fields::from(vec![
            Field::new("keys", DataType::Utf8, false),
            Field::new("values", DataType::Int32, true),
        ]);
        let pairs = StructArray::new(
            kv.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b"])) as ArrayRef,
                Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef,
            ],
            None,
        );
        let entries_field = Arc::new(Field::new("entries", DataType::Struct(kv), false));
        let map = MapArray::new(
            entries_field.clone(),
            OffsetBuffer::new(vec![0, 2].into()),
            pairs,
            None,
            false,
        );
        let schema = Schema::new(vec![Field::new(
            "tags",
            DataType::Map(entries_field, false),
            true,
        )]);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![Arc::new(map)]).unwrap();

        assert_eq!(cell_root(&batch, 0, 0).unwrap().value, NodeValue::Nest(2));
        let kids = cell_children(&batch, 0, 0, &[], 0, 10).expect("entries");
        assert_eq!(kids[0].key.as_deref(), Some("a"));
        assert_eq!(kids[0].value, NodeValue::Leaf("1".into()));
        assert_eq!(kids[1].key.as_deref(), Some("b"));
    }

    /// An empty container is not expandable: `{}` is a leaf as far as a reader is concerned.
    #[test]
    fn an_empty_container_does_not_open() {
        let item = Arc::new(Field::new("item", DataType::Int32, true));
        let empty = ListArray::new(
            item.clone(),
            OffsetBuffer::new(vec![0, 0].into()),
            Arc::new(Int32Array::from(Vec::<i32>::new())),
            None,
        );
        let schema = Schema::new(vec![Field::new("xs", DataType::List(item), true)]);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![Arc::new(empty)]).unwrap();
        let root = cell_root(&batch, 0, 0).expect("a root");
        assert_eq!(root.value, NodeValue::Nest(0));
        assert!(!root.value.expandable());
    }
}
