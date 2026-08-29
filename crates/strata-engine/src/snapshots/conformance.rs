//! The contract every [`SnapshotStore`] is held to, run against each shipped store.
//!
//! One body, two callers: a seam with a single implementation is a seam only by intention, and
//! this is what stops the trait's law from quietly becoming whatever
//! [`LocalIpcSnapshotStore`](super::LocalIpcSnapshotStore) happens to do.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, Int32Array, Int64Array, StringArray, UInt64Array, UnionArray,
};
use datafusion::arrow::buffer::ScalarBuffer;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef, UnionFields, UnionMode};
use datafusion::arrow::record_batch::RecordBatch;
use strata_model::SnapshotId;

use super::{snapshot_name, LocalIpcSnapshotStore, MemSnapshotStore, SnapshotStore};
use crate::builder::test_context;

/// A result with a union column in it — the type parquet cannot write at all, and so the one
/// that says whether a store keeps a result's type or a coerced picture of it.
fn union_fields() -> UnionFields {
    UnionFields::try_new(
        vec![1_i8, 2],
        vec![
            Field::new("s", DataType::Utf8, true),
            Field::new("i", DataType::Int64, true),
        ],
    )
    .expect("two distinct type ids")
}

fn result_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("n", DataType::Int32, true),
        Field::new(
            "j",
            DataType::Union(union_fields(), UnionMode::Sparse),
            false,
        ),
    ]))
}

/// Three rows: one `n` null, and a union carrying a string, an int and a string.
fn batch(schema: &SchemaRef, first: i32) -> RecordBatch {
    let n: ArrayRef = Arc::new(Int32Array::from(vec![Some(first), None, Some(first + 2)]));
    let j: ArrayRef = Arc::new(
        UnionArray::try_new(
            union_fields(),
            ScalarBuffer::from(vec![1_i8, 2, 1]),
            None,
            vec![
                Arc::new(StringArray::from(vec![Some("a"), None, Some("c")])),
                Arc::new(Int64Array::from(vec![None, Some(7), None])),
            ],
        )
        .expect("a sparse union"),
    );
    RecordBatch::try_new(Arc::clone(schema), vec![n, j]).expect("a result batch")
}

/// Write → settle → open → read back → retire, asserted the same way whatever the store is
/// made of.
async fn the_contract(store: &dyn SnapshotStore) {
    let ctx = test_context(&BTreeMap::new());
    let schema = result_schema();
    let id = SnapshotId(1);

    let mut sink = store
        .begin(id, Arc::clone(&schema), Some("__strata_ord".into()))
        .expect("a write pass");
    sink.write(&batch(&schema, 1)).expect("first batch");
    sink.write(&batch(&schema, 4)).expect("second batch");
    let stats = sink.settle().expect("a settled snapshot");

    assert_eq!(
        stats.ord.as_deref(),
        Some("__strata_ord"),
        "the pass reports the ordinal it was asked for"
    );
    assert_eq!(
        stats.nulls,
        vec![2, 0],
        "exact null counts, per result column, from the write pass"
    );

    let provider = store
        .open(&ctx, id)
        .await
        .expect("a settled snapshot opens");
    let name = snapshot_name(id);
    ctx.register_table(name.as_str(), provider)
        .expect("the engine registers what open answered");

    let read = ctx
        .sql(&format!("SELECT * FROM {name}"))
        .await
        .expect("a read of the snapshot");
    assert_eq!(
        read.schema().field(1).data_type(),
        &DataType::Union(union_fields(), UnionMode::Sparse),
        "a union survives the round trip as itself"
    );
    assert_eq!(
        read.schema().field(2).name(),
        "__strata_ord",
        "the ordinal is written when the snapshot is minted, last"
    );

    let ordered = ctx
        .sql(&format!(
            "SELECT \"n\", \"__strata_ord\" FROM {name} ORDER BY \"__strata_ord\""
        ))
        .await
        .expect("an ordered read")
        .collect()
        .await
        .expect("the rows");
    let rows: Vec<(Option<i32>, u64)> = ordered
        .iter()
        .flat_map(|b| {
            let n = b
                .column(0)
                .as_any()
                .downcast_ref::<Int32Array>()
                .expect("n")
                .clone();
            let ord = b
                .column(1)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .expect("the ordinal")
                .clone();
            (0..b.num_rows())
                .map(|r| (n.is_valid(r).then(|| n.value(r)), ord.value(r)))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (Some(1), 1),
            (None, 2),
            (Some(3), 3),
            (Some(4), 4),
            (None, 5),
            (Some(6), 6),
        ],
        "the ordinal numbers the rows in the order they were handed over, across batches"
    );

    names_where_its_bytes_are(&store.owned_storage());

    store.retire(id);
    assert!(
        store.open(&ctx, id).await.is_err(),
        "a retired snapshot is gone"
    );
}

