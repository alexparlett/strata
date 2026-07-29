//! Union-tolerant JSON schema inference.
//!
//! A fork of arrow-json's `infer_json_schema_from_iterator` with **one** rule changed: where
//! arrow errors on a key whose value disagrees across occurrences, we record `Text` and the
//! column becomes `Utf8` holding that value's raw JSON.
//!
//! Arrow's own merge (`InferredType::merge`, arrow-json/src/reader/schema.rs) admits exactly
//! five combinations — Scalar∪Scalar, Array∪Array, Object∪Object, anything∪Any, and the
//! scalar↔array promotion — and returns `Err` for every other pair. So `{"c": ["x", true]}`
//! followed by `{"c": {...}}` fails the whole registration with
//! `Expected object json type, found: Array(Scalar({Utf8, Boolean}))`, naming neither the key
//! nor the file.
//!
//! # The rule is deliberately narrow
//!
//! `Text` is produced **only** where arrow would have errored. Every combination arrow can
//! already infer is inferred identically here, which is what makes this reader a superset of
//! the stock one rather than a behaviour change to every JSON table in every project.
//!
//! # Why `Utf8` and not a union
//!
//! Parquet has no union logical type, so a union column could not be exported — and the export
//! window is opened *on a result*, pinning the snapshot precisely so it can always write what it
//! is showing. A struct-of-variants cannot hold the array arm at all. `Utf8` costs nothing
//! downstream: the grid, inspector, profiler and export all handle it unchanged, and the
//! `json_get` family reads straight into it.

use std::collections::{BTreeMap, HashSet};

use datafusion::arrow::datatypes::{DataType, Field, Fields, Schema};
use serde_json::{Map, Value};

/// What we have learned about one JSON path so far.
///
/// Mirrors arrow's `InferredType` plus [`Text`](Inferred::Text) — the absorbing state a conflict
/// collapses to. `BTreeMap` rather than arrow's `HashMap` so a schema's field order is stable
/// across runs; an inferred schema is compared against the stored def, and a field order that
/// varied per registration would make every comparison spurious.
#[derive(Debug, Clone, PartialEq)]
pub enum Inferred {
    /// Nothing yet — JSON `null`, or a key that has only ever been absent.
    Any,
    /// One or more scalar arrow types, coerced together on the way out.
    Scalar(HashSet<DataType>),
    /// A list; the element type is itself inferred across every element seen.
    Array(Box<Inferred>),
    /// An object, inferred per key.
    Object(BTreeMap<String, Inferred>),
    /// **The conflict state.** Two shapes met that arrow has no common type for, so the value
    /// is carried as its own JSON text. Absorbing: once `Text`, always `Text`.
    Text,
}

impl Inferred {
    /// Fold `other` into `self`, collapsing to [`Text`](Inferred::Text) where arrow would have
    /// failed instead.
    pub fn merge(&mut self, other: Inferred) {
        // Take ownership so the match arms can move out of `self` without cloning the (possibly
        // very deep) subtree — 65MB of config produced 236k paths, so this runs a lot.
        let this = std::mem::replace(self, Inferred::Any);
        *self = match (this, other) {
            // Absorbing in both directions.
            (Inferred::Text, _) | (_, Inferred::Text) => Inferred::Text,

            // Any is the identity.
            (Inferred::Any, v) | (v, Inferred::Any) => v,

            (Inferred::Scalar(mut a), Inferred::Scalar(b)) => {
                a.extend(b);
                Inferred::Scalar(a)
            }
            (Inferred::Array(mut a), Inferred::Array(b)) => {
                a.merge(*b);
                Inferred::Array(a)
            }
            (Inferred::Object(mut a), Inferred::Object(b)) => {
                for (k, v) in b {
                    a.entry(k).or_insert(Inferred::Any).merge(v);
                }
                Inferred::Object(a)
            }

            // Arrow's scalar↔array promotion: `1` and `[2]` in the same key is a list of ints,
            // not a conflict. Kept so the narrow rule holds.
            (Inferred::Array(mut inner), s @ Inferred::Scalar(_)) => {
                inner.merge(s);
                Inferred::Array(inner)
            }
            (s @ Inferred::Scalar(_), Inferred::Array(mut inner)) => {
                inner.merge(s);
                Inferred::Array(inner)
            }

            // Object vs Array, Object vs Scalar — the arms arrow has no answer for.
            _ => Inferred::Text,
        };
    }
}

