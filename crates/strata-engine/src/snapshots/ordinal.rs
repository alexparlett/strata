//! The ordinal column — the written result order every ordered read sorts by
//! (`docs/SNAPSHOT_SPEC.md` §9).
//!
//! Contract law rather than one store's habit: a snapshot read has no order of its own, so any
//! store that claims to serve stable pages has to number the rows it is handed, in the order it
//! is handed them.

use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, UInt64Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::record_batch::RecordBatch;

/// The unescalated ordinal column name (`docs/SNAPSHOT_SPEC.md` §9).
const ORDINAL_BASE: &str = "__strata_ord";

/// The name the snapshot's ordinal column gets: `__strata_ord`, prefix-escalated until it
/// collides with nothing in the result. Result column names come out of the user's own
/// query and can be anything, including this one.
pub fn ordinal_name(schema: &Schema) -> String {
    let mut name = String::from(ORDINAL_BASE);
    while schema.fields().iter().any(|f| f.name() == &name) {
        name.insert(0, '_');
    }
    name
}

/// The spooled schema: the result's own, with the ordinal appended last.
///
/// `UInt64` and non-nullable, which is what the plan-level `row_number()` this replaced
/// produced — so the file's shape, and every reader's view of it, is unchanged.
pub fn ordinal_schema(schema: &SchemaRef, ord: &str) -> SchemaRef {
    let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
    fields.push(Field::new(ord, DataType::UInt64, false));
    Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()))
}

/// `batch` with the ordinal appended, numbering its rows from `written` — the count already
/// spooled, so the value is the row's position in the snapshot.
///
/// **1-based**, which is what the `row_number()` this replaced produced. Nothing reads the
/// values (every reader only orders by them), but the shape is described in
/// `docs/SNAPSHOT_SPEC.md` §9 and there is no reason to make that description false.
///
/// One allocation per batch, of a contiguous range — as cheap as an Arrow column gets; the
/// user's own columns are carried over by reference.
pub fn with_ordinal(
    batch: &RecordBatch,
    schema: &SchemaRef,
    written: u64,
) -> Result<RecordBatch, String> {
    let first = written + 1;
    let mut columns: Vec<ArrayRef> = batch.columns().to_vec();
    columns.push(Arc::new(UInt64Array::from_iter_values(
        first..first + batch.num_rows() as u64,
    )));
    RecordBatch::try_new(Arc::clone(schema), columns).map_err(|e| e.to_string())
}
