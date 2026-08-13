//! Strata's own SQL built-ins (QE-01): the four functions that make an object-keyed `Struct`
//! enumerable and walkable.
//!
//! [`json_poly`](super::json_poly) infers **every** JSON object as a `Struct`, keys unioned across
//! the file — there is no `Map` arm anywhere — so the shape a config document actually has (an
//! object keyed by UUID, thousands of same-shaped values under it) arrives as a struct with
//! thousands of fields, and DataFusion's struct vocabulary is entirely literal: `get_field` and dot
//! access take a key written into the SQL. Nothing in the built-in set answers "which keys does
//! **this row** have", and nothing indexes by a key computed from elsewhere in the row.
//!
//! The four are Arrow-side first, JSON text only as the fallback:
//!
//! - [`StructKeys`] reads the null bitmaps. No value is touched and nothing is serialized, which
//!   is what makes it affordable on the wide struct that motivated it.
//! - [`StructEntries`] pairs each key with its value, still typed Arrow, so
//!   `unnest(struct_entries(s))` walks the map end to end and ordinary struct access continues
//!   from there.
//! - [`StructGet`] indexes by a **computed** key, which is the one thing `get_field` cannot do.
//! - [`ToJson`] serializes any value to JSON text — the escape hatch the two `V`-typed functions
//!   refuse toward, and the input `datafusion-functions-json`'s accessors speak.
//!
//! **The shape rule, and why only two of them have it.** `struct_entries` and `struct_get` return
//! *one* Arrow type per call, so they need the struct's fields to agree on one; a heterogeneous
//! struct is refused **at planning time**, by name, pointing at `to_json`. `struct_keys` has no
//! such requirement (keys are keys) and neither does `to_json` (text is text).
//!
//! **What a null means here.** `json_poly` unions keys across the file, so a key absent from a
//! record is a *null field* in that row — which is what makes the null-bitmap read the honest
//! per-row answer. It also means a source-level explicit `null` and an absent key are the same
//! Arrow null and cannot be told apart. Each function's description says so, in the same words,
//! and the loss is at inference rather than here: the spike below measured `cast_to_variant` — a
//! format that *can* carry the difference — dropping the null fields too.
//!
//! **A key, not a path.** `struct_get` takes one key and matches it exactly; two levels down is
//! two calls. `datafusion-variant` took the other road, and its own path parser documents where
//! that ends: it carries a `List` overload specifically "for keys that contain dots such as OTEL
//! attribute keys like `http.response.status_code`". A key in a keyed map is data, not an
//! expression.
//!
//! **Why these and not [`datafusion-variant`](https://github.com/datafusion-contrib/datafusion-variant)**
//! (the spike this task opened with; the evidence is in `.claude/tasks/`). Its published release
//! resolves a *second* DataFusion into the graph — 52 and arrow 57, beside our 54 and 58.3 — and
//! carries no key-enumeration function at all; only its unreleased git HEAD builds against our
//! pin. Against the fixture that HEAD does work, computed path and per-row keys included, with no
//! shape-unification rule — a capability these four genuinely lack. What decided it is the result
//! side and the cost: a Variant column reaches the app as
//! `Struct{metadata: BinaryView, value: BinaryView}`, which the grid, the inspector and export
//! each render as hex, and a keys-only read of a 5,000-key struct measured 58.7ms against 19.75µs
//! for the bitmap walk. These four return `List`, `Struct` and `Utf8` — types every one of those
//! readers already has an arm for, which is a requirement and not a coincidence.

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::array::{
    new_null_array, Array, ArrayBuilder, ArrayRef, AsArray, ListArray, NullBufferBuilder,
    StringArray, StringBuilder, StructArray,
};
use datafusion::arrow::buffer::{NullBuffer, OffsetBuffer};
use datafusion::arrow::compute::interleave;
use datafusion::arrow::compute::kernels::cast::cast;
use datafusion::arrow::datatypes::{DataType, Field, FieldRef, Fields};
use datafusion::arrow::error::ArrowError;
use datafusion::arrow::json::writer::{
    make_encoder, Encoder, EncoderFactory, EncoderOptions, NullableEncoder,
};
use datafusion::common::{exec_err, internal_datafusion_err, plan_err};
use datafusion::error::Result;
use datafusion::execution::FunctionRegistry;
use datafusion::logical_expr::scalar_doc_sections::DOC_SECTION_STRUCT;
use datafusion::logical_expr::{
    ColumnarValue, Documentation, ReturnFieldArgs, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl,
    Signature, Volatility,
};
use datafusion::prelude::SessionContext;

use crate::engine::json_poly::infer::JSON_TEXT_KEY;

/// The sentence every one of these functions ends with. One wording, because it is one fact:
/// `json_poly` unions a file's keys, so both an absent key and a source-level `null` arrive as an
/// Arrow null and nothing downstream can separate them.
const NULL_NOTE: &str =
    "A key whose value is null in the source is indistinguishable from a key the row does not have.";

