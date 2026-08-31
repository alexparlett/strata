//! The **column vocabulary**: one Arrow [`Field`] read into the [`ColumnInfo`] row every surface
//! shows — its type spelling, its [`Kind`], its chart role and its nested children.
//!
//! One place, because a column's row is derived from an Arrow type in exactly one place: anything
//! building a column — a fixture included — goes through [`column_info`] rather than hand-writing a
//! row whose `kind` and `role` are then a second opinion about the same type.

use arrow::datatypes::{DataType, Field};
use strata_model::{ChartRole, ColumnInfo, Kind};

/// What a chart may encode a column of this type as (`docs/CHART_SPEC.md` §3) — matched on
/// the `DataType` itself, here, because this is the last place that still has one: everything
/// downstream sees a [`ColumnInfo`], and a type's *spelling* is a rendering of a type rather
/// than the type.
///
/// The measure arm is [`DataType::is_numeric`] rather than a list of variants, because that is
/// the very predicate the chart read gates a Y on — so the encoder cannot offer a
/// measure the read would then refuse. The two time arms together mirror the same module's
/// `positions`, which is what gives those columns a place on an axis; they are *two* because a
/// date stride bins a calendar instant and means nothing to a time of day (see
/// [`ChartRole::Instant`]). A dictionary is a **dimension**
/// whatever it encodes: it is a category by construction, and a dictionary of numbers is not a
/// measure the read accepts. Anything else — nested, binary, interval, a variant Arrow grows
/// later — is [`ChartRole::Other`] and is offered nowhere, which is the safe default in the
/// direction that matters.
pub fn chart_role(dt: &DataType) -> ChartRole {
    if dt.is_numeric() {
        return ChartRole::Measure;
    }
    match dt {
        DataType::Date32 | DataType::Date64 | DataType::Timestamp(_, _) => ChartRole::Instant,
        DataType::Time32(_) | DataType::Time64(_) => ChartRole::Clock,
        DataType::Utf8
        | DataType::LargeUtf8
        | DataType::Utf8View
        | DataType::Boolean
        | DataType::Dictionary(_, _) => ChartRole::Dimension,
        _ => ChartRole::Other,
    }
}

/// The vocabulary row an Arrow field becomes.
pub fn column_info(field: &Field) -> ColumnInfo {
    let dtype = short_type(field.data_type());
    ColumnInfo {
        name: field.name().clone(),
        kind: Kind::from_arrow(&dtype),
        role: chart_role(field.data_type()),
        dtype,
        nullable: field.is_nullable(),
        children: nested_children(field.data_type()),
        stats: Vec::new(),
    }
}

fn nested_children(dt: &DataType) -> Vec<ColumnInfo> {
    match dt {
        DataType::Struct(fields) => fields.iter().map(|f| column_info(f)).collect(),
        DataType::List(f) | DataType::LargeList(f) | DataType::FixedSizeList(f, _) => {
            vec![column_info(f)]
        }
        DataType::Map(entries, _) => nested_children(entries.data_type()),
        _ => Vec::new(),
    }
}

/// The type spelling every surface shows — the grid's header, the inspector, and the value tree's
/// rows ([`value_tree::cell_children`](crate::value_tree::cell_children)). One function, so a node
/// and its column cannot disagree.
///
/// The composite types are matched **by variant, not by their `Debug` text**, which matters more
/// than it looks: `DataType`'s `Debug` is *recursive*, so `format!("{dt:?}")` on a struct renders
/// its entire subtree just to have the first word taken off the front. On `config.json`'s
/// `contentBlocks` (19,311 keys, each a struct) that one call cost **18ms** — and [`column_info`]
/// makes it per field, all the way down. Every remaining variant's `Debug` is a single term, so
/// those still take the parameters off the front generically rather than being enumerated here.
pub fn short_type(dt: &DataType) -> String {
    match dt {
        DataType::Struct(_) => "Struct".into(),
        DataType::List(_) | DataType::LargeList(_) | DataType::FixedSizeList(..) => "List".into(),
        DataType::ListView(_) => "ListView".into(),
        DataType::LargeListView(_) => "LargeListView".into(),
        DataType::Map(..) => "Map".into(),
        DataType::Union(..) => "Union".into(),
        DataType::Dictionary(..) => "Dictionary".into(),
        DataType::RunEndEncoded(..) => "RunEndEncoded".into(),
        DataType::LargeUtf8 => "Utf8".into(),
        leaf => {
            let full = format!("{leaf:?}");
            full.split(['(', '<']).next().unwrap_or(&full).to_string()
        }
    }
}