/// What one JSON value says about its own path.
fn of_value(v: &Value) -> Inferred {
    match v {
        Value::Null => Inferred::Any,
        Value::Bool(_) => Inferred::Scalar(one(DataType::Boolean)),
        // Arrow's exact rule (arrow-json reader/schema.rs:381): `is_i64` or nothing. Adding
        // `is_u64` here looks like a widening and is a bug — a u64 above `i64::MAX` would infer
        // `Int64`, registration would succeed, and arrow's `Int64` decoder would then fail to
        // parse the value on every scan. Diverging from arrow's *inference* is only safe where we
        // also own the *decode*, and the scalar arms are exactly where we do not.
        Value::Number(n) => Inferred::Scalar(one(if n.is_i64() {
            DataType::Int64
        } else {
            DataType::Float64
        })),
        Value::String(_) => Inferred::Scalar(one(DataType::Utf8)),
        Value::Array(items) => {
            let mut elem = Inferred::Any;
            for item in items {
                elem.merge(of_value(item));
            }
            Inferred::Array(Box::new(elem))
        }
        Value::Object(map) => Inferred::Object(of_object(map)),
    }
}

fn of_object(map: &Map<String, Value>) -> BTreeMap<String, Inferred> {
    map.iter().map(|(k, v)| (k.clone(), of_value(v))).collect()
}

fn one(dt: DataType) -> HashSet<DataType> {
    let mut hs = HashSet::new();
    hs.insert(dt);
    hs
}

/// Infer a schema across `records`, each of which must be a JSON object.
///
/// The error case is deliberately kept: a top-level value that is not an object is not a record,
/// and no stringify rule rescues that — it means the file is being read in the wrong
/// [`JsonShape`](strata_model::JsonShape), which `json_shape_error` already explains.
pub fn infer<'a, I>(records: I) -> Result<Schema, String>
where
    I: IntoIterator<Item = &'a Value>,
{
    let mut root = Tree::new();
    for rec in records {
        absorb(&mut root, rec)?;
    }
    Ok(schema_of(&root))
}

/// The partially-inferred root: one [`Inferred`] per top-level key.
///
/// Exposed because **merging happens here, not on the `Schema`**. Collapsing each file to a
/// `Schema` and folding those with `Schema::try_merge` throws the conflict rule away — arrow's
/// `Field::try_merge` hard-errors on Struct-vs-Utf8 — so a conflict spanning two files failed
/// registration with a raw arrow message even though the same conflict inside one file was
/// handled. Fold `Tree`s with [`absorb`] and call [`schema_of`] once at the end.
pub type Tree = BTreeMap<String, Inferred>;

/// Fold one record into `root`.
pub fn absorb(root: &mut Tree, rec: &Value) -> Result<(), String> {
    let Value::Object(map) = rec else {
        return Err(format!(
            "Expected JSON record to be an object, found {}",
            kind_word(rec)
        ));
    };
    for (k, v) in map {
        root.entry(k.clone())
            .or_insert(Inferred::Any)
            .merge(of_value(v));
    }
    Ok(())
}

/// The arrow schema for a fully-folded [`Tree`].
pub fn schema_of(root: &Tree) -> Schema {
    Schema::new(fields_of(root))
}

/// The type word for a non-record top-level value.
///
/// Deliberately arrow's own vocabulary — `Array`, not "an array". This message is **parsed** by
/// `catalog::json_shape_error`, which turns it into "the source is a JSON array. Set the JSON
/// shape to array in Table Config" by taking the word up to the first space or bracket. Replacing
/// arrow's reader with ours must not silently replace the diagnosis the user gets, so the wording
/// is part of the contract, not incidental prose.
///
/// Never the value itself: arrow's equivalent arm interpolated the whole parsed document into its
/// error, which is why `MAX_PASSTHROUGH` exists in `catalog.rs`.
pub fn kind_word(v: &Value) -> &'static str {
    match v {
        Value::Null => "Null",
        Value::Bool(_) => "Bool",
        Value::Number(_) => "Number",
        Value::String(_) => "String",
        Value::Array(_) => "Array",
        Value::Object(_) => "Object",
    }
}

