//! Reading JSON that stock DataFusion refuses, and the SQL to query it with.
//!
//! This is what an embedder's first real-world JSON file runs into, so it is worth reading before
//! the file does.
//!
//! # The problem
//!
//! Arrow's JSON schema inference admits five type combinations and errors on every other pair. A
//! **type-discriminated union** — the ordinary shape of a config document, a content tree, an
//! event envelope — is not one of them:
//!
//! ```json
//! {"id": 1, "value": "on"}
//! {"id": 2, "value": true}
//! ```
//!
//! Registering that file with stock DataFusion fails outright, and the message names neither the
//! key nor the file:
//!
//! ```text
//! Expected object json type, found: Array(Scalar({Utf8, Boolean}))
//! ```
//!
//! There is nothing to do about it from outside: inference has already given up before any option
//! could be applied to it.
//!
//! # What this engine does instead
//!
//! [`json_poly`](crate::json_poly) forks arrow's merge rule so that **text is the absorbing
//! conflict state**. A path whose type disagrees across records becomes `Utf8` carrying each
//! value's own JSON text; every other path infers exactly as it does today. So the file above
//! reads as `{id: Int64, value: Utf8}`, with `value` holding `"on"` and `true` — the second one
//! spelled as JSON, not as a string that used to be a boolean.
//!
//! Nothing is dropped and nothing is guessed. Three parts do it:
//!
//! - [`infer`](mod@crate::json_poly::infer) — the merge rule, with `Text` as the conflict state.
//! - [`normalize`](crate::json_poly::normalize) — rewriting a parsed record so its values match
//!   the inferred schema, since arrow's string decoder accepts a JSON string and nothing else.
//! - [`format`](mod@crate::json_poly::format) — the `FileFormat` / `FileSource` / `FileOpener` that
//!   runs both over a file on DataFusion's own read path.
//!
//! Neither half is a JSON→Arrow decoder: arrow still builds every array.
//!
//! # The union's journey
//!
//! The conflict state is not confined to inference — it has to survive everything downstream, and
//! two of those places are why the engine is shaped the way it is.
//!
//! **The snapshot.** A settled result spools to Arrow IPC rather than parquet, and this is one of
//! the two reasons why: **parquet cannot write a union at all.** A store that wrote parquet would
//! be a store that could not hold the results of the queries this reader exists to make possible.
//! See [`storage`](super::storage) if you are writing your own.
//!
//! **The presentation.** A union renders through a projection
//! (`datafusion.format.json_unions_as_text`), which is *presentation and not storage*: the value
//! kept is the union, and the text is what a grid cell shows. A reader that stored the text
//! instead would have thrown the type away at the first render.
//!
//! # Querying it
//!
//! Once a JSON object is a `Struct` with keys unioned across the file, DataFusion's struct
//! vocabulary runs out: `get_field` and dot access take a key written into the SQL, nothing
//! answers "which keys does *this row* have", and nothing indexes by a computed key. So the
//! engine registers four functions ([`udfs`](crate::udfs)):
//!
//! | Function | What it answers |
//! |---|---|
//! | `struct_keys(s)` | the keys this row has, as a list of strings |
//! | `struct_entries(s)` | each key paired with its value, still typed — `unnest` it to walk the map |
//! | `struct_get(s, k)` | the value at a **computed** key, the one thing `get_field` cannot do |
//! | `to_json(v)` | any value as JSON text |
//!
//! Plus `regexp_extract_all(col, pattern)`, which returns every match rather than the first.
//!
//! Two of them carry a shape rule. `struct_entries` and `struct_get` return one Arrow type per
//! call, so a struct whose values do not share a type is refused **at planning time**, by name,
//! pointing at `to_json`. Keys are keys and text is text, so the other two need no such rule.
//!
//! **What a null means here.** A key absent from a record is a null field in that row, which is
//! what makes the bitmap read the honest per-row answer — and also means an explicit `null` and
//! an absent key cannot be told apart. That loss happens at inference, not in these functions,
//! and every one of them says so in its own description.
//!
//! # À la carte: the reader without the engine
//!
//! Everything above is usable in a `SessionContext` of your own, with no engine at all.
//! `strata-engine` with `--no-default-features` compiles no data source and is a legitimate first
//! taste of the crate.
//!
//! ## The functions
//!
//! [`udf_package::register`](crate::udf_package::register) applies the engine's own registration
//! rule — including the warning when a name replaces something, which `register_udf` is silent
//! about:
//!
//! ```
//! use datafusion::prelude::SessionContext;
//! use strata_engine::udf_package;
//! use strata_engine::udfs::StrataFunctions;
//!
//! let ctx = SessionContext::new();
//! udf_package::register(&ctx, &StrataFunctions);
//! ```
//!
//! ## The reader, per table
//!
//! [`PolyJsonFormat`](crate::json_poly::PolyJsonFormat) is an ordinary DataFusion `FileFormat`.
//! Hand it to a `ListingTable` for the tables that need it and leave the rest of the session
//! alone:
//!
//! ```
//! use std::sync::Arc;
//!
//! use datafusion::datasource::listing::{
//!     ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
//! };
//! use datafusion::prelude::SessionContext;
//! use strata_engine::json_poly::PolyJsonFormat;
//! use strata_model::JsonRead;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let dir = std::env::temp_dir().join("strata-guide-json");
//! # std::fs::create_dir_all(&dir)?;
//! # std::fs::write(
//! #     dir.join("events.json"),
//! #     "{\"id\":1,\"value\":\"on\"}\n{\"id\":2,\"value\":true}\n",
//! # )?;
//! let ctx = SessionContext::new();
//! let options = ListingOptions::new(Arc::new(PolyJsonFormat::new(JsonRead::default())))
//!     .with_file_extension(".json");
//! let url = ListingTableUrl::parse(dir.join("events.json").display().to_string())?;
//! let config = ListingTableConfig::new(url).with_listing_options(options);
//!
//! futures::executor::block_on(async {
//!     let config = config.infer_schema(&ctx.state()).await?;
//!     ctx.register_table("events", Arc::new(ListingTable::try_new(config)?))?;
//!
//!     // 'value' disagreed across records, so it is text carrying each value's own JSON.
//!     let rows = ctx
//!         .sql("SELECT value FROM events ORDER BY id")
//!         .await?
//!         .collect()
//!         .await?;
//!     assert_eq!(rows[0].num_rows(), 2);
//!     Ok::<_, Box<dyn std::error::Error>>(())
//! })?;
//! # std::fs::remove_dir_all(&dir).ok();
//! # Ok(())
//! # }
//! ```
//!
//! ## The reader, session-wide — and the caveat
//!
//! [`PolyJsonFormatFactory`](crate::json_poly::PolyJsonFormatFactory) registers the reader under
//! `json` for the whole session, so `CREATE EXTERNAL TABLE … STORED AS JSON` resolves through it.
//! A plain DataFusion session has no per-table seam to select on, so this is the only way to get
//! the reader everywhere.
//!
//! **The engine itself does not use it, and that is a decision rather than an omission.** The
//! factory map is what DataFusion resolves `COPY … STORED AS JSON` against too, so registering
//! here moves the **writer** as well as the reader. Strata selects the reader per table
//! ([`formats`](crate::formats)) precisely so the writer is left alone. Take the factory knowing
//! that trade; the per-table route above does not make it.
//!
//! ## Arrow statistics
//!
//! [`StrataArrowFormat`](crate::arrow_stats::StrataArrowFormat) is `ArrowFormat` with one method
//! replaced: it reads the row count out of the IPC footer instead of answering "unknown". It is
//! useful anywhere an Arrow-backed `ListingTable` should report a free row count, engine or no
//! engine.
