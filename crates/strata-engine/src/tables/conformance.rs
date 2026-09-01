//! The contract every [`InternalTableStore`] is held to, as a body any implementation can be run
//! through.
//!
//! One body, three callers: the two shipped stores, and whatever an embedder wrote. A seam with a
//! single implementation is a seam only by intention, and this is what stops the trait's law from
//! quietly becoming whatever [`LocalIpcTableStore`](super::LocalIpcTableStore) happens to do.
//!
//! Available to embedders under the `testing` cargo feature — see
//! [`guide::storage`](crate::guide::storage).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, Int32Array, Int64Array, StringArray, UnionArray};
use datafusion::arrow::buffer::ScalarBuffer;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef, UnionFields, UnionMode};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::prelude::SessionContext;
use futures::stream;

use super::InternalTableStore;
use crate::builder::test_context;
use datafusion::execution::SendableRecordBatchStream;

/// Runs `store` through the whole contract, panicking on the first clause it does not keep.
///
/// `store` has to be empty: the body creates, appends to and discards tables of its own under
/// slugs it picks.
///
/// # Examples
///
/// ```
/// # use strata_engine::tables::MemTableStore;
/// # async fn check() {
/// strata_engine::testing::tables::conforms(&MemTableStore::new()).await;
/// # }
/// ```
///
/// # Panics
///
/// On any clause the store does not keep.
pub async fn conforms(store: &dyn InternalTableStore) {
    the_contract(store).await;
    an_empty_create_still_carries_its_schema(store).await;
    an_append_to_nothing_is_refused(store).await;
}

/// A table with a union column in it — the type parquet cannot write at all, and so the one
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

fn table_schema() -> SchemaRef {
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
    RecordBatch::try_new(Arc::clone(schema), vec![n, j]).expect("a table batch")
}

/// One statement's rows, as the arms hand them over.
fn unit(batches: Vec<RecordBatch>) -> SendableRecordBatchStream {
    Box::pin(RecordBatchStreamAdapter::new(
        table_schema(),
        stream::iter(batches.into_iter().map(Ok)),
    ))
}

/// The values of `n` under `name`, in order — read through SQL, because that is how every real
/// consumer reads a registered table.
async fn read_n(ctx: &SessionContext, name: &str) -> Vec<Option<i32>> {
    let batches = ctx
        .sql(&format!(
            "SELECT \"n\" FROM {name} ORDER BY \"n\" NULLS FIRST"
        ))
        .await
        .expect("a read of the table")
        .collect()
        .await
        .expect("the rows");
    batches
        .iter()
        .flat_map(|b| {
            let n = b
                .column(0)
                .as_any()
                .downcast_ref::<Int32Array>()
                .expect("n")
                .clone();
            (0..b.num_rows()).map(move |r| n.is_valid(r).then(|| n.value(r)))
        })
        .collect()
}

/// Create → read → append → visibility through the provider already registered → replace →
/// discard, asserted the same way whatever the store is made of.
async fn the_contract(store: &dyn InternalTableStore) {
    let ctx = test_context(&BTreeMap::new());
    let schema = table_schema();

    let created = store
        .create("t", unit(vec![batch(&schema, 1), batch(&schema, 4)]))
        .await
        .expect("a create");
    assert_eq!(created, 6, "the write pass is what counted the rows");

    let provider = store
        .provider(&ctx, "t")
        .await
        .expect("a held table serves")
        .expect("and is held");
    ctx.register_table("t", provider)
        .expect("the engine registers what the store answered");

    let read = ctx.sql("SELECT * FROM t").await.expect("a read");
    assert_eq!(
        read.schema().field(1).data_type(),
        &DataType::Union(union_fields(), UnionMode::Sparse),
        "a union survives the round trip as itself"
    );
    assert_eq!(
        read_n(&ctx, "t").await,
        vec![None, None, Some(1), Some(3), Some(4), Some(6)]
    );

    let appended = store
        .append("t", unit(vec![batch(&schema, 7)]))
        .await
        .expect("an append");
    assert_eq!(appended, 3, "one unit per statement, counted by the pass");
    assert_eq!(
        read_n(&ctx, "t").await.len(),
        9,
        "the provider registered before the append re-lists per scan and sees it"
    );

    let replaced = store
        .create("t", unit(vec![batch(&schema, 10)]))
        .await
        .expect("a replace");
    assert_eq!(replaced, 3);
    assert_eq!(
        read_n(&ctx, "t").await,
        vec![None, Some(10), Some(12)],
        "a create publishes whole: the replaced rows are gone with it"
    );

    names_where_its_bytes_are(&store.owned_storage());

    store.discard("t").await.expect("a discard");
    assert!(
        store.provider(&ctx, "t").await.expect("answered").is_none(),
        "a discarded slug holds nothing"
    );
    store
        .discard("t")
        .await
        .expect("and discarding it again is a no-op, not a fault");
}

