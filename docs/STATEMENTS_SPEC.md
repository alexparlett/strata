# Full-statement editor — lifting the managed-DDL policy (ED)

Spec for the **full SQL statement surface** in Strata's editor: internal tables persisted to disk
(`CREATE TABLE` / CTAS, `INSERT`, `DROP TABLE`), typed view DDL, typed `CREATE EXTERNAL TABLE`,
`COPY … TO`, session statements (`SET`/`RESET`, `PREPARE`/`EXECUTE`/`DEALLOCATE`) and
`CREATE FUNCTION` — replacing the managed-DDL
policy's blanket refusal with a per-statement router while keeping every settled funnel (the
catalog store, the persist path, the epoch discipline, the snapshot lifecycle) exactly where it is.
Design settled 2026-08-04 with Alex; workstream: `.claude/tasks/workstream-editor-statements/`.

The one-sentence architecture: **Strata owns the catalog/schema providers as the identity and
visibility layer, and owns statement lifecycle by interception in the engine facade** — because
DataFusion 54's provider traits are resolution/enumeration interfaces, not lifecycle ones, and the
machinery the lifecycle needs (the classifier, the Arrow IPC spool, `register_external`,
`create_view`, the registration pass) already exists. An internal table is an ordinary `TableDef`
whose sources point at `.strata/tables/<name>/` with `format: Arrow` — replayed on open by the
existing pass, in the headless host too, with zero new code.

**On mechanism this spec supersedes the managed-DDL sections of `docs/reference/ENGINE.md` and the
policy invariants listed in §10** — each amendment lands with the task that implements it, per the
AGENTS.md upkeep rule; until then the code enforces the old policy and the old text stays true.

---

## 1. Direction (decided)

- **Scope**: internal tables (CTAS / `CREATE TABLE` / `INSERT` / `DROP`); typed `CREATE`/`DROP
  VIEW`; typed `CREATE EXTERNAL TABLE` onto the Table Config funnel; editor `COPY TO`; session
  statements + `CREATE FUNCTION`. The editor runs the full statement surface — the only remaining
  editor refusals are the short list in §4; unknown statement kinds stay default-deny.
- **Internal-table data is Arrow IPC under `.strata/tables/`** — type fidelity, the same rationale
  as snapshots (parquet cannot write a union or a zero-field struct, so some query results could
  not become tables). Data files are gitignored; the defs in `project.json` are the shareable half.
- **The agent surface stays read-only.** The one policy predicate gains a capability parameter;
  the MCP gate keeps exactly today's refusals and today's messages. Curated write tools may arrive
  later as new tools — never by loosening `run` (AGENT_ACCESS_SPEC §1 stands).
- **DROP TABLE works on both origins, without a confirm dialog.** Internal: deregister, then
  delete `.strata/tables/<name>/`. External: def removal only — source files untouched, and the
  report says so. The report names dependent views after the fact; the catalog surface keeps its
  before-the-fact confirm for the pointer gesture. This amends the "DROP is not supported"
  routing.
- **History records successful statements as well as data runs** — a typed `CREATE TABLE` is a
  query the user may want back. Amends the "only successful data runs" invariant; dedupe and cap
  unchanged.
- **Save-as-view stays.** Typed view DDL and ⌘S are two gestures into one funnel
  (`Engine::create_view` + the store fold); views created either way are indistinguishable.
- **Session things are session-scoped, and say so.** The SET overlay, prepared statements and
  created functions die with the engine (a restart is the `ProjectRoot` remount); every report
  string states the scope. Defs survive; nothing session-scoped is persisted.

## 2. DataFusion 54 ground truth (verified)

Verified against the sources this workspace compiles
(`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`, `datafusion-54.0.0` and siblings).
Every design decision below hangs off one of these; do not re-derive them.