/// Register Strata's built-ins into `ctx`.
///
/// Called from `build_context` beside `datafusion_functions_json::register_all`, for the same
/// reason and on the same terms: these belong to the engine rather than to a table, and one call
/// is the whole integration — `functions::snapshot` walks the live registry, so completion,
/// signature detail, the docs panel and the agent's `list_functions` follow with no further wiring.
///
/// A name already in the registry is **shadowed silently** by `SessionContext::register_udf`,
/// which returns nothing, so the registry is asked first: taking a DataFusion built-in's name
/// would replace it for the whole session with no way to put it back. Warned rather than fatal for
/// the same reason the JSON crate's registration is: the failure names itself on the first query
/// that wanted the function.
pub fn register(ctx: &SessionContext) {
    let udfs = [
        ScalarUDF::from(StructKeys::new()),
        ScalarUDF::from(StructEntries::new()),
        ScalarUDF::from(StructGet::new()),
        ScalarUDF::from(ToJson::new()),
    ];
    for udf in udfs {
        let name = udf.name().to_string();
        if ctx.udf(&name).is_ok() {
            tracing::warn!("engine: '{name}' is already registered and will be replaced");
        }
        ctx.register_udf(udf);
    }
}

/// The fields of a struct argument, or the plan-time refusal for anything else.
///
/// Named `function(argument_type)` rather than "expected struct": the call that lands here is
/// usually a path one level off (`struct_keys(cb.content)` where `content` is a list), and the type
/// is what says which.
fn struct_fields<'a>(function: &str, dtype: &'a DataType) -> Result<&'a Fields> {
    match dtype {
        DataType::Struct(fields) => Ok(fields),
        other => plan_err!("'{function}' takes a struct, not {other}"),
    }
}

/// The one value **field** a struct's values share, or the plan-time refusal naming the two that
/// disagree and the way out.
///
/// **Nullability is not a disagreement.** Two fields that differ only in whether they admit nulls
/// hold the same values, so they unify to the more permissive of the two — recursively, because a
/// `Struct`'s and a `List`'s nullability live *inside* their `DataType`. Anything else is a genuine
/// difference with no single Arrow answer, and the caller is sent to [`ToJson`], which has one.
///
/// A **field**, not a type, because [`JSON_TEXT_KEY`] rides on the field: a keyed object whose
/// values are all conflict-state text is one struct where the mark is the difference between
/// `to_json(struct_get(…))` handing back a document and handing back a document inside a string.
/// What survives is what every field agrees on, which is the only thing that can honestly be said
/// of the merged value.
fn unified_value_field(function: &str, fields: &Fields) -> Result<FieldRef> {
    let dtype = unified_value_type(function, fields)?;
    let mut metadata = fields
        .first()
        .map(|f| f.metadata().clone())
        .unwrap_or_default();
    for field in fields.iter().skip(1) {
        metadata.retain(|key, value| field.metadata().get(key) == Some(value));
    }
    Ok(Arc::new(
        Field::new("value", dtype, true).with_metadata(metadata),
    ))
}

/// The merged type alone.
///
/// The refusal names the offending field and calls the accumulated type "the values before it",
/// attributing it to no single key: once a `Null` field has widened into a later one's type, the
/// running type belongs to none of them, and naming the first key beside it sends the reader to a
/// key that does not have it.
///
/// An empty struct settles on `Null` — the type arrow gives a column it has never seen a value
/// for, and the honest one for the empty entry list this produces. `json_poly` infers `{}` as
/// exactly that, and the document behind this family has 19,159 of them.
fn unified_value_type(function: &str, fields: &Fields) -> Result<DataType> {
    let mut settled: Option<DataType> = None;
    for field in fields {
        settled = match settled {
            None => Some(field.data_type().clone()),
            Some(dtype) => match merge(&dtype, field.data_type()) {
                Some(merged) => Some(merged),
                None => {
                    return plan_err!(
                        "'{function}' needs a struct whose values share one type, but '{}' is {} \
                         and the values before it are {dtype}. Use to_json to read this value as \
                         JSON text.",
                        field.name(),
                        field.data_type()
                    )
                }
            },
        };
    }
    Ok(settled.unwrap_or(DataType::Null))
}

