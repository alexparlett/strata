# WJ-02 · `PolymorphicJsonFormat` — a union-tolerant JSON reader

**Workstream:** JSON · **Status:** ✅ `[core ✓]` · **DEV_TASKS:** — · **Depends on:** —
(WJ-01 is not a build dependency; it is what makes the output worth having)

## State of play

Done. `strata-core::engine::json_poly`:

- `infer` — the union-tolerant schema inference (the fork of arrow's merge rule).
- `normalize` — making a parsed record fit that schema.
- `format` — `PolyJsonFormat` / `PolyJsonSource` / `PolyJsonOpener`, selected by the
  `SourceFormat::Json` arm of `register_external`. **It is the only JSON reader now**; arrow's
  `JsonFormat` is no longer constructed anywhere.

Verified against the real `sample/config.json` (62MB, one record, 19 top-level columns, 241,425
nested fields): **registers in ~1.4s** and queries. The conflicted path
`nba.nbas[].contentVariants[].content.content[].content` comes back `Utf8` with every level above
it keeping its real structure (List → Struct → List → Struct → Struct → List → Struct → Utf8).

Peak RSS during a standalone infer+decode run was ~880MB. Worth watching: a 241k-field schema is a
lot of Arrow arrays for one row, and it is the reason `create_file_opener` projects the file schema
down to the requested columns rather than decoding all of them.

## Goal
Register a JSON source whose fields disagree across records, instead of failing schema inference.
A conflicted field becomes **`Utf8` holding the raw JSON text** of whatever that record had.

## Where it hooks in
One arm of the reader match in `register_external` (`engine/catalog.rs`):

```rust
SourceFormat::Json(o) => Arc::new(json_format(o)),   // -> PolymorphicJsonFormat
```

`FileFormat` is a public trait and `ListingOptions` / `infer_schema` / `ListingTable` are all
generic over `Arc<dyn FileFormat>`. Nothing in DataFusion resists the substitution. The source file
is never touched and no new Table Config option is required.

## What is actually wrong (verified against arrow-json 58.3)

`collect_field_types_from_object` (reader/schema.rs:390) errors when a key already inferred as one
`InferredType` arrives as another:

```
Expected object json type, found: Array(Scalar({Utf8, Boolean}))
```

`InferredType::merge` (schema.rs:36) is the whole compatibility rule, and it is deliberately narrow
— Array∪Array, Scalar∪Scalar, Object∪Object, anything∪Any, and the scalar↔array promotion. Every
other pair is `Incompatible type found during schema inference`. **Object ∪ Array has no arm**, and
neither does Object ∪ Scalar.

## The corrections — read these before estimating

### 1. You are not writing a JSON→Arrow decoder — but feed it bytes, not serde values
Arrow builds the arrays either way. The transform is: parse each record to a `serde_json::Value`,
rewrite the values that do not fit, hand it back to arrow.

`Decoder::serialize<S: Serialize>` (reader/mod.rs:652) looks like the obvious route and **does not
work in this crate**. `strata-core` builds `serde_json` with `arbitrary_precision`, which encodes
every `Number` as the magic map `{"$serde_json::private::Number": "0"}`; arrow walks that as a
struct and fails with `expected primitive got {…}` on the first numeric field. Use
`Decoder::decode(&[u8])` over re-serialized text instead — one allocation per batch, and it
sidesteps serde's representation entirely. (Do not "fix" this by dropping `arbitrary_precision`:
it is there so result serialization keeps full numeric precision.)

### 1b. Three normalization rules, not one — each found by running the real file
The stringify rule alone gets you two failures deep into `sample/config.json` and no further:

- **objects and arrays** in a `Utf8` slot → their JSON text. The headline rule.
- **scalars** in a `Utf8` slot → their JSON text *as well*. Arrow's `StringArrayDecoder` renders a
  bool or number into a string column only under `coerce_primitive`, and DataFusion's `JsonOpener`
  builds a bare `ReaderBuilder::new(schema)` — so `{"content": false}` is `expected string got
  false`. Setting the flag on our own reader is the smaller diff and the wrong one: it is not
  scoped to the conflicted column, so it would also start accepting `"1"` into an `Int64`.