/// Metadata key marking a `Utf8` field whose contents are **JSON text**, not prose.
///
/// The two are indistinguishable by `DataType` alone and must not be normalized the same way: a
/// conflicted column has to hold uniformly valid JSON (so a string value is quoted, and
/// `json_get` can read every row), while an ordinary string column must keep its values verbatim
/// (quoting them would put `"` around every cell in the grid). `normalize::fit` reads this;
/// `Schema::project` preserves field metadata, so it survives the opener's projection.
pub const JSON_TEXT_KEY: &str = "strata.json_text";

/// Whether this node is carried as JSON text rather than as a typed column.
///
/// Two nodes qualify. [`Text`](Inferred::Text) is the conflict state. An **empty object** is the
/// other: `{}` tells us a value was an object and nothing about its keys, and the honest arrow
/// type for that is not `Struct([])` — parquet cannot write a zero-field struct at all
/// (`Parquet does not support writing empty structs`), so inferring one produces a table that
/// registers and then fails every query that touches it. `sample/config.json` has 19,159 of them.
fn is_json_text(t: &Inferred) -> bool {
    match t {
        Inferred::Text => true,
        Inferred::Object(map) => map.is_empty(),
        _ => false,
    }
}

fn fields_of(map: &BTreeMap<String, Inferred>) -> Fields {
    map.iter()
        .map(|(k, t)| {
            let f = Field::new(k, datatype_of(t), true);
            if is_json_text(t) {
                f.with_metadata([(JSON_TEXT_KEY.to_string(), "1".to_string())].into())
            } else {
                f
            }
        })
        .collect()
}

/// The arrow type for an inferred node.
///
/// [`Any`](Inferred::Any) becomes `Null` exactly as arrow does — a key seen only as JSON `null`
/// has no other honest type, and `Null` is what the stock reader produces for it.
pub fn datatype_of(t: &Inferred) -> DataType {
    if is_json_text(t) {
        return DataType::Utf8;
    }
    match t {
        Inferred::Any => DataType::Null,
        Inferred::Text => DataType::Utf8,
        Inferred::Scalar(hs) => coerce(hs),
        Inferred::Object(map) => DataType::Struct(fields_of(map)),
        Inferred::Array(elem) => DataType::List(std::sync::Arc::new(Field::new_list_field(
            datatype_of(elem),
            true,
        ))),
    }
}