/// Two types that hold the same values, merged to the one that admits the most nulls; `None` when
/// they genuinely differ.
///
/// `Null` is every other type's identity element here: a key seen only as JSON `null` infers as
/// it, and carries no values to disagree about.
fn merge(a: &DataType, b: &DataType) -> Option<DataType> {
    if a == b {
        return Some(a.clone());
    }
    match (a, b) {
        (DataType::Null, other) | (other, DataType::Null) => Some(other.clone()),
        (DataType::Struct(left), DataType::Struct(right)) if left.len() == right.len() => {
            let fields = left
                .iter()
                .zip(right)
                .map(|(l, r)| (l.name() == r.name()).then(|| merge_field(l, r))?)
                .collect::<Option<Vec<FieldRef>>>()?;
            Some(DataType::Struct(fields.into()))
        }
        (DataType::List(left), DataType::List(right)) => {
            Some(DataType::List(merge_field(left, right)?))
        }
        _ => None,
    }
}

/// Two fields merged: the type by [`merge`], nullability by either, and the metadata **kept where
/// both agree**.
///
/// Rebuilding a nested field with `Field::new` alone starts it from empty metadata, which drops
/// the [`JSON_TEXT_KEY`] mark off a conflict-state child — and `to_json` one step later would
/// then encode that subtree as an ordinary string, the double-encoding the passthrough exists to
/// prevent. Same rule as [`unified_value_field`]: what survives is what both sides say.
fn merge_field(left: &FieldRef, right: &FieldRef) -> Option<FieldRef> {
    let dtype = merge(left.data_type(), right.data_type())?;
    let mut metadata = left.metadata().clone();
    metadata.retain(|key, value| right.metadata().get(key) == Some(value));
    Some(Arc::new(
        Field::new(
            left.name(),
            dtype,
            left.is_nullable() || right.is_nullable(),
        )
        .with_metadata(metadata),
    ))
}

/// Which rows of `child` hold a value, resolved **once** per child.
///
/// `Array::is_null` is the wrong question for a `Null`-typed child: its nulls are logical, there is
/// no bitmap to consult, and it answers `false` for every index — so a key seen only as JSON `null`
/// would read as present in every row, including the rows that never mentioned it.
/// `logical_nulls` is the question that covers both, and it allocates, so it is asked per child
/// rather than per cell.
fn validity(child: &ArrayRef) -> Option<NullBuffer> {
    child.logical_nulls()
}

fn valid_at(validity: &Option<NullBuffer>, row: usize) -> bool {
    validity.as_ref().is_none_or(|nulls| nulls.is_valid(row))
}

/// The struct argument as an array, whatever form the call passed it in.
fn struct_arg(args: &ScalarFunctionArgs, function: &str) -> Result<ArrayRef> {
    let value = args
        .args
        .first()
        .ok_or_else(|| internal_datafusion_err!("'{function}' takes an argument"))?;
    let array = value.to_array(args.number_rows)?;
    match array.data_type() {
        DataType::Struct(_) => Ok(array),
        other => exec_err!("'{function}' takes a struct, not {other}"),
    }
}

/// `struct_keys(struct) -> List<Utf8>` — the keys this row has.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct StructKeys {
    signature: Signature,
    documentation: Documentation,
}

/// The list field both `struct_keys` and the entries list are built around. A key is never null,
/// which the item field says so the array a caller collects matches the type they planned against.
fn keys_field() -> FieldRef {
    Arc::new(Field::new("item", DataType::Utf8, false))
}

impl StructKeys {
    pub fn new() -> StructKeys {
        StructKeys {
            signature: Signature::any(1, Volatility::Immutable),
            documentation: Documentation::builder(
                DOC_SECTION_STRUCT,
                format!(
                    "Returns the keys the struct has in this row, as a list of strings. Reads the \
                     null bitmaps: no value is read and nothing is serialized. {NULL_NOTE}"
                ),
                "struct_keys(struct)",
            )
            .build(),
        }
    }
}

impl ScalarUDFImpl for StructKeys {
    fn name(&self) -> &str {
        "struct_keys"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        struct_fields(self.name(), &args[0])?;
        Ok(DataType::List(keys_field()))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let array = struct_arg(&args, self.name())?;
        let structs = array.as_struct();
        let validity: Vec<_> = structs.columns().iter().map(validity).collect();

        let mut keys = StringBuilder::new();
        let mut offsets: Vec<i32> = Vec::with_capacity(structs.len() + 1);
        offsets.push(0);
        let mut nulls = NullBufferBuilder::new(structs.len());
        for row in 0..structs.len() {
            if structs.is_null(row) {
                nulls.append_null();
                offsets.push(keys.len() as i32);
                continue;
            }
            for (field, valid) in structs.fields().iter().zip(&validity) {
                if valid_at(valid, row) {
                    keys.append_value(field.name());
                }
            }
            nulls.append_non_null();
            offsets.push(keys.len() as i32);
        }
        let list = ListArray::try_new(
            keys_field(),
            OffsetBuffer::new(offsets.into()),
            Arc::new(keys.finish()),
            nulls.finish(),
        )?;
        Ok(ColumnarValue::Array(Arc::new(list)))
    }

    fn documentation(&self) -> Option<&Documentation> {
        Some(&self.documentation)
    }
}

