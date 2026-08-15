//! Flattening a catalog entry's (possibly nested) column tree into the flat list of **visible**
//! rows — every top-level column, plus the children of any expanded struct / list / map column,
//! depth-first. Ported from the Dioxus sidebar's `flatten_cols`.
//!
//! Two identifiers, deliberately distinct:
//!
//! - **`key`** — the tree's expansion key *and* the row's reconciliation key, so it has to be
//!   **injective**, not merely unique-in-practice. A plain dotted join is neither: a struct `a`
//!   with a child `b` and a flat column literally named `a.b` produce one string, which shares one
//!   entry in the expansion set and — since the fix that keyed these rows — trips Freya's
//!   duplicate-sibling-key panic. So each segment is escaped before it is joined ([`segment`]),
//!   and the owner is separated by `::` rather than the `/` that joins node paths, so a table
//!   named `orders/id` cannot address `orders`'s `id` column.
//! - **`path`** — the `Vec<String>` that, with the owner, *is* the column's identity
//!   ([`ColRef`](strata_model::ColRef)). Not a dotted string: names come from the user's files and
//!   may contain dots, so a string that has to be parsed back is a bug waiting to be rediscovered.

use std::collections::HashSet;

use strata_model::{ColumnInfo, Kind};

/// One flattened, visible column row.
#[derive(Clone, PartialEq)]
pub struct ColRow {
    /// The tree's expansion key and this row's reconciliation key — injective over `path`, see
    /// the module docs.
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

/// One path segment, escaped so the join that follows is reversible: a backslash doubles and a
/// dot is backslash-escaped, so `["a", "b"]` and `["a.b"]` can no longer produce one string.
///
/// Escaping rather than a separator no name may contain, because "no name may contain it" is the
/// assumption this key was built on and it was wrong — these names come from the user's files and
/// from a remote server.
fn segment(name: &str) -> String {
    name.replace('\\', "\\\\").replace('.', "\\.")
}

/// Walk `cols` into the visible rows under `owner`, appending to `out`. `owner` is the owning
/// row's **node path**, `parent` the path of the column being descended into (empty at the top
/// level), `parts` the owner's partition columns, `expanded` the tree's expansion set.
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
        let key = format!(
            "{owner}::{}",
            path.iter()
                .map(|s| segment(s))
                .collect::<Vec<_>>()
                .join(".")
        );
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
    use strata_engine::column_info;

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
        flatten_cols("ws/tables/orders", &[], 0, cols, parts, &exp, &mut out);
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
        let rows = flatten(&cols, &[], &["ws/tables/orders::address"]);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["address", "city", "id"], "depth-first, in place");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[1].path, vec!["address", "city"]);
        assert_eq!(rows[1].key, "ws/tables/orders::address.city");
    }

    #[test]
    fn nesting_recurses_only_through_expanded_ancestors() {
        let cols = vec![col("a", vec![field("b", vec![field("c", vec![])])])];
        let rows = flatten(&cols, &[], &["ws/tables/orders::a.b"]);
        assert_eq!(rows.len(), 1, "a closed ancestor hides the whole branch");

        let rows = flatten(
            &cols,
            &[],
            &["ws/tables/orders::a", "ws/tables/orders::a.b"],
        );
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
        let rows = flatten(&cols, &[], &["ws/tables/orders::address"]);
        let nested = &rows[1];
        let top = &rows[2];
        assert_eq!(nested.name, top.name);
        assert_ne!(nested.path, top.path);
        assert_ne!(nested.key, top.key);
    }

    /// **The key is injective over the path**, which it has to be: it is the row's
    /// reconciliation key as well as its expansion key, and two siblings sharing one is a panic
    /// in Freya's differ, not a mis-expanded twig. A struct `a` holding `b` and a flat column
    /// literally named `a.b` are the case a plain dotted join collapses.
    #[test]
    fn a_dotted_column_name_cannot_collide_with_a_nested_path() {
        let cols = vec![col("a", vec![field("b", vec![])]), col("a.b", vec![])];
        let rows = flatten(&cols, &[], &["ws/tables/orders::a"]);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "a.b"]);

        let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "ws/tables/orders::a",
                "ws/tables/orders::a.b",
                "ws/tables/orders::a\\.b"
            ],
            "the flat column's dot is escaped, so it is a different key from a.b nested"
        );
        assert_ne!(rows[1].key, rows[2].key);
    }

    #[test]
    fn partition_flag_is_top_level_only() {
        let cols = vec![
            col("year", vec![]),
            col("nested", vec![field("year", vec![])]),
        ];
        let parts = vec![("year".to_string(), "Int32".to_string())];
        let rows = flatten(&cols, &parts, &["ws/tables/orders::nested"]);
        assert!(rows[0].is_part, "the top-level partition column is flagged");
        assert!(
            !rows[2].is_part,
            "a nested field sharing the name is not a partition column"
        );
    }
}
