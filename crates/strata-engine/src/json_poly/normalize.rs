//! Making a parsed record fit the schema [`infer`](mod@super::infer) produced.
//!
//! Inference decided that a conflicted path is `Utf8`; the decoder still has to be handed
//! something it can put there. Arrow's `StringArrayDecoder` accepts a JSON string and nothing
//! else — a bool or a number only with `coerce_primitive`, which DataFusion never sets, and an
//! object or array not at all (`Err(tape.error(pos, "string"))`). It has no notion of "the JSON
//! text of this value", so we rewrite the value before it reaches the tape.
//!
//! This is **not** a JSON→Arrow decoder; arrow still builds every array. It only ensures the
//! values arrow is given match the schema it was built with.
//!
//! Each rule here was found by running the real `sample/config.json` through the pipeline, and
//! each one is a distinct failure that file produces — see the comments on [`fit`].

use datafusion::arrow::datatypes::{DataType, Field, Fields};
use serde_json::Value;

use super::infer::JSON_TEXT_KEY;

/// Rewrite `value` in place so it fits `target`.
///
/// Three rules, each narrow:
///
/// - any non-string value whose target is `Utf8` becomes its own compact JSON text;
/// - a bare value whose target is a list is wrapped into a one-element list, finishing arrow's
///   own scalar↔array promotion;
/// - struct and list targets are walked, so both rules reach any depth.
///
/// Anything else is left exactly as parsed. In particular a value that does not fit a *non*-`Utf8`
/// target is **not** touched: nulling it would be silent data loss and there is no honest text
/// form for it, so arrow's own decode error stands, naming the field. That case is reachable only
/// when inference did not see the value, i.e. when a sampled inference missed a conflict that
/// appears later in the file.
pub fn fit(value: &mut Value, field: &Field) {
    match field.data_type() {
        _ if is_json_text(field) => {
            if !value.is_null() {
                *value = Value::String(json_text(value));
            }
        }
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => match value {
            Value::String(_) => {}
            Value::Null => {}
            _ => *value = Value::String(json_text(value)),
        },
        DataType::Struct(fields) => fit_record(value, fields),
        DataType::List(elem) | DataType::LargeList(elem) | DataType::FixedSizeList(elem, _) => {
            if !value.is_array() && !value.is_null() {
                *value = Value::Array(vec![value.take()]);
            }
            if let Value::Array(items) = value {
                for item in items {
                    fit(item, elem);
                }
            }
        }
        _ => {}
    }
}

/// [`fit`] over a whole record against a field list.
///
/// Also `fit`'s own `Struct` arm — walking an object against a field list is one rule, and it was
/// briefly written twice.
pub fn fit_record(value: &mut Value, fields: &Fields) {
    if let Value::Object(map) = value {
        for f in fields {
            if let Some(v) = map.get_mut(f.name()) {
                fit(v, f);
            }
        }
    }
}

/// Whether `field` carries JSON text rather than prose — see [`infer::JSON_TEXT_KEY`].
fn is_json_text(field: &Field) -> bool {
    field.metadata().contains_key(JSON_TEXT_KEY)
}

