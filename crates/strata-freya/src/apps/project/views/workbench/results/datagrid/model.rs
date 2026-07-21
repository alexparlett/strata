//! Datagrid data model — the column type vocabulary ([`Kind`]) with its theme-colour mapping, the
//! [`Column`] / [`GridData`] shapes and the throwaway [`fixture`], and the cell-padding [`Density`].
//! A stand-in for the real RecordBatch-shaped results model; only the fixture is disposable.

use freya::prelude::*;

use super::{DataGridTheme, N_ROWS};

/// A column's logical type — drives the header dtype-label colour and the cell text colour (matches
/// the Dioxus `Kind` → `text_class()` / `cell_class()`).
#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Str,
    Num,
    Bool,
    Ts,
    Struct,
    List,
    Map,
}

impl Kind {
    /// The header dtype-label colour (Dioxus `.ct .t-*`).
    pub fn type_color(self, t: &DataGridTheme) -> Color {
        match self {
            Kind::Str => t.type_str_color,
            Kind::Num => t.type_num_color,
            Kind::Bool => t.type_bool_color,
            Kind::Ts => t.type_ts_color,
            Kind::Struct => t.type_struct_color,
            Kind::List => t.type_list_color,
            Kind::Map => t.type_map_color,
        }
    }

    /// The cell text colour (Dioxus `.cell.num` / `.cell.ts` / `.cell.bool`; everything else default).
    pub fn cell_color(self, t: &DataGridTheme) -> Color {
        match self {
            Kind::Num => t.cell_num_color,
            Kind::Ts => t.cell_ts_color,
            Kind::Bool => t.type_bool_color,
            _ => t.color,
        }
    }
}

pub struct Column {
    pub name: &'static str,
    pub dtype: &'static str,
    pub kind: Kind,
}

/// The grid's input: columns + pre-formatted cell text per row.
pub struct GridData {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<String>>,
}

/// Cell padding density — the vertical breathing room around cell text (the horizontal inset is
/// fixed). Defaults to [`Comfortable`](Density::Comfortable); [`Compact`](Density::Compact) halves the
/// vertical padding for denser tables. Wire to a user setting later (the Dioxus grid has a compact toggle).
#[derive(Clone, Copy, PartialEq)]
pub enum Density {
    Comfortable,
    Compact,
}

impl Density {
    /// This density's cell padding, read from the `datagrid` theme (`comfortable_cell_padding` /
    /// `compact_cell_padding`) — the two formats live in the theme file, not in code. The horizontal
    /// sides inset the text; the vertical sides set the row height (`CELL_LINE_H + padding.vertical()`).
    pub fn padding(self, t: &DataGridTheme) -> Gaps {
        match self {
            Density::Comfortable => t.comfortable_cell_padding,
            Density::Compact => t.compact_cell_padding,
        }
    }
}

/// Build the throwaway fixture (10k rows × 8 typed columns) that stands in for real query results.
pub fn fixture() -> GridData {
    let columns = vec![
        Column { name: "id", dtype: "Int64", kind: Kind::Num },
        Column { name: "amount", dtype: "Float64", kind: Kind::Num },
        Column { name: "name", dtype: "Utf8", kind: Kind::Str },
        Column { name: "active", dtype: "Boolean", kind: Kind::Bool },
        Column { name: "created_at", dtype: "Timestamp", kind: Kind::Ts },
        Column { name: "score", dtype: "Int64", kind: Kind::Num },
        Column { name: "meta", dtype: "Struct", kind: Kind::Struct },
        Column { name: "tags", dtype: "List", kind: Kind::List },
    ];
    let names = ["alpha", "beta", "gamma", "delta"];
    let rows = (0..N_ROWS)
        .map(|i| {
            vec![
                i.to_string(),
                format!("{:.2}", (i as f32 * 1.37) % 1000.),
                format!("{}_{i}", names[i % names.len()]),
                (i % 2 == 0).to_string(),
                format!("2026-07-{:02} 12:{:02}", (i % 28) + 1, i % 60),
                ((i * 7) % 100).to_string(),
                "{…}".to_string(),
                "[…]".to_string(),
            ]
        })
        .collect();
    GridData { columns, rows }
}