/// `struct_entries(struct) -> List<Struct{key, value}>` — the row's keys with their values, still
/// typed Arrow.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct StructEntries {
    signature: Signature,
    documentation: Documentation,
}

/// The entry struct for a settled value field. Built in one place because it is the return type
/// *and* the array's own type, and a mismatch between those two is a plan-level fault.
fn entry_field(value: FieldRef) -> FieldRef {
    Arc::new(Field::new(
        "item",
        DataType::Struct(vec![Arc::new(Field::new("key", DataType::Utf8, false)), value].into()),
        false,
    ))
}

impl StructEntries {
    pub fn new() -> StructEntries {
        StructEntries {
            signature: Signature::any(1, Volatility::Immutable),
            documentation: Documentation::builder(
                DOC_SECTION_STRUCT,
                format!(
                    "Returns the struct's keys and values for this row, as a list of \
                     'key'/'value' structs with the values still typed. Needs a struct whose \
                     values share one type; use to_json for one that mixes them. {NULL_NOTE}"
                ),
                "struct_entries(struct)",
            )
            .build(),
        }
    }
}

impl ScalarUDFImpl for StructEntries {
    fn name(&self) -> &str {
        "struct_entries"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let fields = struct_fields(self.name(), &args[0])?;
        let value = unified_value_field(self.name(), fields)?;
        Ok(DataType::List(entry_field(value)))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let DataType::List(entry) = args.return_type().clone() else {
            return exec_err!("'{}' returns a list", self.name());
        };
        let DataType::Struct(pair) = entry.data_type().clone() else {
            return exec_err!("'{}' returns a list of key/value structs", self.name());
        };
        let value_type = pair[1].data_type().clone();

        let array = struct_arg(&args, self.name())?;
        let structs = array.as_struct();
        let children = unify_children(structs, &value_type)?;
        let sources: Vec<&dyn Array> = children.iter().map(AsRef::as_ref).collect();
        let validity: Vec<_> = structs.columns().iter().map(validity).collect();

        let mut keys = StringBuilder::new();
        let mut picks: Vec<(usize, usize)> = Vec::new();
        let mut offsets: Vec<i32> = Vec::with_capacity(structs.len() + 1);
        offsets.push(0);
        let mut nulls = NullBufferBuilder::new(structs.len());
        for row in 0..structs.len() {
            if structs.is_null(row) {
                nulls.append_null();
                offsets.push(picks.len() as i32);
                continue;
            }
            for (child, (field, valid)) in structs.fields().iter().zip(&validity).enumerate() {
                if valid_at(valid, row) {
                    keys.append_value(field.name());
                    picks.push((child, row));
                }
            }
            nulls.append_non_null();
            offsets.push(picks.len() as i32);
        }
        let values = gather(&sources, &value_type, &picks)?;
        let entries = StructArray::try_new(pair, vec![Arc::new(keys.finish()), values], None)?;
        let list = ListArray::try_new(
            entry,
            OffsetBuffer::new(offsets.into()),
            Arc::new(entries),
            nulls.finish(),
        )?;
        Ok(ColumnarValue::Array(Arc::new(list)))
    }

    fn documentation(&self) -> Option<&Documentation> {
        Some(&self.documentation)
    }
}

/// Every child as the settled value type. A child whose type already *is* it — the common case,
/// since `json_poly` builds a keyed object's values from one inference — is handed over untouched.
fn unify_children(structs: &StructArray, value: &DataType) -> Result<Vec<ArrayRef>> {
    structs
        .columns()
        .iter()
        .map(|child| match child.data_type() == value {
            true => Ok(Arc::clone(child)),
            false => Ok(cast(child, value)?),
        })
        .collect()
}

/// `picks` gathered out of `sources`, as one array of `value` type.
///
/// `interleave` refuses an empty source list, and a struct with no fields has one — so the empty
/// gather is answered directly rather than by special-casing the caller.
fn gather(sources: &[&dyn Array], value: &DataType, picks: &[(usize, usize)]) -> Result<ArrayRef> {
    if sources.is_empty() || picks.is_empty() {
        return Ok(new_null_array(value, picks.len()));
    }
    Ok(interleave(sources, picks)?)
}

/// `struct_get(struct, key) -> V` — the value under a key computed at run time.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct StructGet {
    signature: Signature,
    documentation: Documentation,
}

impl StructGet {
    pub fn new() -> StructGet {
        StructGet {
            signature: Signature::any(2, Volatility::Immutable),
            documentation: Documentation::builder(
                DOC_SECTION_STRUCT,
                format!(
                    "Returns the struct's value under 'key', which may be computed per row -- \
                     unlike a dot path, which is written into the query. The key is matched \
                     exactly, and a key the row does not have gives null. Needs a struct whose \
                     values share one type; use to_json for one that mixes them. {NULL_NOTE}"
                ),
                "struct_get(struct, key)",
            )
            .build(),
        }
    }