/// A value's compact JSON text.
///
/// `to_string` on a value that came *from* a parse cannot fail — there is no NaN and no
/// non-string map key — so the `unwrap_or` is unreachable rather than a swallowed error. It is
/// written as `"null"` and not `""` so that if it ever were reached the cell would say something
/// false-looking rather than impersonating an empty string.
fn json_text(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::arrow::datatypes::Field;
    use serde_json::json;
    use std::sync::Arc;

    /// An ordinary `Utf8` field — a prose column, not a conflicted one.
    fn utf8(name: &str) -> Field {
        Field::new(name, DataType::Utf8, true)
    }

    /// A **conflicted** `Utf8` field, marked the way `infer` marks one.
    fn json_field(name: &str) -> Field {
        utf8(name).with_metadata([(JSON_TEXT_KEY.to_string(), "1".to_string())].into())
    }

    /// Wrap a bare target type in an unmarked field, for the ordinary-column cases.
    fn plain(dt: DataType) -> Field {
        Field::new("c", dt, true)
    }

    #[test]
    fn an_object_in_a_text_slot_becomes_its_json() {
        let mut v = json!({"kind": "block", "n": 1});
        fit(&mut v, &utf8("c"));
        assert_eq!(v, json!(r#"{"kind":"block","n":1}"#));
    }

    #[test]
    fn an_array_in_a_text_slot_becomes_its_json() {
        let mut v = json!(["a", true, 3]);
        fit(&mut v, &utf8("c"));
        assert_eq!(v, json!(r#"["a",true,3]"#));
    }

    /// A plain string in an **ordinary** text column is left as its value, not re-encoded with
    /// quotes — serializing it would put `"` around every cell in the grid.
    #[test]
    fn a_string_in_an_ordinary_text_column_is_untouched() {
        let mut v = json!("plain");
        fit(&mut v, &utf8("c"));
        assert_eq!(v, json!("plain"));
    }

    /// But a string in a **conflicted** column *is* quoted, because that column holds JSON text
    /// and every row of it has to parse. Leaving it bare made a string containing JSON
    /// indistinguishable from the object it looks like, and left `json_get` unable to read the
    /// rows that were already text.
    #[test]
    fn a_conflicted_column_holds_uniformly_valid_json() {
        let f = json_field("content");

        let mut s = json!("hello");
        fit(&mut s, &f);
        assert_eq!(s, json!(r#""hello""#), "a bare string is not valid JSON");

        let mut o = json!({"k": 1});
        fit(&mut o, &f);
        assert_eq!(o, json!(r#"{"k":1}"#));

        let mut looks_like = json!(r#"{"k":1}"#);
        fit(&mut looks_like, &f);
        assert_ne!(looks_like, o, "a string holding JSON is not that object");
        assert_eq!(looks_like, json!(r#""{\"k\":1}""#));

        let mut n = json!(null);
        fit(&mut n, &f);
        assert_eq!(n, json!(null), "null is absent, not the text 'null'");
    }

    /// Scalars in a text slot become their JSON text too. Arrow would only do this with
    /// `coerce_primitive`, which DataFusion never sets — `sample/config.json` hits it as
    /// `expected string got false`.
    #[test]
    fn scalars_in_a_text_slot_become_their_json_text() {
        for (mut v, want) in [
            (json!(true), "true"),
            (json!(false), "false"),
            (json!(42), "42"),
            (json!(1.5), "1.5"),
        ] {
            fit(&mut v, &utf8("c"));
            assert_eq!(v, Value::String(want.to_string()));
        }
    }

    /// Null stays null. The column is nullable, and "no value" is not the four characters
    /// `null` — writing those would make an absent field indistinguishable from a present one
    /// holding that string.
    #[test]
    fn null_stays_null_in_a_text_slot() {
        let mut v = json!(null);
        fit(&mut v, &utf8("c"));
        assert_eq!(v, json!(null));
    }

    /// The real shape: a conflicted key inside a struct, stringified without disturbing the
    /// typed siblings around it.
    #[test]
    fn a_nested_conflict_is_reached_and_its_siblings_are_not() {
        let target = DataType::Struct(Fields::from(vec![
            Field::new("id", DataType::Int64, true),
            utf8("content"),
        ]));
        let mut v = json!({"id": 7, "content": {"kind": "image"}});
        fit(&mut v, &plain(target));
        assert_eq!(v["id"], json!(7), "a typed sibling is untouched");
        assert_eq!(v["content"], json!(r#"{"kind":"image"}"#));
    }

    /// `sample/config.json`'s actual case: a list of structs whose `content` disagrees per
    /// element. Every element is walked, and each keeps its own text.
    #[test]
    fn every_element_of_a_list_of_structs_is_walked() {
        let elem = DataType::Struct(Fields::from(vec![utf8("content")]));
        let target = DataType::List(Arc::new(Field::new_list_field(elem, true)));
        let mut v = json!([
            {"content": "text"},
            {"content": {"kind": "image"}},
            {"content": ["a", "b"]},
        ]);
        fit(&mut v, &plain(target));
        assert_eq!(v[0]["content"], json!("text"));
        assert_eq!(v[1]["content"], json!(r#"{"kind":"image"}"#));
        assert_eq!(v[2]["content"], json!(r#"["a","b"]"#));
    }

    /// Recursion has to survive several levels, which is the whole point for a content tree:
    /// `content.content[].content` is three hops down.
    #[test]
    fn the_walk_reaches_a_recursive_content_tree() {
        let leaf = DataType::Struct(Fields::from(vec![utf8("content")]));
        let list = DataType::List(Arc::new(Field::new_list_field(leaf, true)));
        let mid = DataType::Struct(Fields::from(vec![Field::new("content", list, true)]));
        let target = DataType::Struct(Fields::from(vec![Field::new("content", mid, true)]));

        let mut v = json!({"content": {"content": [{"content": {"deep": true}}]}});
        fit(&mut v, &plain(target));
        assert_eq!(
            v["content"]["content"][0]["content"],
            json!(r#"{"deep":true}"#)
        );
    }

    /// Arrow's scalar↔array promotion, finished on the read side. Inference says `List<Utf8>`
    /// for a key that is sometimes `"x"` and sometimes `["y"]`; without this the decoder reports
    /// `expected [ got "x"` on the very schema arrow itself inferred.
    #[test]
    fn a_scalar_against_a_list_target_is_wrapped() {
        let target = DataType::List(Arc::new(Field::new_list_field(DataType::Utf8, true)));
        let mut v = json!("N0004768");
        fit(&mut v, &plain(target.clone()));
        assert_eq!(v, json!(["N0004768"]));

        let mut v = json!(["a", "b"]);
        fit(&mut v, &plain(target));
        assert_eq!(v, json!(["a", "b"]));
    }

    /// A null list is **absent**, not a list holding one null. Wrapping it would turn every
    /// missing list into a one-element list and change every count over that column.
    #[test]
    fn a_null_against_a_list_target_is_not_wrapped() {
        let target = DataType::List(Arc::new(Field::new_list_field(DataType::Utf8, true)));
        let mut v = json!(null);
        fit(&mut v, &plain(target));
        assert_eq!(v, json!(null));
    }

    /// The wrap composes with the stringify: a scalar promoted into a list whose element type
    /// is text still gets its text treatment.
    #[test]
    fn a_wrapped_scalar_still_gets_its_element_treatment() {
        let target = DataType::List(Arc::new(Field::new_list_field(DataType::Utf8, true)));
        let mut v = json!(7);
        fit(&mut v, &plain(target));
        assert_eq!(v, json!(["7"]));
    }

    /// A value that does not fit a non-text target is deliberately left alone — see [`fit`].
    /// Nulling it here would hide the problem; arrow's error names the field.
    #[test]
    fn a_misfit_against_a_typed_target_is_left_for_arrow() {
        let mut v = json!({"n": {"unexpected": true}});
        let fields = Fields::from(vec![Field::new("n", DataType::Int64, true)]);
        fit_record(&mut v, &fields);
        assert_eq!(v["n"], json!({"unexpected": true}));
    }

    /// Keys the schema does not name are left in place. Arrow skips them itself
    /// (`strict_mode` is off), and removing them here would mean walking every record twice.
    #[test]
    fn keys_absent_from_the_schema_are_ignored() {
        let mut v = json!({"known": {"a": 1}, "extra": {"b": 2}});
        fit_record(&mut v, &Fields::from(vec![utf8("known")]));
        assert_eq!(v["known"], json!(r#"{"a":1}"#));
        assert_eq!(v["extra"], json!({"b": 2}), "untouched, not removed");
    }
}