/// **What a store names as its storage is where its bytes are** — the basis of the write fence
/// (`export::refuse_owned_target`), which refuses a `COPY` beneath these roots and nothing else.
/// A relative root would be compared against a process cwd the store never meant, and a root
/// holding nothing after a create is a fence over the wrong place.
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
        "a created table is somewhere under what the store named: {roots:?}"
    );
}

/// A stream with no batches is still a table: the schema rides the stream, so what is published
/// describes its columns on every later read — which is where a bare `CREATE TABLE (cols…)`'s
/// schema comes back from on replay.
async fn an_empty_create_still_carries_its_schema(store: &dyn InternalTableStore) {
    let ctx = test_context(&BTreeMap::new());

    let created = store.create("blank", unit(vec![])).await.expect("created");
    assert_eq!(created, 0);

    let provider = store
        .provider(&ctx, "blank")
        .await
        .expect("served")
        .expect("held");
    assert_eq!(
        provider
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect::<Vec<_>>(),
        vec!["n", "j"]
    );
    ctx.register_table("blank", provider).expect("registered");
    assert!(read_n(&ctx, "blank").await.is_empty());

    store.discard("blank").await.expect("discarded");
}

/// An append to a slug the store does not hold is a refusal, not a minted table: publishing is
/// [`create`](InternalTableStore::create)'s alone.
async fn an_append_to_nothing_is_refused(store: &dyn InternalTableStore) {
    let schema = table_schema();
    store
        .append("ghost", unit(vec![batch(&schema, 1)]))
        .await
        .expect_err("nothing to append to");
}

#[cfg(test)]
mod tests {
    use super::super::{LocalIpcTableStore, MemTableStore};
    use super::*;

    /// A store rooted in a scratch directory, so the suite never writes into a project.
    fn local(tag: &str) -> LocalIpcTableStore {
        let mut root = std::env::temp_dir();
        root.push(format!(
            "strata_table_conformance_{}_{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        LocalIpcTableStore::new_in(root)
    }

    #[tokio::test]
    async fn local_ipc_keeps_the_contract() {
        the_contract(&local("contract")).await;
    }

    #[tokio::test]
    async fn mem_keeps_the_contract() {
        the_contract(&MemTableStore::new()).await;
    }

    #[tokio::test]
    async fn local_ipc_publishes_an_empty_create() {
        an_empty_create_still_carries_its_schema(&local("empty")).await;
    }

    #[tokio::test]
    async fn mem_publishes_an_empty_create() {
        an_empty_create_still_carries_its_schema(&MemTableStore::new()).await;
    }

    #[tokio::test]
    async fn local_ipc_refuses_an_append_to_nothing() {
        an_append_to_nothing_is_refused(&local("ghost")).await;
    }

    #[tokio::test]
    async fn mem_refuses_an_append_to_nothing() {
        an_append_to_nothing_is_refused(&MemTableStore::new()).await;
    }

    /// The union column is what makes the fidelity claim testable, so it must genuinely be one:
    /// a store that silently coerced it would pass every other assertion above.
    #[test]
    fn the_fixture_carries_a_union() {
        assert!(matches!(
            table_schema().field(1).data_type(),
            DataType::Union(_, UnionMode::Sparse)
        ));
    }
}
