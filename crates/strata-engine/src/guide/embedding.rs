//! Embedding the engine: building one, giving it a catalog, running a statement, reading the
//! result.
//!
//! # Getting started
//!
//! An engine is built once and used from anywhere. [`Engine::builder`](crate::Engine::builder)
//! has a default for every setting, so the shortest complete engine is
//! `Engine::builder().build()`; the builder's own documentation covers each knob.
//!
//! A built engine is reached through six group handles — `ws` · `snapshot` · `catalog` ·
//! `sources` · `lang` · `work` — plus a short set of methods on the engine itself. Each handle
//! borrows the engine and carries the identity the call is about, so a call's subject is in the
//! call rather than in an argument you can pass the wrong value for.
//!
//! Four calls are the whole round trip:
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use strata_arrow::config::display_subset;
//! use strata_engine::register::CatalogSpec;
//! use strata_engine::{Engine, RunOutcome, RunTag, TableSpec, WsId};
//! use strata_model::{PageQuery, SourceFormat};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let dir = std::env::temp_dir().join("strata-guide-getting-started");
//! # std::fs::create_dir_all(&dir)?;
//! # std::fs::write(dir.join("events.csv"), "id,city\n1,Lisbon\n2,Porto\n")?;
//! let engine = Engine::builder().build();
//!
//! // 1. Tell it what catalog to hold. `sync` takes the WHOLE catalog and reconciles:
//! //    anything the spec does not name is taken out.
//! let spec = CatalogSpec {
//!     tables: vec![TableSpec {
//!         name: "events".into(),
//!         paths: vec![dir.join("events.csv").display().to_string()],
//!         format: SourceFormat::from_name("csv"),
//!         partitions: vec![],
//!         source: None,
//!         internal: false,
//!     }],
//!     ..Default::default()
//! };
//! futures::executor::block_on(engine.catalog().sync(spec, |_outcome| {}));
//!
//! // 2. Run a statement. `run` classifies first: a query executes, a statement the engine
//! //    intercepts is performed, and anything the caller may not do is refused.
//! let outcome = futures::executor::block_on(engine.ws(WsId(1)).run(
//!     RunTag(1),
//!     "SELECT city FROM events ORDER BY id".into(),
//!     100,
//! ))?;
//!
//! // 3. Take the snapshot handle off the result.
//! let RunOutcome::Rows(rows) = outcome else { panic!("a SELECT settles rows") };
//! let snapshot = rows.output.snapshot.expect("two rows were materialized");
//! assert_eq!(rows.output.total, 2);
//!
//! // 4. Read any page of it, as many times as you like. The result is immutable.
//! let display = display_subset(&BTreeMap::new());
//! let page = futures::executor::block_on(engine.snapshot(snapshot).page(
//!     PageQuery { page: 1, page_size: 10, sort: None },
//!     display,
//! ))?;
//! assert_eq!(page.rows[0][0].text, "Lisbon");
//! # std::fs::remove_dir_all(&dir).ok();
//! # Ok(())
//! # }
//! ```
//!
//! Two things in that flow are worth stating plainly, because they are the engine's shape rather
//! than incidental.
//!
//! **A result is materialized once and read many times.** A run spools its whole result to an
//! immutable snapshot and hands back a [`SnapshotId`](strata_model::SnapshotId); every later read
//! is a bounded window over that snapshot. Nothing is recomputed, memory holds one page, and the
//! total is exact because the snapshot knows — no `LIMIT` is ever injected into the caller's SQL.
//!
//! **`sync` is the whole catalog, not a work list.** It reconciles: a name the
//! [`CatalogSpec`](crate::register::CatalogSpec) does not hold is deregistered. A narrower
//! gesture is a narrower *call* — [`Catalog::register`](crate::Catalog::register) for one table,
//! [`Catalog::create_views`](crate::Catalog::create_views) for views — never a narrower spec.
//! The `settled` closure is called per def as the engine answers for it, so a host can flip one
//! row at a time rather than waiting for the pass.
//!
//! # Connecting a database
//!
//! A database is a **data source**: one def, connected once, and its whole catalog is queryable.
//! There are no per-relation defs to write — a database answers for itself, which is why
//! [`Sources::connect`](crate::Sources::connect) takes the connection and nothing under it.
//!
//! ```no_run
//! use std::collections::BTreeMap;
//!
//! use strata_engine::{Engine, RunTag, WsId};
//! use strata_model::SourceDef;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let engine = Engine::builder().build();
//!
//! futures::executor::block_on(engine.sources().connect(SourceDef {
//!     // The registrant to serve it — `SourceKind::NAME`, and what `with_source` filed it under.
//!     kind: "postgres".into(),
//!     // The name is the identity, and it is the catalog its relations are addressed by.
//!     name: "analytics".into(),
//!     config: BTreeMap::from([
//!         ("address".to_string(), "db.internal:5432/warehouse".to_string()),
//!         ("user".to_string(), "reader".to_string()),
//!         ("sslmode".to_string(), "require".to_string()),
//!     ]),
//!     // The schemas to show. Scopes display and the implicit bare-name search; a name written
//!     // in full still resolves into any schema the account can read.
//!     schemas: vec!["public".to_string()],
//!     // Writes are refused unless the def opts in. A def predating this field reads read-only.
//!     read_only: true,
//!     // No secret filed, so the password is read from `PGPASSWORD`. See below.
//!     ..Default::default()
//! }))?;
//!
//! // Three-part, or bare — `orders` resolves across the connected databases when the workspace
//! // holds no table of that name.
//! let outcome = futures::executor::block_on(engine.ws(WsId(1)).run(
//!     RunTag(1),
//!     "SELECT status, count(*) FROM analytics.public.orders GROUP BY status".into(),
//!     100,
//! ))?;
//! # let _ = outcome;
//! # Ok(())
//! # }
//! ```
//!
//! Filters, projections, limits and aggregates are pushed into **one statement** in the server's
//! own SQL, so that `GROUP BY` is counted by PostgreSQL and not by scanning the table over the
//! wire. `EXPLAIN` shows it as a `VirtualExecutionPlan`.
//!
//! **The password does not ride the def.** `SourceDef` carries the *expectation* of a secret and
//! never a value, which is what lets a project's connection descriptions be committed. Each source
//! declares an environment convention — PostgreSQL reads `PGPASSWORD` — so a def with no filed
//! secret works from the environment. To hold one instead, give the engine a
//! [`SecretProvider`](crate::secrets::SecretProvider) with
//! [`with_secrets`](crate::EngineBuilder::with_secrets) and record the slot on
//! [`SourceDef::secrets`](strata_model::SourceDef::secrets); the default is the OS keystore, and
//! [`MemSecrets`](crate::secrets::MemSecrets) is the one to reach for in a test.
//!
//! **Connecting probes**, so this call is where a mistyped address or a rejected credential is
//! reported — as the sentence to act on, not as a failure at the first query.
//!
//! Everything else about a database follows from that one call:
//! [`Sources::listing`](crate::Sources::listing) is the snapshot every surface reads,
//! [`Sources::dependents`](crate::Sources::dependents) answers what a connection is holding up,
//! and a `SHOW TABLES` costs nothing remote. To connect a backend nobody has written yet, see
//! [`data_source`](super::data_source).
//!
//! # The capability model
//!
//! ## The default is data, and it refuses nothing
//!
//! An engine built without [`with_policy`](crate::EngineBuilder::with_policy) runs every statement
//! it can run. Restriction is something an embedder *says*, not something it has to remember to
//! switch off — which means a build that forgets the policy is over-permissive and obvious, rather
//! than under-permissive and silent.
//!
//! The shipped provider is [`Capability`](crate::Capability) data: a set of
//! [`Grant`](crate::Grant)s over a local/remote axis, with the remote ones narrowable to named
//! data sources.
//!
//! ```
//! use strata_engine::policy::CapabilityPolicyProvider;
//! use strata_engine::{Capability, Engine, Grant, Locality, RemoteSel};
//!
//! // Reads anything; writes rows into the postgres connections and nothing else.
//! let capability = Capability::read_only()
//!     .with(Grant::Write(Locality::Remote))
//!     .remote_only([RemoteSel::Kind("postgres".into())]);
//!
//! let engine = Engine::builder()
//!     .with_policy(CapabilityPolicyProvider::new(capability))
//!     .build();
//! # let _ = engine;
//! ```
//!
//! [`Capability::read_only`](crate::Capability::read_only) reproduces the agent surface exactly:
//! it is the same value the MCP tools are held to, not an approximation of it.
//!
//! ## The provider seam
//!
//! [`PolicyProvider`](crate::policy::PolicyProvider) is a trait, so a decision can come from
//! somewhere other than a value — an authorization service, a per-tenant table, a feature flag.
//! It is asked in two phases:
//!
//! - [`admit`](crate::policy::PolicyProvider::admit) — may this caller ever perform this family
//!   of action? Asked before anything has resolved a target, so it is cheap and it is what
//!   refuses `CREATE TABLE` to a reader without planning anything.
//! - [`permit`](crate::policy::PolicyProvider::permit) — may they perform it against *this*
//!   target? Asked once the name has resolved, with the target's locality, backend kind and
//!   data-source url in hand.
//!
//! `permit` refines `admit` and is never more permissive than it. Both return a `Result`, and the
//! `Err` arm is not a decision: it means the provider could not decide. **The engine fails
//! closed** — the statement is refused and the provider's own words are surfaced. A provider that
//! times out reaching its authorization service denies; it does not allow.
//!
//! Attach whatever your provider needs to reason with through
//! [`Principal::with_claims`](crate::policy::Principal::with_claims), which carries an
//! `Any + Send + Sync` your `impl` downcasts.
//!
//! ## Three gates, and they are not interchangeable
//!
//! A statement passes three different kinds of judgement, and confusing them is how a fence ends
//! up in the wrong place.
//!
//! 1. **Who** — the policy provider, above. Asked in front of dispatch by
//!    [`Workspace::run`](crate::Workspace::run), and asked *without* dispatching by
//!    [`Lang::policy_verdicts`](crate::Lang::policy_verdicts), which answers the statements a
//!    read-only caller may not perform. A caller that has no discipline of its own — an agent —
//!    asks `policy_verdicts` first and refuses on any non-empty answer, an `Err` included.
//! 2. **What** — the statement pipeline's own grammar. A `CREATE TEMPORARY VIEW`, a clause the
//!    engine cannot honour, a name in the reserved `__snap_` namespace: these are
//!    [`Fault`](crate::statements::Fault)s, and **no capability makes them well-formed**. They
//!    are refused by name so a caller can act on the answer.
//! 3. **Where** — the storage fences. A write may not land in storage the engine owns (its
//!    snapshot spool, its internal-table directory, the project's `.strata/`), and an `INSERT`
//!    may only target a table whose data the engine owns or a connection whose def says it is
//!    writable. These are asked of resolved paths and parsed plans, never of SQL text.
//!
//! [`Workspace::query`](crate::Workspace::query) and
//! [`Workspace::explain`](crate::Workspace::explain) are handed a statement to *read* and do not
//! consult the policy provider at all: they are limited to reading by the read path's own
//! `SQLOptions`. Use [`run`](crate::Workspace::run) for anything a caller typed.
//!
//! # What the engine asks of its caller
//!
//! The engine owns a multi-thread Tokio runtime and spawns every call onto it, so a caller awaits
//! a `JoinHandle` rather than driving the work itself. That has three consequences worth building
//! against.
//!
//! **Futures are `Send`.** Nothing the facade returns is tied to the calling thread, and no call
//! needs a Tokio context of its own. A single-threaded UI executor, an `async-std` task and a
//! `tokio::spawn` all await engine calls the same way. This is why the app's non-Tokio render
//! thread can `await` an engine method directly while DataFusion's own parallelism runs on the
//! engine's threads.
//!
//! **Reads are values keyed by values.** [`SnapshotReads::page`](crate::SnapshotReads::page) and
//! [`SnapshotReads::chart`](crate::SnapshotReads::chart) are snapshot-scoped and side-effect
//! free: the same `(snapshot, query, display)` returns the same answer, and nothing about the
//! engine's state changes underneath. That is what makes them safe to put a cache in front of.
//!
//! **A hidden input is made keyable rather than hidden.** A read that renders cells depends on
//! the `datafusion.format.*` settings, which
//! [`set_config`](crate::Engine::set_config) moves with no restart and no new snapshot. Rather
//! than reading them off the engine — which would make the answer depend on an invisible clock —
//! `page` and `chart` take a [`DisplayStamp`](strata_arrow::config::DisplayStamp) and answer under
//! it. The stamp is constructible only by
//! [`display_subset`](strata_arrow::config::display_subset), which makes passing one a
//! compile-time obligation rather than a rule to remember. A run has no earlier moment to be told,
//! so it renders under whatever the engine holds and *reports* the stamp it used, on
//! [`RunRows::display`](crate::RunRows::display): compare that against the stamp you hold now to
//! learn whether the rows on screen still render the way a fresh read would.
//!
//! ## The swap test
//!
//! Strata's own frontend caches these reads with `freya-query`. That is the contract's
//! **reference implementation, not its definition.** Any cache layer with these three properties
//! composes with the engine:
//!
//! - it keys an entry by a value (`(snapshot, page query, display stamp)`), not by an identity;
//! - it can hold a `Send` future;
//! - it lets every input to the answer appear in the key, the hidden ones included.
//!
//! Swap `freya-query` for `moka`, for a `HashMap` behind a `Mutex`, or for nothing at all, and the
//! engine's behaviour is unchanged. There is no cache-layer trait to implement and no registration
//! call to make, because the engine does not know a cache exists.
//!
//! The **streaming boundary is the facade's own rule**, not the cache layer's: a facade call
//! answers with a whole page, never a stream. Pagination is where bounded memory comes from, and a
//! streaming read would put that bound in the caller's hands — where a cache layer, in particular,
//! cannot hold it.
//!
//! # The session ledger: can I override X?
//!
//! DataFusion settings reach an engine three ways, and which one applies decides whether a change
//! takes effect now, later, or never.
//!
//! - **At build** — [`with_config`](crate::EngineBuilder::with_config). Every `datafusion.*` key,
//!   `datafusion.runtime.*` included. This is the only place a runtime key can be set, because
//!   `RuntimeEnv` is fixed when the `SessionContext` is built.
//! - **On a built engine** — [`set_config`](crate::Engine::set_config). Answers a
//!   [`ConfigOutcome`](crate::ConfigOutcome): applied, or
//!   [`RestartOwed`](crate::ConfigOutcome::RestartOwed) for a `datafusion.runtime.*` key, which
//!   is *recorded and not applied*. [`restart_owed`](crate::Engine::restart_owed) stays true until
//!   the engine is actually rebuilt, so a host that offered a restart and had it declined can
//!   offer it again. A key **removed** from the map goes back to its
//!   [`ENGINE_KEYS`](strata_arrow::config::ENGINE_KEYS) default, not to whatever it was.
//! - **In the session** — a typed `SET`, which is an overlay in front of the other two and wins
//!   for its keys until `RESET` or restart. `set_config` **skips a key the overlay holds**: the
//!   new value becomes the baseline a later `RESET` lands on, rather than quietly overwriting
//!   what the user typed. That is the whole precedence rule, and it is enforced in `set_config`
//!   because that is the only place the two writers meet.
//!
//! Some keys are refused to `SET` and `RESET` alike and belong to the embedder's own settings
//! surface: `datafusion.runtime.*`, `format.*`, the parser dialect, and anything else the
//! embedder reads back out of its own store. Two layers answering differently about one buffer is
//! the failure that rule exists to prevent.
//!
//! [`ENGINE_KEYS`](strata_arrow::config::ENGINE_KEYS) is the written-down catalogue of the keys an
//! engine offers, with each one's default, value shape and one-line description — enough to build
//! a settings editor from without naming a key twice.
//!
//! # Where to go next
//!
//! - [`json`](super::json) — the polymorphic JSON reader, which is what an embedder's first
//!   real-world JSON file runs into, and how to use it in a plain DataFusion session.
//! - [`data_source`](super::data_source) — writing your own backend.
//! - [`storage`](super::storage) — putting results or engine-owned tables somewhere else.
