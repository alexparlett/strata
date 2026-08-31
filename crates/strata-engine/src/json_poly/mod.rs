//! A JSON reader that tolerates a field whose type disagrees across records.
//!
//! Arrow's JSON schema inference admits five type combinations and errors on every other pair,
//! so a **type-discriminated union** — the ordinary shape of a config document or a content
//! tree — fails registration outright with wording that names neither the key nor the file:
//!
//! ```text
//! Expected object json type, found: Array(Scalar({Utf8, Boolean}))
//! ```
//!
//! Here the conflicted path becomes `Utf8` carrying that value's own JSON text, and everything
//! else infers exactly as it does today. Three parts:
//!
//! - [`infer`](mod@infer) — the fork of arrow's merge rule, with `Text` as the absorbing conflict
//!   state.
//! - [`normalize`] — rewriting a parsed record so the values match, since arrow's string decoder
//!   accepts a JSON string and nothing else.
//! - [`format`](mod@format) — the DataFusion reader that runs both over a file.
//!
//! Neither half is a JSON→Arrow decoder — arrow still builds every array. Feed it **bytes**
//! (`Decoder::decode`), not `Decoder::serialize`: this crate builds `serde_json` with
//! `arbitrary_precision`, which encodes every `Number` as `{"$serde_json::private::Number": …}`,
//! and arrow walks that as a struct and rejects it.
//!
//! [`format`](mod@format) is the `FileFormat` / `FileSource` / `FileOpener` that puts these on
//! DataFusion's
//! read path, selected by `register_external`'s `SourceFormat::Json` arm. The swap point sits
//! *inside* DataFusion's `JsonOpener::open`, so none of that plumbing could be inherited from
//! `JsonSource`.

pub mod format;
pub mod infer;
pub mod normalize;

pub use format::{PolyJsonFormat, PolyJsonFormatFactory, PolyJsonSource};
pub use infer::{infer, Inferred};
pub use normalize::{fit, fit_record};

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::arrow::array::{Array, StringArray};
    use datafusion::arrow::json::ReaderBuilder;
    use serde_json::Value;
    use std::sync::Arc;

    /// Infer, normalize, and hand the records to arrow — the whole read path minus DataFusion's
    /// file plumbing.
    ///
    /// The decode goes through **bytes**, not `Decoder::serialize`. This crate builds
    /// `serde_json` with `arbitrary_precision`, which encodes every `Number` as the magic map
    /// `{"$serde_json::private::Number": "0"}`; arrow walks that as a struct and fails with
    /// `expected primitive got {...}`. Round-tripping through text costs one allocation per
    /// batch and sidesteps serde's representation entirely.
    fn round_trip(records: &[Value]) -> datafusion::arrow::record_batch::RecordBatch {
        let schema = Arc::new(infer(records.iter()).expect("infer"));
        let mut decoder = ReaderBuilder::new(Arc::clone(&schema))
            .with_batch_size(1024)
            .build_decoder()
            .expect("decoder");
        for rec in records {
            let mut rec = rec.clone();
            fit_record(&mut rec, schema.fields());
            let line = serde_json::to_vec(&rec).expect("re-serialize");
            decoder.decode(&line).expect("decode");
        }
        decoder.flush().expect("flush").expect("one batch")
    }

    fn text_col(batch: &datafusion::arrow::record_batch::RecordBatch, name: &str) -> Vec<String> {
        let idx = batch.schema().index_of(name).expect("column");
        let col = batch.column(idx);
        let s = col
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("a conflicted column is Utf8");
        (0..col.len())
            .map(|i| {
                if s.is_null(i) {
                    "<null>".to_string()
                } else {
                    s.value(i).to_string()
                }
            })
            .collect()
    }

    /// The end-to-end claim: a key that is a string, an object and an array across records
    /// reads back as rows of **valid JSON**. This is what neither half can prove alone —
    /// inference says `Utf8`, normalize produces the text, and **arrow accepts it**.
    ///
    /// Note the string row is `"plain"`, quoted. A conflicted column holds JSON, so every row of
    /// it has to parse — that is what makes `json_get` work on all of them, and what stops a
    /// string that happens to contain JSON reading back identically to the object it resembles.
    #[test]
    fn a_conflicted_key_round_trips_as_json_text() {
        let batch = round_trip(&[
            serde_json::json!({"id": 1, "content": "plain"}),
            serde_json::json!({"id": 2, "content": {"kind": "block"}}),
            serde_json::json!({"id": 3, "content": ["a", true]}),
            serde_json::json!({"id": 4, "content": false}),
            serde_json::json!({"id": 5, "content": null}),
            serde_json::json!({"id": 6, "content": r#"{"kind":"block"}"#}),
        ]);
        assert_eq!(batch.num_rows(), 6);
        let col = text_col(&batch, "content");
        assert_eq!(
            col,
            vec![
                r#""plain""#.to_string(),
                r#"{"kind":"block"}"#.to_string(),
                r#"["a",true]"#.to_string(),
                "false".to_string(),
                "<null>".to_string(),
                r#""{\"kind\":\"block\"}""#.to_string(),
            ]
        );
        assert_ne!(col[1], col[5], "a string holding JSON is not that object");
    }

    /// `sample/config.json`'s real shape — a recursive content tree inside a list of structs,
    /// where the conflict is three levels down.
    #[test]
    fn a_recursive_content_tree_round_trips() {
        let batch = round_trip(&[serde_json::json!({
            "variants": [
                {"content": {"content": [{"content": "leaf text"}]}},
                {"content": {"content": [{"content": {"kind": "image"}}]}},
            ]
        })]);
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 1);
    }

    /// Arrow's scalar↔array promotion, end to end: inference makes the column a list, normalize
    /// wraps the bare scalar, and the decode succeeds. Stock arrow infers the same schema and
    /// then refuses to read it.
    #[test]
    fn a_promoted_scalar_round_trips_as_a_one_element_list() {
        let batch = round_trip(&[
            serde_json::json!({"v": ["a"]}),
            serde_json::json!({"v": "b"}),
        ]);
        assert_eq!(batch.num_rows(), 2);
        let col = batch.column(0);
        let list = col
            .as_any()
            .downcast_ref::<datafusion::arrow::array::ListArray>()
            .expect("promoted to a list");
        assert_eq!(list.value_length(0), 1);
        assert_eq!(
            list.value_length(1),
            1,
            "the bare scalar became one element"
        );
    }

    /// The superset claim, checked against arrow itself rather than argued: for a file with no
    /// conflicts, our inference must produce exactly what arrow's does.
    #[test]
    fn a_conflict_free_file_infers_exactly_as_arrow_does() {
        let records = vec![
            serde_json::json!({"i": 1, "s": "x", "b": true, "l": [1, 2], "o": {"k": "v"}}),
            serde_json::json!({"i": 2, "s": "y", "b": false, "l": [3], "o": {"k": "w"}}),
        ];
        let ours = infer(records.iter()).expect("ours");
        let theirs = datafusion::arrow::json::reader::infer_json_schema_from_iterator(
            records.iter().map(Ok),
        )
        .expect("arrow's");

        let norm = |s: &datafusion::arrow::datatypes::Schema| {
            let mut v: Vec<(String, String)> = s
                .fields()
                .iter()
                .map(|f| (f.name().clone(), format!("{:?}", f.data_type())))
                .collect();
            v.sort();
            v
        };
        assert_eq!(norm(&ours), norm(&theirs));
    }
}