- **a bare value against a list target** → wrapped into a one-element list. This is arrow's own
  scalar↔array promotion, finished: `InferredType::merge` folds Scalar into Array, but the decoder
  has no matching rule and reports `expected [ got "x"`. **Stock arrow can infer a schema it then
  refuses to read**, and `config.json` has exactly one such path
  (`nba.nbas[].templateRules…rules[].value`) that fails with or without any conflict handling.

### 2. The `FileOpener` is the real cost, and it is plumbing
The swap point is *inside* `JsonOpener::open` (datafusion-datasource-json/src/source.rs) — 212
lines wrapping compression handling, byte-range boundary alignment (`boundary_stream.rs`) and
whole-document-array→NDJSON conversion, with `ReaderBuilder` constructed in the middle. None of it
is inheritable. Plus a `FileSource` impl (~8 required methods: `create_file_opener`,
`create_morselizer`, `table_schema`, `with_batch_size`, `metrics`, `file_type`, and the schema-
adapter pair).

That is the bulk of this task, it is the least interesting part of it, and it is ours to re-verify
on every DataFusion upgrade. Budget accordingly.

### 3. Inference is **sampled**, and the naive rule makes things worse
`schema_infer_max_rec` defaults to 1000 (`DEFAULT_SCHEMA_INFER_MAX_RECORD`), and `JsonRead::infer_rows`
defaults to `None` = that default.

Today, a conflict at record 1001 fails at **registration** — loud, immediate, one catalog row,
before any query runs. Under a "fix the merge rule so a conflict infers Utf8" reader, the first
1000 records agree, the schema says `Struct`, and the conflict blows up **mid-query in the scan**
as a query failure on a table the catalog calls healthy.

So the normalizer must be tolerant **at read time too**: any value that does not fit its target
slot is stringified if that slot is `Utf8`, regardless of what inference decided. This is not a
detail — it is the difference between a reader that works and one that relocates the failure
somewhere worse. Cheap to implement, easy to forget, and the acceptance test for it is a file whose
conflict appears after `infer_rows`.

Arrow's own `ReaderBuilder::with_ignore_type_conflicts` is **not** the answer: it turns conflicts
into **nulls** (string_array.rs:78 — `_ if self.ignore_type_conflicts => builder.append_null()`),
which is silent data loss. DataFusion never sets it (zero occurrences in the JSON datasource).

### 4. Keep the rule narrow
Stringify **only** where arrow would have errored. Never touch a field arrow can already infer.
That makes the new reader a provable superset of the old one and keeps every existing JSON table
byte-identical, which is what lets it replace `SourceFormat::Json` wholesale rather than hiding
behind a `JsonRead` option nobody will find. A behaviour change to every JSON table in every
existing project is the thing to avoid here.

## Why Utf8 and not a Union or a struct-of-variants
Considered and rejected:

- **`DataType::Union`.** Parquet has no union logical type, so the export window could not write the
  table it was opened on — and P4-10 pins a snapshot precisely so that it always can. (WJ-01 meets
  the same wall from the other side: `json_get` *returns* a union.)
- **Struct of nullable variants.** Does not hold the array arm at all — `["...", true]` is not a
  struct, whatever its fields are.
- **Utf8 of raw JSON.** The grid, inspector, profiler and export all keep working with no change,
  and WJ-01's accessors read it natively. This is the one that costs nothing downstream.

## Blunt-vs-precise
A general reader cannot discover that `content` is tagged on `type` — that is knowledge about the
file. The options were a blunt general rule ("fields that disagree get stringified") or a per-table
hint in Table Config.

**Take the general rule**, and note the reason is stronger than "it matches the generic-capability
bar": a per-table hint requires the user to already know which column is polymorphic, and the only
way they can find out today is by hitting this error. The hint is a worse general rule, not a more
precise one.

## Acceptance
- A file with a key that is an array in some records and an object in others registers, and that
  column reads back as the raw JSON text of each record's value.
- A file whose conflict first appears **after** `infer_rows` registers and **queries** — the case
  §3 exists for.
- Every existing JSON fixture in `engine/catalog.rs`'s tests infers the identical schema it does
  today (the §4 superset claim, asserted rather than argued).
- Nested conflicts (a conflicted key inside a struct, and inside a list of structs) stringify at
  the level they occur.
- The stringified column is navigable with WJ-01's accessors.
- `cargo test -p strata-core`.

## Not in scope
- Distribution/statistics over a stringified column beyond what `Utf8` already gets.
- Any Table Config surface. If the general rule turns out to need an escape hatch, that is its own
  task with evidence behind it.