/// **What a store names as its storage is where its bytes are** — the whole basis of the write
/// fence (`export::refuse_owned_target`), which refuses a `COPY` beneath these roots and nothing
/// else. A root that is relative would be compared against a process cwd the store never meant,
/// and a root holding nothing after a settle is a fence over the wrong place.
///
/// Answering nothing is the default and says nothing, so it is asserted about only when a store
/// does answer.
fn names_where_its_bytes_are(roots: &[PathBuf]) {
    if roots.is_empty() {
        return;
    }
    assert!(
        roots.iter().all(|root| root.is_absolute()),
        "a store names its storage absolutely: {roots:?}"
    );
    assert!(
        roots
            .iter()
            .any(|root| root.read_dir().is_ok_and(|mut it| it.next().is_some())),
        "a settled snapshot is somewhere under what the store named: {roots:?}"
    );
}

/// A live set spares what it names, and nothing else.
async fn purge_keeps_only_the_live(store: &dyn SnapshotStore) {
    let ctx = test_context(&BTreeMap::new());
    let schema = result_schema();
    for id in [SnapshotId(1), SnapshotId(2)] {
        let mut sink = store
            .begin(id, Arc::clone(&schema), None)
            .expect("a write pass");
        sink.write(&batch(&schema, 1)).expect("the batch");
        sink.settle().expect("a settled snapshot");
    }

    store.purge_orphans(&HashSet::from([SnapshotId(2)]));

    assert!(store.open(&ctx, SnapshotId(1)).await.is_err());
    assert!(store.open(&ctx, SnapshotId(2)).await.is_ok());

    store.purge_orphans(&HashSet::new());
    assert!(store.open(&ctx, SnapshotId(2)).await.is_err());
}

/// A store rooted in a scratch directory, so the suite never writes into the machine-shared root.
fn local() -> LocalIpcSnapshotStore {
    let mut root = std::env::temp_dir();
    root.push(format!("strata_conformance_{}", std::process::id()));
    LocalIpcSnapshotStore::new_in(root)
}

#[tokio::test]
async fn local_ipc_keeps_the_contract() {
    let store = local();
    the_contract(&store).await;
    store.purge_orphans(&HashSet::new());
}

#[tokio::test]
async fn mem_keeps_the_contract() {
    the_contract(&MemSnapshotStore::new()).await;
}

#[tokio::test]
async fn local_ipc_purges_only_orphans() {
    let store = local();
    purge_keeps_only_the_live(&store).await;
}

#[tokio::test]
async fn mem_purges_only_orphans() {
    purge_keeps_only_the_live(&MemSnapshotStore::new()).await;
}

/// The union column is what makes the fidelity claim testable, so it must genuinely be one:
/// a store that silently coerced it would pass every other assertion above.
#[test]
fn the_fixture_carries_a_union() {
    assert!(matches!(
        result_schema().field(1).data_type(),
        DataType::Union(_, UnionMode::Sparse)
    ));
}