    /// What one call returns, from the two argument types — the whole of this function's typing
    /// rule, stated once so `return_type` and `return_field_from_args` cannot come to disagree
    /// about which keys it takes.
    fn value_field(&self, structure: &DataType, key: &DataType) -> Result<FieldRef> {
        let fields = struct_fields(self.name(), structure)?;
        match key {
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => {}
            other => return plan_err!("'{}' takes a string key, not {other}", self.name()),
        }
        unified_value_field(self.name(), fields)
    }
}

impl ScalarUDFImpl for StructGet {
    fn name(&self) -> &str {
        "struct_get"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        Ok(self.value_field(&args[0], &args[1])?.data_type().clone())
    }

    /// The default would rebuild the field from [`return_type`](Self::return_type) and drop its
    /// metadata; here the value *is* the return, so a [`JSON_TEXT_KEY`] mark the whole struct
    /// carries has to survive the call or `to_json` one step later re-quotes the document.
    fn return_field_from_args(&self, args: ReturnFieldArgs) -> Result<FieldRef> {
        let value = self.value_field(
            args.arg_fields[0].data_type(),
            args.arg_fields[1].data_type(),
        )?;
        Ok(Arc::new(value.as_ref().clone().with_name(self.name())))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let value_type = args.return_type().clone();
        let array = struct_arg(&args, self.name())?;
        let structs = array.as_struct();

        let keys = args.args[1].to_array(args.number_rows)?;
        let keys = cast(&keys, &DataType::Utf8)?;
        let keys: &StringArray = keys.as_string();

        let position: HashMap<&str, usize> = structs
            .fields()
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name().as_str(), i))
            .collect();
        let children = unify_children(structs, &value_type)?;
        let missing = new_null_array(&value_type, 1);
        let mut sources: Vec<&dyn Array> = children.iter().map(AsRef::as_ref).collect();
        sources.push(missing.as_ref());
        let absent = (sources.len() - 1, 0);
        let validity: Vec<_> = structs.columns().iter().map(validity).collect();

        let picks: Vec<(usize, usize)> = (0..structs.len())
            .map(|row| {
                if structs.is_null(row) || keys.is_null(row) {
                    return absent;
                }
                match position.get(keys.value(row)) {
                    Some(&child) if valid_at(&validity[child], row) => (child, row),
                    _ => absent,
                }
            })
            .collect();
        Ok(ColumnarValue::Array(interleave(&sources, &picks)?))
    }

    fn documentation(&self) -> Option<&Documentation> {
        Some(&self.documentation)
    }
}

/// `to_json(value) -> Utf8` — any value as JSON text.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct ToJson {
    signature: Signature,
    documentation: Documentation,
}

impl ToJson {
    pub fn new() -> ToJson {
        ToJson {
            signature: Signature::any(1, Volatility::Immutable),
            documentation: Documentation::builder(
                DOC_SECTION_STRUCT,
                format!(
                    "Returns the value as JSON text -- a struct, list or scalar, however deeply \
                     nested, and however much its parts disagree in type. Null in, null out; a \
                     null field is omitted from its object. The result is plain text, which is \
                     what the json_get family reads. {NULL_NOTE}"
                ),
                "to_json(value)",
            )
            .build(),
        }
    }
}

impl ScalarUDFImpl for ToJson {
    fn name(&self) -> &str {
        "to_json"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Utf8)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let field = args
            .arg_fields
            .first()
            .cloned()
            .ok_or_else(|| internal_datafusion_err!("'to_json' takes an argument"))?;
        let array = args.args[0].to_array(args.number_rows)?;
        let options = EncoderOptions::default().with_encoder_factory(Arc::new(JsonTextVerbatim));
        let mut encoder = make_encoder(&field, array.as_ref(), &options)?;
        let mut text = StringBuilder::new();
        let mut buffer: Vec<u8> = Vec::new();
        for row in 0..array.len() {
            if encoder.is_null(row) {
                text.append_null();
                continue;
            }
            buffer.clear();
            encoder.encode(row, &mut buffer);
            match std::str::from_utf8(&buffer) {
                Ok(json) => text.append_value(json),
                Err(e) => return exec_err!("'to_json' produced invalid UTF-8: {e}"),
            }
        }
        Ok(ColumnarValue::Array(Arc::new(text.finish())))
    }

    fn documentation(&self) -> Option<&Documentation> {
        Some(&self.documentation)
    }
}

