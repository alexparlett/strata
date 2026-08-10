# ED-10 · Typed CREATE EXTERNAL TABLE onto the Table Config funnel

**Workstream:** Editor statements · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** ED-02

**Built** in `crates/strata-core/src/engine/ddl/external.rs`; the surface as built is
`docs/STATEMENTS_SPEC.md` §6.7 and the invariant is AGENTS.md §2. What follows is the design as
planned, then **what the build settled beyond it** — read that section, it is where the
options/connections collision was decided.

## Goal

`CREATE EXTERNAL TABLE` typed in the editor registers an ordinary external table through the
funnel Table Config already uses — the parsed statement becomes a `TableDef`, and Table Config
and typed DDL are two gestures into one registration path, exactly as ⌘S and typed `CREATE VIEW`
are for views. The identical settle as CTAS's: `docs/STATEMENTS_SPEC.md` §2 + §7.

## Current state

- Table registration is one function: `register_external`
  (`crates/strata-core/src/engine/catalog.rs:68`), driven from defs via `table_spec`
  (`register.rs:54`). The Configure window builds defs (`apps/configure/model.rs::def(root)` —
  source relativization, format options, hive partitions) and never registers directly.
- The statement is refused today (`Blocked::CreateExternalTable` → "Register tables in Table
  Config"); that variant and message stay as the **agent** path's refusal.
- DF's native `CREATE EXTERNAL TABLE` path (`TableProviderFactory`) must stay unused — it would
  register behind the store's back, and the def, not the engine registration, is the durable
  artifact.

## What to build

`engine/ddl.rs::create_external` — map the parsed `CreateExternalTable` statement onto a
`TableDef { origin: External }`, then register and fold like CTAS:

- `STORED AS` → `SourceFormat`: `PARQUET`/`CSV`/`JSON`/`ARROW`; anything else refused **by name**
  (the Avro-fallthrough rule — a format with no reader must fail, never fall through, P4-11).
- `LOCATION` → one source; relativize when under the project root (the same rule Configure's
  `def(root)` applies — share it, don't restate it).
- `OPTIONS(…)` → the matching `CsvRead`/`JsonRead` fields (`format.has_header`,
  `format.delimiter`, quote, escape, comment, compression, newlines-in-values, infer rows). Any
  key with no def field is refused **by name** — a silently dropped option is a def that lies
  about how the table reads.
- `PARTITIONED BY` → `partition_cols`. A column list is accepted only where every listed column
  is a partition column (declared types checked against the supported partition types —
  `Utf8`/`Int32`/`Int64`/`Date32`); data columns refuse: "Schemas are inferred. Remove the
  column list."
- Also refused: constraints, `ORDER BY` clauses, `UNBOUNDED`, `TEMPORARY`, a reserved `__snap_`
  name (router + `register_external` backstop). `IF NOT EXISTS` honored against the store's
  namespace; a plain create over an existing name errors with the store's wording.
- Outcome: `register_external` from the built def → `TableMeta` →
  `StoreEffect::TableUpserted { def, meta }` — the identical settle (store fold on
  `ProjChan::Tables`, `persisted_defs`, `catalog_settled`, history + event log) as CTAS's.

## Acceptance

- A CSV `CREATE EXTERNAL TABLE` with header/delimiter/compression options lands a def equal to
  what Configure would build for the same choices (def-equality asserted), the row `Reg::Ready`
  in the sidebar, persisted to `project.json`, queryable after the epoch bump; restart replays it
  through the ordinary pass.
- `STORED AS AVRO` refused by name; an unknown `OPTIONS` key refused by name; a data-column list
  refused; a partition-only column list carries its declared types into the def.
- `LOCATION` under the project root stores relative, outside stores absolute (matches Configure).
- `IF NOT EXISTS` over an existing name no-ops with a report; plain create over an existing name
  errors.
- The agent surface still refuses the statement with today's message.

## Verification

`cargo test -p strata-core`; run the app: type one against a fixture CSV, see it land in TABLES,
open Configure on it (ordinary editable external def), restart, still registered.

---

## What the build settled

### OPTIONS and connections collide, and the split is by namespace

The thing the plan above did not say. `datafusion-cli` puts **both** vocabularies in one
`OPTIONS` list: the reader's settings (`format.has_header`) and the object store's
(`aws.access_key_id`, `aws.region`, `aws.endpoint`, client timeouts). Strata keeps those in two
different files on purpose — the reader's are the table def's, the store's are a `ConnectionDef`'s,
and a `ConnectionDef` holds a *reference* to credentials and never a credential. So the list is
split three ways:

1. a `format.` key the def has a field for is read onto it. **The key set is the def**: every
   `CsvRead` / `JsonRead` field has a DataFusion key name and nothing else does, which is what
   `docs/IMPORT_OPTIONS.md` now tabulates from the other side;
2. a store namespace (`aws.`, `s3.`, `gcp.`, `google.`, `azure.`) or a client option — read from
   `engine::store::CLIENT_KEYS`, **shared** rather than re-listed — is refused toward Connections
   **on the key alone**. The value is never read and never echoed: it may be a secret, and a
   refusal is a sentence the user reads, copies and pastes. (A refused statement is also never
   recorded — history keeps successful runs only — so a pasted key does not outlive the buffer.)
   This arm exists *only* to choose a better sentence; it is not a gate, because —
3. everything else is refused **by name**, which is what keeps the mechanism total rather than a
   list of the keys we thought of. A CSV option on a parquet table lands here naming the format,
   which is the state `SourceFormat` exists to make unwritable.

### A LOCATION with a scheme is a connection reference

`project::split_remote` is `resolve_source` read backwards (round-trip asserted), so
`s3://acme-lake/events/2024/` becomes the pair every other path holds. The URL must be a connection
**this project has** — a statement cannot mint one (it states no provider, no region and no
credential, and must never carry the last) — and refusing here is what keeps DataFusion's "No
suitable object store found" off a table row, which is the whole point of the connections-first
phase.

Membership is a new `Engine::connections`: URLs noted by `connect` **whatever the outcome** and
removed by `disconnect`. Same shape as `InternalTables`, same defence — names and nothing else, one
engine-side question. Deliberately **not** the object-store registry, which would have answered
*no* for exactly the connections whose row the user is on their way to fix.

It is `resolve`, not `contains`, and both halves were review findings. The fallback is
**case-insensitive** because the registry is — `Url::parse` lower-cases a scheme and a host, so
`S3://acme-lake/events/` names a store that is registered and a byte-for-byte test refused it. And
it answers with the **connection's** spelling, which is what the def stores: that string is what
the Configure picker, `resolve_source` and the Forget confirm all match on, so a def must never
end up holding the user's casing of a URL nothing else recognises.

This does not soften "the LOCATION toggle is an explicit choice, never a scheme parsed out of a
path" (AGENTS.md §2): that rule is about a *typed path* in the Configure box, and here the scheme
is the only thing the statement says about where the files are. A `file://` URL is refused naming
the plain-path form rather than decoded — percent-encoding and platform traps for nothing, since
nothing in Strata writes one.

### Smaller decisions worth not re-deriving

- **`util::one_char` moved from `components::form` into the engine.** A delimiter is the same field
  in three surfaces now, and DataFusion's own `u8` config parse is not a substitute: it reads a
  numeric string as the **byte value** (so `'format.delimiter' '9'` silently means tab) and has no
  escape at all, so `'\t'` reaches it as two characters and is refused as "Non-ASCII".
- **`STORED AS NDJSON` states a shape**, so `format.newline_delimited` is refused on it and belongs
  to `STORED AS JSON` — two statements of one fact that could otherwise disagree.
- **`OR REPLACE` over an internal table is refused, and only `OR REPLACE`.** It would leave
  `.strata/tables/<slug>/` with no def naming it and nothing that could ever delete it
  (`tidy_strata_dir` sweeps only `.tmp-…`). A drop is what discards that data, and it says so. The
  fence sits **after** the `IF NOT EXISTS` and plain-create arms, because those never perform a
  replacement — it was written before them first, which answered a statement that asked to do
  nothing with advice to go and drop a table. The fence is `Engine::is_internal`, the same
  answer `INSERT` gates on, which means it cannot see an internal def whose *registration failed*
  (`note_origin` records `internal && meta.is_ok()`) — a create over that name replaces the def,
  exactly as it does over a broken external one, which is the outcome ED-04 already settled as the
  honest one. Reaching for the store's copy of the origin instead would be a second source of
  truth for one question, which is the thing `InternalTables` exists to avoid.
- **Partition types are the four Configure offers**, in SQL spelling (`VARCHAR`, `INT`, `BIGINT`,
  `DATE`); anything else is refused by name rather than falling onto `parse_dtype`'s `Utf8`, which
  a *def* still needs. `INT8` is excluded on top of that — Postgres's eight-*byte* integer and
  Arrow's eight-*bit* one, and a spelling meaning two types is not one to guess at.
- **`export::partition_columns_are_bare_words` is shared with this statement**, and its wording
  moved from naming `COPY` to naming `PARTITIONED BY` — one clause, one rule, two statements.
- **A partition name repeated in `PARTITIONED BY` is refused**, reproducing `CREATE TABLE`'s
  duplicate-column rule rather than inheriting one: Arrow's `Schema` permits duplicate field names,
  so nothing downstream would have caught it and the def would have persisted the column twice.
- **The spec comes from `register::table_spec`**, not a hand-built `TableSpec` beside it — a second
  copy of that mapping is how the typed path and the replay of the very def it wrote would drift.
- The report's `count` is `None`: a registration reads a schema, it moves no rows.
- `ddl::execute`'s `unimplemented` stub is **gone** — this was the last arm — along with the test
  that asserted it, which folded into `creating_a_table_without_a_project_folder_says_why`.

### Where it is verified

`engine::ddl::external`'s own tests cover the def equality against what Configure builds, replay
through `register_project`, every refusal above, and `source_of` directly. The one half no unit
test can reach — a typed `LOCATION` splitting onto a live bucket and actually reading it — is a
phase of `tests/object_store_minio.rs`, called in sequence for the reason that file's doc comment
gives (ambient credentials are process-wide).