/// Arrow's scalar coercion: ints and floats meet at `Float64`, anything else mixed becomes
/// `Utf8`. Iterated in a fixed order because `HashSet` iteration is not deterministic and the
/// fold is not commutative for the `_ => Utf8` arm.
fn coerce(hs: &HashSet<DataType>) -> DataType {
    let mut types: Vec<&DataType> = hs.iter().collect();
    types.sort_by_key(|t| format!("{t:?}"));
    let mut out = match types.first() {
        Some(t) => (*t).clone(),
        None => return DataType::Null,
    };
    for t in types.into_iter().skip(1) {
        out = match (&out, t) {
            (DataType::Null, o) => (*o).clone(),
            (o, DataType::Null) => o.clone(),
            (a, b) if a == b => a.clone(),
            (DataType::Int64, DataType::Float64) | (DataType::Float64, DataType::Int64) => {
                DataType::Float64
            }
            _ => DataType::Utf8,
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(records: &[Value]) -> Schema {
        infer(records.iter()).expect("records are objects")
    }

    fn field<'a>(s: &'a Schema, name: &str) -> &'a Field {
        s.field_with_name(name).expect("field present")
    }

    /// The exact shape that defeats the stock reader — `sample/config.json`'s
    /// `nba.nbas[].contentVariants[].content.content[].content`, which is a string, an object
    /// and an array across its 5171 occurrences.
    #[test]
    fn a_key_that_is_scalar_object_and_array_becomes_text() {
        let s = schema(&[
            json!({"content": "plain text"}),
            json!({"content": {"kind": "block"}}),
            json!({"content": ["a", true]}),
        ]);
        assert_eq!(field(&s, "content").data_type(), &DataType::Utf8);
    }

    /// Order must not matter — the conflict state is absorbing in both directions, so an object
    /// arriving after an array and before one give the same answer.
    #[test]
    fn the_conflict_state_is_order_independent() {
        let shapes = [json!({"c": {"a": 1}}), json!({"c": ["x"]}), json!({"c": 3})];
        for rotation in 0..shapes.len() {
            let mut rotated = shapes.to_vec();
            rotated.rotate_left(rotation);
            assert_eq!(
                field(&schema(&rotated), "c").data_type(),
                &DataType::Utf8,
                "rotation {rotation} disagreed"
            );
        }
    }

    /// The narrow rule: everything arrow can already infer must infer identically, or this
    /// reader is a behaviour change to every existing JSON table rather than a superset.
    #[test]
    fn combinations_arrow_accepts_are_untouched() {
        let s = schema(&[
            json!({"i": 1, "f": 1.5, "b": true, "s": "x", "l": [1, 2], "o": {"k": "v"}}),
            json!({"i": 2, "f": 2.5, "b": false, "s": "y", "l": [3], "o": {"k": "w"}}),
        ]);
        assert_eq!(field(&s, "i").data_type(), &DataType::Int64);
        assert_eq!(field(&s, "f").data_type(), &DataType::Float64);
        assert_eq!(field(&s, "b").data_type(), &DataType::Boolean);
        assert_eq!(field(&s, "s").data_type(), &DataType::Utf8);
        assert!(matches!(field(&s, "l").data_type(), DataType::List(_)));
        assert!(matches!(field(&s, "o").data_type(), DataType::Struct(_)));
    }

    /// Arrow's own coercions, kept: int meets float at Float64, and a scalar meets a list of
    /// that scalar as a list. Neither is a conflict, so neither may become text.
    #[test]
    fn arrows_scalar_coercions_are_kept() {
        let s = schema(&[json!({"n": 1}), json!({"n": 2.5})]);
        assert_eq!(field(&s, "n").data_type(), &DataType::Float64);

        let s = schema(&[json!({"p": 1}), json!({"p": [2, 3]})]);
        let DataType::List(inner) = field(&s, "p").data_type() else {
            panic!("scalar and list promote to a list");
        };
        assert_eq!(inner.data_type(), &DataType::Int64);
    }

    /// A conflict inside a struct stringifies **at the level it occurs**, leaving its siblings
    /// and its ancestors fully typed. Collapsing the whole parent would throw away the 236k
    /// paths that are perfectly inferrable.
    #[test]
    fn a_nested_conflict_stringifies_only_that_field() {
        let s = schema(&[
            json!({"outer": {"id": 1, "content": "text"}}),
            json!({"outer": {"id": 2, "content": {"nested": true}}}),
        ]);
        let DataType::Struct(fields) = field(&s, "outer").data_type() else {
            panic!("outer stays a struct");
        };
        let by = |n: &str| fields.iter().find(|f| f.name() == n).expect(n).clone();
        assert_eq!(by("id").data_type(), &DataType::Int64);
        assert_eq!(by("content").data_type(), &DataType::Utf8);
    }

    /// The recursive case from the real file: the conflict is inside a list of structs, and
    /// every element of that list merges into one element type.
    #[test]
    fn a_conflict_inside_a_list_of_structs_stringifies() {
        let s = schema(&[json!({
            "blocks": [
                {"content": "text"},
                {"content": {"kind": "image"}},
                {"content": ["a", "b"]},
            ]
        })]);
        let DataType::List(elem) = field(&s, "blocks").data_type() else {
            panic!("blocks is a list");
        };
        let DataType::Struct(fields) = elem.data_type() else {
            panic!("of structs");
        };
        assert_eq!(fields[0].data_type(), &DataType::Utf8);
    }

    /// JSON null is `Any`, which merges with anything and never provokes a conflict — a key
    /// that is null in one record and an object in the next is just that object.
    #[test]
    fn null_is_not_a_conflict() {
        let s = schema(&[json!({"c": null}), json!({"c": {"k": 1}})]);
        assert!(matches!(field(&s, "c").data_type(), DataType::Struct(_)));

        // ...and a key that is only ever null keeps arrow's answer for it.
        let s = schema(&[json!({"c": null})]);
        assert_eq!(field(&s, "c").data_type(), &DataType::Null);
    }

    /// A top-level non-object is still an error. No stringify rule rescues it: it means the
    /// file is being read in the wrong shape, which is a different diagnosis entirely.
    #[test]
    fn a_top_level_non_record_is_still_an_error() {
        let arr = json!([1, 2, 3]);
        let err = infer(std::iter::once(&arr)).expect_err("not a record");
        // Arrow's exact wording, because `catalog::json_shape_error` parses it — see `kind_word`.
        assert_eq!(err, "Expected JSON record to be an object, found Array");
        assert!(
            !err.contains('1'),
            "the value must not be interpolated: {err}"
        );
    }
}
