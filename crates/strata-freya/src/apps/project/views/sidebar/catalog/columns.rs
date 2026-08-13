//! Flattening a catalog entry's (possibly nested) column tree into the flat list of **visible**
//! rows — every top-level column, plus the children of any expanded struct / list / map column,
//! depth-first. Ported from the Dioxus sidebar's `flatten_cols`.
//!
//! Two identifiers, deliberately distinct:
//!
//! - **`key`** — `"{owner}::{a.b.c}"`, the expansion-set key. Display-only: it only has to be
//!   unique per row, and a collision would merely expand the wrong twig.
//! - **`path`** — the `Vec<String>` that, with the owner, *is* the column's identity
//!   ([`ColRef`](strata_model::ColRef)). Not a dotted string: names come from the user's files and
//!   may contain dots, so a string that has to be parsed back is a bug waiting to be rediscovered.

use std::collections::HashSet;

use strata_model::{ColumnInfo, Kind};

/// One flattened, visible column row.
#[derive(PartialEq)]
pub struct ColRow {
    /// Expansion-set key (`"{owner}::{a.b.c}"`) — display-only, see the module docs.
    pub key: String,
    /// Path within the owner (`["address", "city"]`); a top-level column is one segment.
    pub path: Vec<String>,
    pub name: String,
    pub dtype: String,
    pub kind: Kind,
    /// A Hive partition column — a top-level concept only.
    pub is_part: bool,
    /// Nesting depth, driving the row's indent.
    pub depth: usize,
    pub has_children: bool,
    pub is_expanded: bool,
}

/// Walk `cols` into the visible rows under `owner`, appending to `out`. `parent` is the path of
/// the column being descended into (empty at the top level), `parts` the owner's partition
/// columns, `expanded` the set of expansion keys currently open.
pub fn flatten_cols(
    owner: &str,
    parent: &[String],
    depth: usize,
    cols: &[ColumnInfo],
    parts: &[(String, String)],
    expanded: &HashSet<String>,
    out: &mut Vec<ColRow>,
) {
    for c in cols {
        let mut path = parent.to_vec();
        path.push(c.name.clone());
        let key = format!("{owner}::{}", path.join("."));
        let has_children = !c.children.is_empty();
        let is_expanded = has_children && expanded.contains(&key);
        out.push(ColRow {
            key,
            name: c.name.clone(),
            dtype: c.dtype.clone(),
            kind: c.kind,
            is_part: depth == 0 && parts.iter().any(|(n, _)| n == &c.name),
            depth,
            has_children,
            is_expanded,
            path: path.clone(),
        });
        if is_expanded {
            flatten_cols(owner, &path, depth + 1, &c.children, parts, expanded, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use datafusion::arrow::datatypes::{DataType, Field};
    use strata_core::engine::column_info;

    use super::*;

    /// A leaf text column, or a struct over `children`, as the Arrow field production reads.
    fn field(name: &str, children: Vec<Field>) -> Field {
        if children.is_empty() {
            Field::new(name, DataType::Utf8, true)
        } else {
            Field::new(name, DataType::Struct(children.into()), true)
        }
    }

    /// The whole tree through the engine's own `column_info`, so the fixture's type, kind,
    /// chart role **and nested children** are the ones production would have derived — rather
    /// than a hand-built row wearing a derived one's clothes.
    fn col(name: &str, children: Vec<Field>) -> ColumnInfo {
        column_info(&field(name, children))
    }

    fn flatten(cols: &[ColumnInfo], parts: &[(String, String)], expanded: &[&str]) -> Vec<ColRow> {
        let exp: HashSet<String> = expanded.iter().map(ToString::to_string).collect();
        let mut out = Vec::new();
        flatten_cols("orders", &[], 0, cols, parts, &exp, &mut out);
        out
    }

    #[test]
    fn collapsed_struct_contributes_only_itself() {
        let cols = vec![
            col("address", vec![field("city", vec![])]),
            col("id", vec![]),
        ];
        let rows = flatten(&cols, &[], &[]);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["address", "id"]);
        assert!(rows[0].has_children && !rows[0].is_expanded);
        assert!(!rows[1].has_children);
    }

    #[test]
    fn expanding_a_struct_splices_its_children_in_place() {
        let cols = vec![
            col("address", vec![field("city", vec![])]),
            col("id", vec![]),
        ];
        let rows = flatten(&cols, &[], &["orders::address"]);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["address", "city", "id"], "depth-first, in place");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[1].path, vec!["address", "city"]);
        assert_eq!(rows[1].key, "orders::address.city");
    }

    #[test]
    fn nesting_recurses_only_through_expanded_ancestors() {
        let cols = vec![col("a", vec![field("b", vec![field("c", vec![])])])];
        let rows = flatten(&cols, &[], &["orders::a.b"]);
        assert_eq!(rows.len(), 1, "a closed ancestor hides the whole branch");

        let rows = flatten(&cols, &[], &["orders::a", "orders::a.b"]);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert_eq!(rows[2].depth, 2);
        assert_eq!(rows[2].path, vec!["a", "b", "c"]);
    }

    #[test]
    fn a_repeated_name_at_two_depths_keeps_distinct_paths() {
        let cols = vec![
            col("address", vec![field("city", vec![])]),
            col("city", vec![]),
        ];
        let rows = flatten(&cols, &[], &["orders::address"]);
        let nested = &rows[1];
        let top = &rows[2];
        assert_eq!(nested.name, top.name);
        assert_ne!(nested.path, top.path);
        assert_ne!(nested.key, top.key);
    }

    #[test]
    fn partition_flag_is_top_level_only() {
        let cols = vec![
            col("year", vec![]),
            col("nested", vec![field("year", vec![])]),
        ];
        let parts = vec![("year".to_string(), "Int32".to_string())];
        let rows = flatten(&cols, &parts, &["orders::nested"]);
        assert!(rows[0].is_part, "the top-level partition column is flagged");
        assert!(
            !rows[2].is_part,
            "a nested field sharing the name is not a partition column"
        );
    }
}
