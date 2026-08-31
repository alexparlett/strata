//! Writing a [`DataSource`](crate::sources::source::DataSource): an object store, or a server with
//! a catalog of its own.
//!
//! Everything the shipped sources get, yours gets: the catalog tree, completion, the connection
//! editor's form, bare-name resolution, the forget confirm, `SHOW TABLES`, the agent's
//! `list_tables`. None of it is per-source code. If your backend needs something outside the trait
//! to work, that is a bug in the seam and worth reporting rather than working around.
//!
//! # The one trait, and its companion
//!
//! [`DataSource`](crate::sources::source::DataSource) has one required method,
//! [`connect`](crate::sources::source::DataSource::connect). [`SourceKind`](crate::sources::source::SourceKind)
//! is a companion trait of associated consts rather than methods on the same trait, because a
//! const is not dyn-compatible — and because the consts are read where the concrete type is still
//! in hand, so a source cannot answer differently from the key it was filed under.
//!
//! ```
//! use std::sync::Arc;
//!
//! use async_trait::async_trait;
//! use strata_engine::secrets::SecretProvider;
//! use strata_engine::sources::source::{DataSource, SourceKind, SourceMode, Sourced};
//! use strata_model::SourceDef;
//!
//! #[derive(Debug)]
//! struct Ledger;
//!
//! impl SourceKind for Ledger {
//!     const NAME: &'static str = "ledger";
//!     const LABEL: &'static str = "Ledger";
//!     const BADGE: &'static str = "LDG";
//!     const MODE: SourceMode = SourceMode::Catalog;
//! }
//!
//! #[async_trait]
//! impl DataSource for Ledger {
//!     async fn connect(
//!         &self,
//!         def: &SourceDef,
//!         _secrets: Arc<dyn SecretProvider>,
//!     ) -> Result<Sourced, String> {
//!         let _address = def.setting("address");
//!         # let _ = Ledger;
//!         # unimplemented!()
//!         // ... open a handle, probe it, and hand back a `Sourced::Catalog`.
//!     }
//! }
//! ```
//!
//! Register it with [`with_source`](crate::EngineBuilder::with_source). The registry is one map
//! keyed by `NAME`, so registering over a name another holds **replaces** it — which is how an
//! embedder substitutes their own implementation for a shipped one. A def naming a kind nothing
//! answers to settles as a failed row naming the fix, never as a panic.
//!
//! # `connect -> Sourced`: the mode rides the answer
//!
//! A source is something you connect to that hands back one of two things:
//!
//! - [`Sourced::Store`](crate::sources::source::Sourced::Store) — an `ObjectStore`, registered
//!   under `scheme://address` from your [`SCHEME`](crate::sources::source::SourceKind::SCHEME).
//!   What it holds is described by table defs, exactly as local files are.
//! - [`Sourced::Catalog`](crate::sources::source::Sourced::Catalog) — a
//!   [`SourceCatalog`](crate::sources::source::SourceCatalog), a live handle that names its own
//!   relations.
//!
//! The mode-specific vocabulary lives on that sum's arms rather than on the trait, so **a bucket
//! is never asked to enumerate**: the method is not there to answer wrongly. Declare which arm you
//! will return on [`MODE`](crate::sources::source::SourceKind::MODE) — the editor has to draw a
//! form before anything connects — and the conformance ring asserts the two agree.
//!
//! **Connecting probes.** It is all-or-nothing and it settles one row, so the error you return
//! *is* that row's sentence: word it as the thing to fix, not as a stack trace. A description that
//! is well-formed and wrong — a mistyped region, a bucket that does not exist — has to fail here,
//! or every table under it fails later with the store's own unhelpful wording. What a probe must
//! **not** do is ask whether the caller may do everything: a prefix-scoped credential and a
//! read-only public bucket both refuse at the root while working perfectly.
//!
//! **Secrets never ride the def.** Read what you need from the `secrets` argument through a
//! [`SecretRequest`](crate::secrets::SecretRequest) naming your own family and your own
//! environment convention — per use, never stored, never held past the login it is for. The def
//! carries the *expectation* of a secret and never a value, which is what lets a `project.json`
//! be committed.
//!
//! # `SourceCatalog`: what a catalog answers
//!
//! Three methods have no default, because no catalog can be read without them:
//! [`kind`](crate::sources::source::SourceCatalog::kind),
//! [`enumerate`](crate::sources::source::SourceCatalog::enumerate) and
//! [`table_provider`](crate::sources::source::SourceCatalog::table_provider). Everything else
//! defaults to the honest answer for a source that does not do that thing — a refusal naming your
//! kind, in the trait's own words.
//!
//! **Every method is a source-agnostic concept**, which is the property that lets a document store
//! implement it honestly. `enumerate` answers a [`Listing`](crate::sources::source::Listing) of
//! namespaces holding relations: a PostgreSQL schema, a MySQL database and a document store's
//! collections all arrive as the same shape, keyed case-insensitively the way SQL resolves names.
//! `Listing::of` does that folding for you, so no source implements the keying rule itself.
//!
//! Leave a refusal alone unless you have something better to say. The conformance ring checks that
//! a source which has not implemented `writer` refuses in the **trait's** words rather than in its
//! own — a source wording that refusal itself is a source that has a writer it is not admitting to.
//!
//! # Two roads for reads
//!
//! ## Your source speaks SQL
//!
//! Compose [`sources::sql`](crate::sources::sql). Build your `SqlTable`, describe it in a
//! [`SqlSpec`](crate::sources::sql::SqlSpec) — a dialect, an
//! [`SQLExecutor`](crate::sources::sql::SQLExecutor), the provider — and hand it to
//! [`federated`](crate::sources::sql::federated). What comes back is a `TableProvider` whose scans
//! leave as **one statement** in your source's own SQL, with filters, projections, limits and
//! aggregates pushed into it.
//!
//! What your source spells differently is its **dialect's** business, not the assembly's: say it
//! once in the `Dialect` you hand over and every path that writes SQL for that connection obeys it.
//! That is how the JSON operators reach PostgreSQL as `->>` rather than as a UDF call.
//!
//! ## Your source speaks something else
//!
//! Bring your own `TableProvider` and compile what you can push down into your own query shape.
//! Nothing in [`SourceCatalog`](crate::sources::source::SourceCatalog) requires SQL, and the
//! shipped test source [`TestDoc`](crate::testing::TestDoc) is exactly this: a catalog whose
//! relations are read through a provider of its own, with every statement method refusing through
//! the trait's defaults.
//!
//! ### The obligation you inherit
//!
//! **Two data sources must never share plan-cache identity.** Whatever `table_provider` hands back
//! has to be distinguishable per data source by whatever the query engine fuses subplans on, or
//! two connections answer each other's queries — a plan built across `north` and `south` is sent
//! whole to whichever executor won.
//!
//! Composing [`federated`](crate::sources::sql::federated) discharges this: the assembly stamps
//! the identity, so a source that composes it *cannot* forget. A source writing its own provider
//! carries the obligation itself, and the conformance shape to reproduce is
//! `two_sources_of_one_kind_are_two_compute_contexts`.
//!
//! # `function_map`: who says what, about a function your source lacks
//!
//! [`function_map`](crate::sources::source::SourceCatalog::function_map) is where a source states
//! what it does with the engine's function names — and only the handful its own vocabulary really
//! differs on. A name absent from the map travels unchanged.
//!
//! The division of labour is fixed and worth stating, because getting it backwards produces two
//! half-refusals that disagree:
//!
//! - **You own the rendering.** If your source has a faithful spelling — the same answer computed
//!   there — say [`Support::Mapped`](crate::sources::source::Support::Mapped) and put the spelling
//!   in your dialect.
//! - **The engine words the refusal.** Say
//!   [`Support::Unmapped`](crate::sources::source::Support::Unmapped) with a `why` clause naming
//!   what to reach for instead, and the engine builds the sentence: it names the function, the
//!   connection and the way out. You do not write "cannot" anywhere.
//!
//! An approximation is not a mapping. `json_contains` is *not* `?`, and answering `Mapped` for it
//! would make a query silently return different rows against your source than against a local
//! file. Refuse by name instead; that is what the `why` clause is for.
//!
//! # Running the conformance rings against your backend
//!
//! There are two, and they answer different questions.
//!
//! ## The generic ring — a body you call
//!
//! Every backend meets it, whether or not it speaks SQL. Turn on the `testing` feature and run
//! your registrant through [`conforms`](crate::testing::conforms):
//!
//! ```toml
//! [dev-dependencies]
//! strata-engine = { version = "0.2", features = ["testing"] }
//! ```
//!
//! Then one call per registrant:
//!
//! ```text
//! strata_engine::testing::conforms(Ledger, &a_def_it_can_connect()).await;
//! ```
//!
//! `examples/custom_source.rs` is a complete source written, run through the ring and queried, in
//! one file — `cargo run -p strata-engine --features testing --example custom_source`.
//!
//! It asserts the form you declare can be drawn, that connecting yields the mode you declared,
//! that the handle names its own kind, that the enumeration is non-empty and a relation resolves
//! to a provider, that what you have not implemented refuses in the trait's own words, and that
//! [`WRITABLE`](crate::sources::source::SourceKind::WRITABLE) agrees with whether a writer is
//! really there — **in both directions**.
//!
//! The def you pass has to be one your source can connect, with a real address and real
//! credentials. Use [`conforms_with`](crate::testing::conforms_with) to supply a
//! [`SecretProvider`](crate::secrets::SecretProvider) of your own where the default empty one will
//! not do.
//!
//! ## The SQL ring — a shape you reproduce
//!
//! For a backend composing [`sources::sql`](crate::sources::sql), and only meaningful against a
//! real server, so it is a set of phases rather than a function to call. Strata's own runs live in
//! `crates/strata-engine/tests/postgres_federation.rs` and `mysql_federation.rs`; read either and
//! reproduce the phases against your own container. What they establish:
//!
//! - **Pushdown is proven, not assumed.** Plan a filtered, projected, limited scan and assert the
//!   plan contains a `VirtualExecutionPlan` — that the work left as one statement rather than
//!   being executed locally over everything.
//! - **JSON support is exercised end to end**, if your dialect maps any of the accessor family:
//!   run the mapped spelling against the live server, and assert the unmapped ones refuse by name.
//! - **The `compute_context` stamp is distinct per connection.** Two data sources of your kind,
//!   two `EXPLAIN`s, two different `compute_context=` values. This is the generic obligation
//!   above, checked where it can actually go wrong.
//! - **Writes, if you have them**, on the arm's own terms: a CTAS that rolls its table back on a
//!   failed fill and on a cancel, an existence answer given atomically by the server rather than
//!   by a check-then-create.
//!
//! Both rings drive a source **through the registry path** — built with
//! [`with_source`](crate::EngineBuilder::with_source), reached through
//! [`Sources::connect`](crate::Sources::connect) — never by calling its methods directly. That is
//! what makes them certify a registrant rather than a struct.
//!
//! # Two sources you can borrow
//!
//! Under the same `testing` feature, and useful for tests about the surfaces *above* the seam
//! rather than about a source of your own:
//!
//! - [`TestDoc`](crate::testing::TestDoc) — a catalog source speaking no SQL at all.
//! - [`TestSql`](crate::testing::TestSql) — one composing
//!   [`sources::sql`](crate::sources::sql) over an in-memory session, so a plan really is unparsed
//!   into a statement and that statement really is parsed and run on the other side of the
//!   executor.
//!
//! A test about what a catalog pane or a forget confirm does with a connected database needs one
//! registered, and a real server is not a reasonable ask of a UI test. They are also the shortest
//! complete implementations of the trait to read.