| Fact | Evidence |
|---|---|
| CTAS materializes the whole result in RAM as a `MemTable`, then calls the **sync** `register_table` — no disk, no hook earlier | `datafusion-54.0.0/src/execution/context/mod.rs:868-927` (`create_memory_table`; both creating arms `collect_partitioned().await` → `MemTable::try_new`) |
| DDL executes eagerly inside `ctx.sql` (`execute_logical_plan`), returning an empty DataFrame | `context/mod.rs:686-775` |
| INSERT plans through `TableProvider::insert_into`; `ListingTable` supports it for Arrow — directory-collection URL required, schema-checked, appends one LZ4-frame IPC file per statement, `Append` only | `physical_planner.rs:845`; `datafusion-catalog-listing-54.0.0/src/table.rs:614-681`; `datafusion-datasource-arrow-54.0.0/src/file_format.rs:227-241` |
| DROP TABLE/VIEW = `find_and_deregister` → `SchemaProvider::deregister_table`; type-checked, knows nothing of files or defs | `context/mod.rs:1052-1078`, `:1430-1455` |
| `CREATE OR REPLACE VIEW` over a **table** name silently replaces the table (the `(true, Ok(_))` arm never checks `table_type`) | `context/mod.rs:939-972` |
| PREPARE/EXECUTE/DEALLOCATE supported; plans stored in `SessionState.prepared_plans` (**`pub(crate)`** — no public enumeration); EXECUTE returns the bound plan as a plain DataFrame | `context/mod.rs:733-772`, `:1534-1587`; `session_state.rs:208`, `:984-1013` |
| `SQLOptions::verify_plan` rejects `Ddl` / `Dml`+`Copy` / `Statement` per flag, visiting subqueries — and **cannot see through EXECUTE**, so DML must be fenced at PREPARE | `context/mod.rs:2305-2339` |
| Native SET applies `datafusion.runtime.*` live (rebuilds the RuntimeEnv); native RESET restores **DataFusion's** default, not the Settings baseline | `context/mod.rs:1115-1219` |
| CREATE FUNCTION requires a `FunctionFactory` (`with_function_factory`); the body arrives as a parsed `Expr` | `context/mod.rs:2204-2227`, `:474-481`, `:1481-1486` |
| SHOW TABLES rewrites to `SELECT * FROM information_schema.tables` and **errors when information_schema is off** (today's default); all information_schema views enumerate via `SchemaProvider::table_names()` across every schema — a separate snapshot schema hides nothing, only a filtering provider does | `datafusion-sql-54.0.0/src/statement.rs:1627-1636`; `datafusion-catalog-54.0.0/src/information_schema.rs:96-216` |
| `TableProvider::statistics` is unused by mainline DF — a snapshot-native provider buys no stats | `datafusion-catalog-54.0.0/src/table.rs:312-315` |
| The DFParser statement space is closed (five variants, matched without a wildcard); the sqlparser space is wildcard-refused — default-deny protects against new statement kinds | `datafusion-sql-54.0.0/src/parser.rs:285-296` |

## 3. Why not providers for lifecycle (the investigation's central question)

Two designs were produced from opposite premises — provider-maximal and interception-minimal —
and converged. DF 54's provider traits cannot carry table lifecycle:

- **CTAS**: by the time `SchemaProvider::register_table` fires, the result is already whole in
  RAM inside a finished `MemTable`, and the hook is sync — no await, no streaming to disk. The
  only place that can spool a CTAS result is in front of `ctx.sql`.
- **DROP**: `deregister_table` carries no caller identity, and routine internals call it
  constantly — `Engine::register` deregisters before re-inferring on every re-scan, snapshot
  retirement deregisters, DF's own `CREATE OR REPLACE VIEW` deregisters. A provider that deleted
  files there would delete user data on a sidebar refresh. Authorization cannot live in a trait
  that cannot see its caller.
- **Write-back**: a provider that accreted native-DDL state would need the app to introspect it
  (a refetch — the `FetchCatalog` shape the store invariant forbids) or an event channel out of
  DF's trait machinery — the message-passing architecture the direct-call facade deleted.
  Interception returns the outcome as a value, which folds into the store like every existing
  mutation.
- **Half the scope never touches the traits**: SET, PREPARE, COPY, CREATE FUNCTION are
  session-level. The router must exist for them regardless.

Where a hook *does* see what we need, it is used: INSERT executes through
`TableProvider::insert_into` (stock `ListingTable`), and enumeration goes through our own
`table_names()` (§5). A custom `TableProvider` for internal tables was examined and rejected — a
delegating wrapper over `ListingTable` would track a visibly growing trait for no behavior we
need; a snapshot-native provider was rejected because its two claimed wins are already delivered
(`with_file_sort_order` on the listing registration; exact null counts from the spool's
`SnapshotStats`) and the third (`statistics`) feeds nothing (§2).

## 4. The router

`policy_block` (`engine/sql/validate.rs:343`) becomes the classification half of a router:

```rust
pub enum Capability { Editor, Agent }

pub enum Verdict {
    Query,                  // SELECT / EXPLAIN / SHOW / DESCRIBE / EXECUTE → snapshot pipeline
    Intercept(StmtKind),    // engine-method implementation + store write-back
    Refuse(Blocked),        // rendered by the consumer, per surface
}

pub fn classify(stmt: &DFStatement, cap: Capability) -> Verdict;
```

with `StmtKind` covering `CreateExternalTable`, `CreateTable`, `Ctas`, `Insert`, `DropTable`,
`CreateView`, `DropView`, `Copy`, `Set`, `Reset`, `Prepare`, `Deallocate`, `CreateFunction`,
`DropFunction`.

- **One predicate, one new axis, zero copies.** `Capability::Agent` returns exactly today's
  answers — every non-query a `Refuse` carrying the same `Blocked` variant — so
  `Engine::policy_verdicts` stays as the agent-facing wrapper and `strata-agent/src/tools.rs`
  does not change. The parity tests become a per-capability matrix, plus a pin that Agent refuses
  everything Editor intercepts.
- **Fail closed, default deny.** Parse failure is still `Err` ("could not judge"); the sqlparser
  wildcard still lands `Refuse(Unsupported)`; the DFParser match stays wildcard-free so a new DF
  variant is a compile error.
- **The editor's refusal set shrinks to almost nothing; `Blocked`'s existing variants stay
  defined as the agent path's error messages.** `Capability::Agent` refuses
  `CREATE EXTERNAL TABLE`/`CREATE TABLE`/`INSERT`/`CREATE VIEW`/`DROP VIEW`/`DROP`/`COPY`/`SET`/
  `RESET` exactly as today — the agent error path renders `editor_message()` and `strata-agent`'s
  parity tests name `Blocked::CreateTable`/`Insert`/`CreateDatabase` directly (`error.rs:145`,
  `tools.rs:1762`) — while on the Editor path every one of those statements classifies
  `Intercept` and runs, so those variants are unreachable there. New refusals join the vocabulary
  for the cases with no sane meaning: INSERT into a non-internal target, `INSERT OVERWRITE`,
  owned/runtime/format-key `SET`, non-query `PREPARE`, reserved names — same register (terse
  sentences, single-quoted identifiers).
- **What the editor still refuses, in full**: `CREATE DATABASE`/`SCHEMA` (structurally impossible
  — §5), transactions and unknown statement kinds (default deny), the context-dependent refusals
  above, and unsupported clauses inside accepted statements (constraints, `TEMPORARY`,
  data-column lists on external tables). Everything else runs.
- **One statement per Run** (today's behavior, kept): a multi-statement buffer is judged per
  statement by diagnostics as now, and Run refuses a mixed batch with a policy message.
- **Reserved names, read and write**: an intercepted statement that references a
  `__snap_`-prefixed table — or **names one as its target** — is refused. The read half keeps a
  typed `COPY (SELECT * FROM __snap_3)` from writing `__strata_ord` into a user file. The write
  half keeps `CREATE TABLE __snap_2 …` / CTAS / `CREATE VIEW __snap_2` / `INSERT` / `DROP` off
  the snapshot namespace: `snapshot_name` is `__snap_{seq}` off a counter starting near zero
  (`engine/mod.rs:517`), and a collision is invisible in SHOW/information_schema because the same
  prefix filters it. (ED-03 correction: `register_table` is **not** last-write-wins — the provider
  keeps `MemorySchemaProvider`'s "already exists" error — so the collision costs a *Run*, failing
  on a name the user cannot see, rather than silently displacing their table. Same conclusion,
  worse failure than the one first written down.) Defense in depth at the funnel:
  `register_external` refuses a reserved-prefix spec name too, which also covers a
  Configure-typed or hand-edited def.

**Dispatch.** New facade entry:

```rust
pub enum RunOutcome {
    Rows(QueryOutput, RecordBatch),      // exactly today's query() result
    Statement(StatementReport),          // no snapshot
}

pub struct StatementReport {
    pub kind: StmtKind,
    pub message: String,                 // IDE register, states session scope where relevant
    pub count: Option<u64>,              // rows created / inserted / exported
    pub elapsed_ms: u128,
    pub effect: Option<StoreEffect>,     // what the app folds into ProjectState
}

pub async fn run(&self, ws: WsId, tag: RunTag, sql: String, page_size: usize)
    -> Result<RunOutcome, String>;
```

`Verdict::Query` delegates to today's `query()` byte-for-byte — same supersede, same
retire-on-dispatch, same pins. **Only the query arm touches the snapshot lifecycle**; DDL never
retires a snapshot (SNAPSHOT_SPEC's "DDL / catalog changes do not retire snapshots" stands).
`Verdict::Intercept` goes to a new `engine/ddl.rs` submodule; long-running kinds (CTAS, COPY)
register in-flight entries so `cancel`/`is_running`/the close confirm keep working, and their
cleanup removes partial output like `run_and_snapshot` does. `Verdict::Refuse` returns
`Err(editor_message())` — the run fails in the results pane with the words the squiggle showed.

**The `SQLOptions` triple becomes per-class defense-in-depth** behind the front classification:
the read path keeps all-false (`query.rs:450`, `explain.rs:23`, unchanged); INSERT dispatches
dml-only; PREPARE/EXECUTE/DEALLOCATE statements-only; CTAS's inner SELECT all-false. Since
`verify_plan` visits subqueries, smuggled nested DDL still dies at the second gate. Dry-plan
validation stays side-effect-free for DDL — planning builds the `Ddl`/`Dml`/`Copy` node without
executing (execution lives only in `execute_logical_plan`), so typed DDL gets name-resolution
squiggles for free.

## 5. The provider layer — identity and visibility, never lifecycle

`engine/providers.rs`, installed in `build_context` via `register_catalog` under the existing
`strata` name:

- **`StrataCatalogProvider`** — exactly one schema, `public`; `register_schema` /
  `deregister_schema` refuse structurally, so `CREATE SCHEMA` is impossible by construction, not
  just by policy (the `is_owned_key` config fence keeps its job as the second layer).
  **`CREATE DATABASE` is not, and cannot be** (ED-03 correction to the line above): DataFusion's
  `create_catalog` registers into the `CatalogProviderList`, not into a `CatalogProvider`, and
  `CatalogProviderList::register_catalog` returns an `Option` with no way to fail
  (`datafusion-54.0.0/src/execution/context/mod.rs:1030-1050`). A refusing list could only lie
  ("catalog already exists") or silently no-op, both worse end-states than a refusal — so the
  router's `Blocked::CreateDatabase` is its only gate, and the first line for `CREATE SCHEMA` too.
- **`StrataSchemaProvider`** — tables keyed by **folded** name (defense in depth for the one
  case-insensitive namespace); `table_names()` filters `__snap_`-prefixed entries while `table()`
  resolves everything, so every existing reader, `DROP`'s `find_and_deregister`, validation's
  `table_exist` and snapshot retirement work with zero call-site changes. The prefix predicate
  lives next to `snapshot_name` in `query.rs` (`is_snapshot_name`) and is imported here, so the
  hiding rule and the naming rule cannot drift. Everything else is `MemorySchemaProvider`'s
  behaviour verbatim, its duplicate-name error included. Folding on **both** sides is what makes
  the namespace genuinely case-insensitive: `SELECT * FROM "MyView"` now resolves the view named
  `MyView`, where DataFusion alone would treat the quoted spelling as a different table. The
  fold-preservation oracle moved with it, pinning the stored identity rather than which spellings
  resolve.

Companion decision: flip `datafusion.catalog.information_schema` default **on** (still a
user-facing Settings key, not owned). Today `SHOW TABLES` is policy-allowed but fails at plan
time on a fresh project; behind the filter it works and agrees with the sidebar (modulo
`Reg::Failed` rows, which are unregistered and thus absent from SHOW — documented, and exactly
why the store remains the catalog authority). "Default" here means two things kept in step:
`build_context` sets it on the `SessionConfig` **before** the override loop, so a user's `false`
still wins, and `ENGINE_KEYS` names `true` so a *removed* override lands back on what the engine
was built with rather than on DataFusion's own `false`.

## 6. Per-capability design

### 6.1 Internal tables — CTAS, CREATE TABLE, INSERT, DROP TABLE

**Layout**: `.strata/tables/<slug>/part-<n>.arrow` — folded name, filesystem-sanitized with a
short hash when sanitizing changed anything; LZ4-frame Arrow IPC (the snapshot codec, and the
same one DF's Arrow sink writes). `ensure_gitignore` adds `tables/`; `tidy_strata_dir` sweeps
`.strata/tables/.tmp-*`. The persist funnel keeps owning `.strata`'s metadata files; the engine
owns the `tables/` payload with the snapshot writer's discipline (tmp + rename, tidy on open).

**CTAS** (`Intercept(Ctas)`) — **as built (ED-04); this replaces the draft's rendered-SQL
mechanism.** The parsed statement goes to `SessionState::statement_to_plan`, whose
`CreateMemoryTable { name, constraints, input, if_not_exists, or_replace, column_defaults }` is
everything the arm needs, and the spool is a `LogicalPlan::Copy` node built over `input` directly
— `CopyTo::new(input, "<data_dir>/.tmp-<pid>-<n>/", [], format_as_file_type(ArrowFormatFactory),
{})`, driven through `DataFrame`. **No SQL text is rendered and no span is sliced**: the query
that runs is the parsed query. (Slicing was rejected on evidence — sqlparser's `Spanned` impls
carry `todo` gaps and `Location` is character-based, the same offset arithmetic over judged text
`PolicyRefusal` already refuses; and re-rendering would be a fidelity claim about a round trip
nothing verifies.) Planning is side-effect free, so this also inherits DataFusion's own
exhaustive clause refusals — `TEMPORARY`, `LOCATION`, `PARTITION BY` and fifty more, each in its
own words — and its resolution of a declared column list against the query (cast + rename). What
is refused here is what DF *plans without enforcing*: constraints and column defaults, plus
duplicate result column names (DF's `ensure_unique_column_names` rule, which its own projection
check does not cover for a join's two same-named fields; an IPC file would store both and every
later read would degrade). A `__snap_`-prefixed target refuses at the router (§4 reserved names),
and `register_external` backstops it with the same `Blocked::ReservedName` wording. `IF NOT
EXISTS` / `OR REPLACE` / plain-exists resolve against the engine's own namespace
(`ctx.table_provider`, tables + views, case-insensitive, a view refused outright) — which is the
store's namespace minus `Reg::Failed` rows, and a create over one of those replaces the broken
def rather than erroring, because a shadow copy of the store's names inside the engine would be
the second catalog the invariant forbids. Rename tmp → `.strata/tables/<slug>/` (atomic; a crash
leaves only a tmp dir the tidy sweeps). Zero-row results and plain `CREATE TABLE (cols…)` — whose
plan is an `EmptyRelation` — write one empty IPC file carrying the schema (IPC self-describes, so
replay infers without a schema in the def). Register through the existing funnel:
`register_external` with `TableSpec { format: Arrow, paths: [dir], internal: true }` →
`TableMeta` → `StoreEffect::TableUpserted { def, meta }`. `Engine::set_data_dir(root)` takes the
**project folder** at open (both hosts) — the data directory and the def's project-relative
source path are both derived from it — and CTAS refuses politely when unset.

**One DataFusion default had to move for any of this to work.** DF 54 runs a `ListFilesCache` by
default (1 MiB, **infinite TTL**), so a re-listing of a table's directory returns the previous
answer. `CREATE OR REPLACE` failed outright against it, and D5's "a re-scan picks up new files"
promise — the catalog's ↻ and the Configure window's re-inference — was already quietly broken by
it. `ENGINE_KEYS` now names `0` as Strata's default for
`datafusion.runtime.list_files_cache_limit` and `build_runtime` applies it before any override
(and therefore always builds a runtime). It stays a default, not an owned key.

**INSERT** (`Intercept(Insert)`, native execution): the interception only gates the target —
resolve the parsed target name against the engine's internal-name set and refuse an external
table or a view ("'events' is an external table. INSERT targets internal tables"); refuse
`INSERT OVERWRITE` before the Arrow sink's `not_impl` would. Then dispatch the user's own text
via `ctx.sql` with dml-only options: `ListingTable::insert_into` appends one schema-checked
IPC file. One file per INSERT, no compaction — documented; `DROP` + CTAS is the compaction story
until a task exists. The fold requests `ScanScope::Table` so `TableMeta.rows` refreshes through
the scan driver, never store-side arithmetic.

The engine-side internal set (`TableSpec.internal` recorded at registration, folded names) is
**not a second catalog** — it is derived state rebuilt by the same registration pass that builds
everything else, and it answers exactly one engine-side question: may a write statement target
this provider. The store remains the only UI-facing catalog.

**DROP TABLE** (`Intercept(DropTable)`, both origins): `cancel_profile` → `ctx.deregister_table`
→ (internal only) delete `.strata/tables/<slug>/` → `StoreEffect::TableRemoved`. Deregister-first
means no new plan can scan it; an in-flight scan holds open fds or fails as cleanly as a retired
snapshot. External: def removal only — "'x' removed from the catalog. Source files were not
deleted". No cascade: the report names dependent views from the store's `ViewInfo` deps; they go
`Reg::Failed` honestly on the next pass (a `ViewTable`'s inlined plan keeps executing until
reload — D11's verified finding — and the epoch bump makes diagnostics re-derive immediately,
which is the surface that matters). `IF EXISTS` honored. Snapshots are unaffected by design —
results are materialized copies.

**Arrow row counts**: a thin `StrataArrowFormat` wrapping DF's `ArrowFormat` implements
`infer_stats` by reading Arrow IPC footers — exact row counts from metadata-only reads (each
batch header carries its length) — used by the `SourceFormat::Arrow` arm, so internal tables (and
external Arrow tables) get real `TableMeta.rows` while staying "only real facts". Null counts
deliberately stay profile/pre-flight territory; nothing displays table-level null counts.
Constraints and column defaults stay refused in v1 — DF does not enforce constraints even on
`MemTable`; a delegating provider wrapper for INSERT defaults, and a RAM-caching wrapper for a
measured-hot table, are noted future extensions.

**Replay**: an internal def is `TableDef { name, format: Arrow, sources:
[".strata/tables/<slug>/"], partition_cols: [], origin: Internal }` — the existing
`register_pass` / `register_project` replays it with zero new code, headless host included. A
clone without the gitignored data yields an honest `Reg::Failed` row from the existing
no-files mapping.

### 6.2 Typed CREATE / DROP VIEW

`Intercept(CreateView)`: extract the folded name and the definition query's canonical rendering
(what lands in `ViewDef.sql`), then fence and delegate:

- The name resolves to a **base table** → refuse ("'sales' is a table") — this closes DF's own
  silent table-replacement hazard (§2).
- A `__snap_`-prefixed view **name**, like a `__snap_` reference in the body → refuse (§4
  reserved names, both halves).
- Plain `CREATE VIEW` over an existing view → "View 'v' already exists. Use CREATE OR REPLACE
  VIEW."
- Otherwise → the existing `Engine::create_view(name, sql)` — one implementation shared with ⌘S,
  returning `ViewMeta` for the same fold the Save flow uses. Running the statement natively was
  rejected precisely because the store write-back needs `ViewMeta` (columns + `plan_deps`);
  intercept-and-delegate gets the outcome from the engine's answer, no introspection.

`Intercept(DropView)`: type-check, `Engine::drop_view` (idempotent), `StoreEffect::ViewRemoved`.
Replay ordering stays covered by `view_order` since the def lands in the same collection.

### 6.3 COPY TO

`Intercept(Copy)`, native execution after a pre-flight:

1. Partition idents must be bare words — reuse the export module's check and wording (DF 54's
   COPY parser re-renders quoted idents broken).
2. **The NULL-partition gate survives, as a pre-flight**: when `PARTITIONED BY` is present, run
   `SELECT count(*) FILTER (WHERE "p" IS NULL) …` over the source first; proceed only on exact
   zero per column, same wording as `partition_columns_have_no_nulls`. Cost: one extra scan per
   partitioned typed COPY — the Export window keeps getting its counts free from the spool's
   `SnapshotStats`; the typed path pays for generality. (DF 54 misfiles NULL partition values
   into a neighbouring value's directory; schema nullability is not a signal — DF reports every
   column nullable.)
3. Dispatch the user's statement text natively; report "Exported N rows to '<path>'" from the
   sink count.

Also in this task: `run_export` sets `datafusion.execution.keep_partition_by_columns` per export
and never restores it — invisible today, observable once SET and `df_settings` are real. Fix by
save/restore or the overlay.

### 6.4 SET / RESET

Engine-implemented, never native (§2 gives both reasons: runtime keys applied live bypassing the
restart discipline; RESET landing on DF defaults instead of the Settings baseline — a second
config authority). `Engine::set_session` / `reset_session`:

- Refuse owned keys (`is_owned_key`), `datafusion.runtime.*` ("Engine runtime options are set in
  Settings") and `datafusion.format.*` (display keys are Settings territory — a session format
  change would split-brain the grid formatter and chart-read cache identity, which key off the
  Settings store's display subset).
- Otherwise apply to the live ctx and record in `session_overlay: Mutex<BTreeMap<String,
  String>>`. `RESET k` removes the overlay entry and re-applies the **Settings baseline** (or the
  DF default when unset). Engine-wide (all tabs, agent reads included), gone on restart; the
  report says "for this session". A `set_config` restart drops the overlay silently —
  documented. `SHOW VARIABLES` / `df_settings` then truthfully reflect the live session.

### 6.5 PREPARE / EXECUTE / DEALLOCATE

PREPARE is intercepted only to verify its **inner** plan with the read-path `SQLOptions`
(`verify_plan` cannot see through EXECUTE, so DML/DDL are fenced at PREPARE: "PREPARE supports
queries"), then dispatched natively — DF stores the optimized plan in session state. EXECUTE
classifies as `Verdict::Query` and rides the full snapshot pipeline (ordinal, page 1, stats all
unchanged) under statements-only options — safe because PREPARE gated the inner plan. DEALLOCATE
is native plus a one-line report. DF's prepared-plan map is `pub(crate)`, so the engine keeps a
name → param-types mirror feeding completion. All of it dies with the session.

### 6.6 CREATE FUNCTION

`StrataFunctionFactory` installed at `build_context`. v1 accepts SQL-bodied scalar functions
(`CREATE FUNCTION f(x BIGINT) RETURNS BIGINT RETURN x + 1`) as a `ScalarUDF` substituting
arguments into the stored body `Expr` (the upstream `function_factory.rs` pattern); other
languages and non-scalar forms refuse tersely. After a successful CREATE/DROP FUNCTION the engine
**re-snapshots** the function catalog: `Engine::functions()` becomes swappable (`Arc` behind a
lock) with a revision counter the completion layer keys on, so autocomplete/signature/docs see
the change on the next keystroke — the live-registry invariant kept honest. Session-scoped, not
persisted; if persistence is ever wanted it is a `FunctionDef` list in `project.json` replayed by
the pass — deferring costs nothing.

### 6.7 Typed CREATE EXTERNAL TABLE

`Intercept(CreateExternalTable)` — the typed form of Table Config: the parsed statement maps onto
an ordinary external `TableDef` and rides the same funnel, so Table Config and typed DDL are two
gestures into one registration path, exactly as ⌘S and typed `CREATE VIEW` are for views. DF's
native path (`TableProviderFactory` → registration behind the store's back) is never used — the
def, not the engine registration, is the durable artifact.

- **Mapping**: `STORED AS` → `SourceFormat` (`PARQUET`/`CSV`/`JSON`/`ARROW`; anything else
  refused by name — the Avro-fallthrough rule, P4-11); `LOCATION` → one source, relativized when
  under the project root (Configure's own rule); `PARTITIONED BY` → `partition_cols`;
  `OPTIONS(…)` → the matching `CsvRead`/`JsonRead` fields (`format.has_header`,
  `format.delimiter`, quote/escape/comment/compression/newlines-in-values/infer-rows). **Any
  OPTIONS key with no def field is refused by name** — a silently dropped option is a def that
  lies about how the table reads.
- **Column lists**: accepted only where every listed column is a partition column (its declared
  type carries into the def, checked against the supported partition types —
  Utf8/Int32/Int64/Date32). Data columns refuse: "Schemas are inferred. Remove the column list."
- Also refused, loudly: constraints, `ORDER BY` clauses, `UNBOUNDED`, `TEMPORARY`, a reserved
  `__snap_` name (§4). `IF NOT EXISTS` honored against the store's namespace.
- **Outcome**: `register_external` from the built def → `TableMeta` →
  `StoreEffect::TableUpserted { def (origin External), meta }` — the identical fold, persist and
  epoch bump as CTAS's (§7). `Blocked::CreateExternalTable` and its message stay as the agent
  path's refusal.

## 7. Integration dataflow (CTAS end to end)

At Run: `Engine::run` classifies → `Intercept(Ctas)` → spool the inner SELECT to
`.strata/tables/t/part-0.arrow` → register via `register_external`
(`TableSpec { format: Arrow, internal: true }`) → return `RunOutcome::Statement` carrying
`StoreEffect::TableUpserted { def, meta }`.

At settle — byte-for-byte the `save_view` shape (`editor/actions.rs:254`): store upsert on
`ProjChan::Tables` (the sidebar shows the row immediately, `Reg::Ready(meta)`) → `persisted_defs`
rewrites `.strata/project.json` atomically through the persist funnel → `catalog_settled` epoch
bump (diagnostics revalidate; other tabs resolve `t`) → history + event log → the results pane
renders a statement row ("Table 't' created, 1,204 rows") — no grid, no snapshot.

At next open — zero new code: `load_defs` → `from_defs` → the scan driver → `register_pass` →
`table_spec` resolves the project-relative source against the root → `register_external`
re-registers over the same files. The headless host replays identically. DROP runs the loop
backwards (deregister → delete dir → `TableRemoved` fold → persist → bump); INSERT appends a file
and its effect requests `ScanScope::Table`.

An internal table is an ordinary def whose sources live under `.strata/`, so the scan driver,
persist funnel and headless host handle it with no new code — that half of the claim holds, and it
is what makes replay free.

**But "there is no new integration surface" was too strong, and the catalog pane is the
correction** (found while building ED-03; settled 2026-08-08). Landing in `ProjectState.tables` is
required — the store *is* the catalog — and the consequence is that an internal table inherits
every affordance a table row has, three of which do not mean the same thing on a def whose data
Strata owns:

- **Configure does not apply at all.** It edits sources, format and partition columns, and an
  internal table has none of those to edit, ever. The item is **absent** from the row menu, which
  is the catalog's existing treatment for an item that could never apply to a row kind (the view
  menu has no Refresh, for the same reason) rather than parking, which means "not right now".
  With the item gone the window cannot receive an internal def — `ConfigureTarget::Edit` is set
  only from that menu and from Configure's own post-save transition on a *New* table — so it
  needs no internal mood and must not grow a guard for one. This **replaces** the earlier
  read-only-window design.
- **Drop is one action with two entry points, and they must be one funnel.** The sidebar's drop
  and the editor's `DROP TABLE` both destroy an internal table; drafted separately, the sidebar's
  would have removed the def and left `.strata/tables/<slug>/` orphaned, under a dialog whose
  fixed copy promises "files on disk are not deleted". Both go through `engine::ddl::drop_table`,
  with the dialog as the confirm in front of it, and the origin-dependent wording stated once.
- **The row has to show which origin it is** — it is what stands between the user and that drop.

**Refresh** is the one that survives unchanged, and it is load-bearing: re-inference is how row
counts move after an INSERT.

Task ownership: the row and the menu are ED-04's (where `TableOrigin` is introduced), the drop is
ED-05's.

## 8. Lifetimes (state in one table)

| Thing | Lives | Survives restart | Persisted |
|---|---|---|---|
| Internal table data | `.strata/tables/<slug>/` | yes | yes (gitignored) |
| Internal table def | `project.json` + store row | yes | yes (shareable) |
| Views (either gesture) | `project.json` + store row | yes | yes |
| SET overlay | `Engine.session_overlay` | no | no |
| Prepared statements | DF session state + engine mirror | no | no |
| Created functions | DF registries + function-catalog snapshot | no | no |
| Snapshots | temp dir, retire-on-dispatch | no (by design) | no |

## 9. Honest costs (accepted, documented)

- **CTAS name-semantics ownership**: `IF NOT EXISTS`/`OR REPLACE`/unique-columns are
  reimplemented beside DF's arm; new DF clauses arrive as refusals, not silent misbehavior (the
  interceptor matches the parsed statement exhaustively on the fields it understands).
- **`ListingTable::insert_into` internals** (file-per-INSERT, append-only Arrow, collection URL)
  are observed behavior, not contract — pinned by an integration test in the style of the
  snapshot-order tests.
- **File growth**: one IPC file per INSERT, no compaction v1.
- **SET re-implementation** duplicates a slice of DF's `set_variable` (~50 lines) — deliberate;
  the alternative is a second config authority and an owned-key bypass.
- **FunctionFactory subset**: SQL scalar macros only; expansion is additive, refusals are the
  contract.
- **Three session lifetimes** the user can conflate (overlay, prepared, functions) — every report
  string says "for this session"; §8 is the reference.
- **Two-agent races** (editor DDL vs a concurrent agent read) yield clean scan errors, the same
  class as snapshot retirement — no locking added, the registration pass's existing stance.

## 10. Invariant amendments (landed with the owning task, per the upkeep rule)

| Settled text (AGENTS.md §2 / reference file) | Amendment | Lands with |
|---|---|---|
| "Managed DDL policy. The editor runs SELECT/EXPLAIN/SHOW/DESCRIBE only…" (INVARIANTS.md, ENGINE.md) | ✅ **Landed.** Became the router invariant (classification, ED-01) plus the dispatch invariant (`Engine::run` routes, only its query arm touches the snapshot lifecycle; a statement's outcome is a value one fold applies — ED-02) | ED-01/ED-02 |
| "Views are Save's artifact… typed DDL is blocked" | Typed view DDL is a second gesture into the same funnel | ED-06 |
| "History is a list of queries… only successful data runs" | ✅ **Landed.** Successful statements enter history too, `count` as the rows moved; dedupe/cap and the success-only rule unchanged | ED-02 |
| "DROP is not supported in the editor. Deregister tables from the catalog" (message + routing) | ✅ **Landed.** DROP TABLE works on both origins from the editor; the catalog confirm remains for the pointer gesture and is now a gesture in front of the *same* call (`ddl::tables::drop_table`) rather than its own deregister, so an internal table's data cannot be orphaned by one path and deleted by the other. `Blocked::Drop` stays as the agent path's message | ED-05 |
| "COPY TO is not supported in the editor. Use Export" | Editor COPY dispatches natively behind the pre-flight NULL gate; Export window unchanged | ED-07 |
| "CREATE EXTERNAL TABLE is not supported in the editor. Register tables in Table Config" | The typed form intercepts onto the Table Config funnel (def-first); message stays as the agent refusal | ED-10 |
| "SET is not supported in the editor. Engine options are set in Settings" | Session overlay for non-owned, non-runtime, non-format keys; Settings stays the durable authority | ED-08 |
| Agent access "Read-only v1" (AGENT_ACCESS_SPEC §1) | **Unchanged** — restated with the capability parameter | ED-01 |
| Snapshot lifecycle ("DDL does not retire snapshots", no epoch in the query key) | **Unchanged** — the query arm is byte-for-byte today's path | — |

## 11. Workstream

`.claude/tasks/workstream-editor-statements/` — ED-01…ED-10, ordering and dependencies in its
README. ED-01 (router) and ED-02 (`Engine::run` + statement results) unblock everything;
ED-04 → ED-05 is the only hard chain; ED-03/06/07/08/09/10 parallelize after ED-02.