/// A `Utf8` column that already holds JSON text is written **through**, not quoted.
///
/// [`json_poly`](super::json_poly) carries a type-conflicted key as text marked
/// [`JSON_TEXT_KEY`], and every row of such a column is valid JSON by construction
/// (`json_poly::normalize` is what makes it so). Encoding it as an ordinary string would put the
/// document back inside a string literal, one escape level down, and the `json_get` family would
/// then need a parse to reach what `to_json` was called to expose.
#[derive(Debug)]
struct JsonTextVerbatim;

impl EncoderFactory for JsonTextVerbatim {
    fn make_default_encoder<'a>(
        &self,
        field: &'a FieldRef,
        array: &'a dyn Array,
        _options: &'a EncoderOptions,
    ) -> std::result::Result<Option<NullableEncoder<'a>>, ArrowError> {
        if !field.metadata().contains_key(JSON_TEXT_KEY) {
            return Ok(None);
        }
        let DataType::Utf8 = array.data_type() else {
            return Ok(None);
        };
        let nulls = array.logical_nulls();
        Ok(Some(NullableEncoder::new(
            Box::new(Verbatim(array.as_string::<i32>())),
            nulls,
        )))
    }
}

struct Verbatim<'a>(&'a StringArray);

impl Encoder for Verbatim<'_> {
    /// An empty cell writes `null` rather than nothing.
    ///
    /// `json_poly::normalize` never produces one — every value it marks is at least `""`, two
    /// bytes — but the mark travels on the field, through an IPC snapshot and into whatever else
    /// carries that metadata, and a zero-byte write here lands in the middle of an object as
    /// `{"note":}`. A document that does not parse is the one failure this function must not have,
    /// since its whole purpose is to feed a parser.
    fn encode(&mut self, idx: usize, out: &mut Vec<u8>) {
        match self.0.value(idx) {
            "" => out.extend_from_slice(b"null"),
            json => out.extend_from_slice(json.as_bytes()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process;

    use strata_model::{JsonRead, QueryOutput, SourceFormat};

    use crate::engine::{Engine, RunTag, TableSpec, WsId};

    /// A JSON fixture registered as `t` through [`json_poly`](crate::engine::json_poly) — the
    /// reader every one of these functions exists for, so the structs under test are the ones the
    /// app actually produces (keys unioned across records, absent keys as null fields) rather than
    /// ones a `named_struct` literal built to suit the assertion.
    async fn fixture(name: &str, body: &str) -> Engine {
        let dir = std::env::temp_dir().join(format!("strata_udfs_{}_{name}", process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path: PathBuf = dir.join("t.json");
        std::fs::write(&path, body).expect("fixture");

        let engine = Engine::new(Default::default());
        engine
            .register(TableSpec {
                name: "t".into(),
                paths: vec![path.to_string_lossy().into_owned()],
                format: SourceFormat::Json(JsonRead::default()),
                partitions: Vec::new(),
                internal: false,
            })
            .await
            .expect("register");
        engine
    }

    async fn run(engine: &Engine, sql: &str) -> QueryOutput {
        engine
            .query(WsId(1), RunTag(1), sql.into(), 50)
            .await
            .unwrap_or_else(|e| panic!("{sql}\n{e}"))
            .0
    }

    async fn fails(engine: &Engine, sql: &str) -> String {
        engine
            .query(WsId(1), RunTag(1), sql.into(), 50)
            .await
            .err()
            .unwrap_or_else(|| panic!("{sql} was expected to fail"))
    }

    /// One column of a result as display text, nulls as `NULL` so an absent answer is visible.
    fn column(out: &QueryOutput, col: usize) -> Vec<String> {
        out.rows
            .iter()
            .map(|r| match r[col].null {
                true => "NULL".to_string(),
                false => r[col].text.clone(),
            })
            .collect()
    }

    /// A keyed object of same-shaped values, exactly the reported shape: the keys differ per
    /// record, and `pick` names one of them.
    const KEYED: &str = concat!(
        r#"{"id":1,"pick":"b2","cb":{"a1":{"kind":"heading","weight":1},"b2":{"kind":"body","weight":2}}}"#,
        "\n",
        r#"{"id":2,"pick":"zz","cb":{"c3":{"kind":"body","weight":3}}}"#,
        "\n",
    );

    /// **The keys are this row's**, not the file's — which is the whole reason the function reads
    /// the null bitmaps rather than the field list. `json_poly` unions `a1`/`b2`/`c3` across the
    /// two records, so every row's struct has all three fields and two of them are null.
    #[tokio::test]
    async fn keys_are_the_ones_this_row_has() {
        let engine = fixture("keys", KEYED).await;
        let out = run(
            &engine,
            "SELECT array_to_string(struct_keys(cb), ',') FROM t ORDER BY id",
        )
        .await;
        assert_eq!(column(&out, 0), vec!["a1,b2", "c3"]);
    }

    /// A null struct is "no object"; an object with no keys is `[]`. Two different answers, and
    /// collapsing them would make `struct_keys(x) IS NULL` mean nothing.
    #[tokio::test]
    async fn a_null_struct_and_an_empty_one_answer_differently() {
        let engine = fixture(
            "empty",
            concat!(
                r#"{"id":1,"cb":{"a":1}}"#,
                "\n",
                r#"{"id":2,"cb":{}}"#,
                "\n",
                r#"{"id":3}"#,
                "\n",
            ),
        )
        .await;
        let out = run(
            &engine,
            "SELECT struct_keys(cb) IS NULL, array_length(struct_keys(cb)) FROM t ORDER BY id",
        )
        .await;
        assert_eq!(column(&out, 0), vec!["false", "false", "true"]);
        assert_eq!(column(&out, 1)[0], "1");
    }

    /// **The values stay typed**: `unnest` walks the map and the entry's own fields are reached by
    /// ordinary struct access, with no JSON text anywhere in the plan.
    ///
    /// Two spellings are load-bearing and both are upstream limitations (workstream README, ledger
    /// items 5 and 8): the `unnest` goes in the **projection** of a subquery, because `UNNEST` in
    /// `FROM` cannot reference a nested outer column, and the alias is indexed with **brackets**,
    /// because dot access on an unnested struct alias fails to qualify.
    #[tokio::test]
    async fn entries_keep_their_arrow_types_through_unnest() {
        let engine = fixture("entries", KEYED).await;
        let out = run(
            &engine,
            "SELECT e['key'] AS k, e['value']['weight'] AS w \
             FROM (SELECT unnest(struct_entries(cb)) AS e FROM t) ORDER BY k",
        )
        .await;
        assert_eq!(column(&out, 0), vec!["a1", "b2", "c3"]);
        assert_eq!(column(&out, 1), vec!["1", "2", "3"]);
        assert_eq!(
            out.columns[1].dtype, "Int64",
            "the value arrived as a number, not as text to re-parse"
        );
    }

    /// The headline of the report: a key **computed per row**, which `get_field` and dot access
    /// cannot take. A key the row does not have is null, the JSON accessors' answer, not an error.
    #[tokio::test]
    async fn struct_get_takes_a_computed_key_and_nulls_an_unknown_one() {
        let engine = fixture("get", KEYED).await;
        let out = run(
            &engine,
            "SELECT struct_get(cb, pick)['kind'] FROM t ORDER BY id",
        )
        .await;
        assert_eq!(column(&out, 0), vec!["body", "NULL"]);
    }

    /// Keys are **data**, so they match exactly. Folding is what a SQL identifier gets, and a
    /// document with `Key` and `key` beside each other has two keys.
    #[tokio::test]
    async fn a_key_is_matched_exactly() {
        let engine = fixture("case", concat!(r#"{"id":1,"cb":{"Key":1,"key":2}}"#, "\n")).await;
        let out = run(
            &engine,
            "SELECT struct_get(cb, 'Key'), struct_get(cb, 'key'), struct_get(cb, 'KEY') FROM t",
        )
        .await;
        assert_eq!(column(&out, 0), vec!["1"]);
        assert_eq!(column(&out, 1), vec!["2"]);
        assert_eq!(column(&out, 2), vec!["NULL"]);
    }

    /// A struct whose values disagree has no single Arrow return type, so **both** `V`-typed
    /// functions refuse it before the query runs, in one wording, naming the way out.
    #[tokio::test]
    async fn a_mixed_struct_is_refused_at_planning_time_by_both() {
        let engine = fixture(
            "mixed",
            concat!(r#"{"id":1,"cb":{"a":{"k":"x"},"b":7}}"#, "\n"),
        )
        .await;
        for sql in [
            "SELECT struct_entries(cb) FROM t",
            "SELECT struct_get(cb, 'a') FROM t",
        ] {
            let err = fails(&engine, sql).await;
            assert!(err.contains("share one type"), "{sql}: {err}");
            assert!(err.contains("to_json"), "{sql}: {err}");
        }
        let out = run(
            &engine,
            "SELECT array_to_string(struct_keys(cb), ',') FROM t",
        )
        .await;
        assert_eq!(column(&out, 0), vec!["a,b"]);
    }

    /// `to_json` over the shapes it has to be total across, and the null rule: null in, null out
    /// (not the four characters `null`), and a field this row does not have is **absent** from the
    /// object rather than written back as `"k": null`.
    #[tokio::test]
    async fn to_json_covers_struct_list_scalar_and_null() {
        let engine = fixture("tojson", KEYED).await;
        let out = run(
            &engine,
            "SELECT to_json(cb), to_json(struct_keys(cb)), to_json(id), to_json(cb['a1']) \
             FROM t ORDER BY id",
        )
        .await;
        assert_eq!(
            column(&out, 0),
            vec![
                r#"{"a1":{"kind":"heading","weight":1},"b2":{"kind":"body","weight":2}}"#,
                r#"{"c3":{"kind":"body","weight":3}}"#,
            ],
            "an absent key is absent, not null"
        );
        assert_eq!(column(&out, 1), vec![r#"["a1","b2"]"#, r#"["c3"]"#]);
        assert_eq!(column(&out, 2), vec!["1", "2"]);
        assert_eq!(
            column(&out, 3),
            vec![r#"{"kind":"heading","weight":1}"#, "NULL"],
            "a null value serializes to null, not to the text 'null'"
        );
    }

    /// A column `json_poly` carries as **JSON text** is written through, not quoted again.
    ///
    /// Without the passthrough the document comes back one escape level down
    /// (`{"note":"{\"kind\":\"body\"}"}`), and the `json_get` family would need a parse to reach
    /// what `to_json` was called to expose.
    #[tokio::test]
    async fn json_text_passes_through_unquoted() {
        let engine = fixture(
            "text",
            concat!(
                r#"{"id":1,"wrap":{"note":"plain prose"}}"#,
                "\n",
                r#"{"id":2,"wrap":{"note":{"kind":"body"}}}"#,
                "\n",
            ),
        )
        .await;
        let out = run(
            &engine,
            "SELECT to_json(wrap), to_json(wrap['note']) FROM t ORDER BY id",
        )
        .await;
        assert_eq!(
            column(&out, 0),
            vec![r#"{"note":"plain prose"}"#, r#"{"note":{"kind":"body"}}"#,]
        );
        assert_eq!(
            column(&out, 1),
            vec![r#""plain prose""#, r#"{"kind":"body"}"#]
        );
    }

    /// **The recursive-CTE spelling that works** (workstream ledger item 4). `json_get_json`'s
    /// output carries `arrow.json` extension metadata, which fails union unification against plain
    /// `Utf8` with "field metadata differs"; `to_json` returns plain text with none, so the same
    /// query plans.
    ///
    /// Two spellings here are DataFusion 54's, not ours, and both were found by this test. The
    /// columns are named in the anchor's own projection rather than by a `w(v, d)` column list,
    /// which it parses but does not apply to the CTE's schema (the recursive term then fails to
    /// resolve `d` against `w."Int64(0)"`); and the outer statement reads the columns rather than
    /// `count(*)`, which reaches execution and fails with "project index 1 out of bounds, max
    /// field 0" — a projection pushed into the recursive node, unrelated to what is being tested.
    #[tokio::test]
    async fn to_json_unifies_against_plain_text_in_a_recursive_cte() {
        let engine = fixture("cte", KEYED).await;
        let out = run(
            &engine,
            "WITH RECURSIVE w AS ( \
               SELECT to_json(cb) AS v, 0 AS d FROM t \
               UNION ALL \
               SELECT 'done', d + 1 FROM w WHERE d < 1 \
             ) SELECT v, d FROM w ORDER BY d",
        )
        .await;
        assert_eq!(out.total, 4);
    }

    /// **The reported job, end to end**: enumerate the keys of a UUID-keyed map and reach each
    /// entry's own fields, in SQL, with no serialization in the plan.
    #[tokio::test]
    async fn the_keyed_map_walk_is_pure_sql() {
        let engine = fixture("walk", KEYED).await;
        let out = run(
            &engine,
            "SELECT e['key'] AS block, e['value']['kind'] AS kind \
             FROM (SELECT unnest(struct_entries(cb)) AS e FROM t) \
             WHERE e['value']['weight'] > 1 ORDER BY block",
        )
        .await;
        assert_eq!(column(&out, 0), vec!["b2", "c3"]);
        assert_eq!(column(&out, 1), vec!["body", "body"]);
        assert_eq!(
            out.columns[1].dtype, "Utf8",
            "the entry's own field, not a JSON document to parse"
        );

        let filtered = run(
            &engine,
            "SELECT count(*) FROM (SELECT unnest(struct_entries(cb)) AS e FROM t) \
             WHERE e['value']['weight'] > 1",
        )
        .await;
        assert_eq!(
            column(&filtered, 0),
            vec!["2"],
            "the predicate ran against a number, which is what having the Arrow type buys"
        );
    }

    /// All four reach the catalog the language service and the agent read, with their
    /// descriptions — the registry walk unedited.
    #[test]
    fn the_four_are_in_the_function_catalog() {
        let engine = Engine::new(Default::default());
        let catalog = engine.functions();
        for name in ["struct_keys", "struct_entries", "struct_get", "to_json"] {
            let sym = catalog
                .scalar
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("'{name}' is not in the catalog"));
            assert!(
                sym.description.as_ref().is_some_and(|d| !d.is_empty()),
                "'{name}' has no description"
            );
            assert!(
                !sym.created,
                "'{name}' is a built-in, not a created function"
            );
        }
    }
}
