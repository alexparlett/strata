# Strata — engineering practices

The **how-we-work** companion to [CLAUDE.md](CLAUDE.md) (the *what/where* map: build, workspace
layout, module map, docs index, backlog). Every rule here was settled deliberately during the Freya
rewrite — most after a wrong version was built and rejected in review — so treat them as decisions,
not suggestions.

**This file is the index of rules; `docs/reference/` holds the reasoning.** Each line below is the
rule in its actionable form — act on it as written. Its full entry, with the failure it exists to
prevent, is in the linked file under **the same bolded lead sentence**, so it greps:

| § | Rules | Full text |
|---|---|---|
| 1 | The engineering bar | [reference/BAR.md](docs/reference/BAR.md) |
| 2 | Architecture invariants | [reference/INVARIANTS.md](docs/reference/INVARIANTS.md) |
| 3 | Freya component & UI conventions | [reference/FREYA_UI.md](docs/reference/FREYA_UI.md) |
| 4 | State: where things live | [reference/FREYA_UI.md](docs/reference/FREYA_UI.md) + `docs/FREYA_STATE_ARCHITECTURE.md` |
| 6–7 | The fork, git, verification | [reference/WORKFLOW.md](docs/reference/WORKFLOW.md) |

**Read the full entry before extending, arguing with, or overturning a rule** — several of these
read as over-engineering until you know what was tried first. If a design seems to conflict with
one, that is the signal to open the reference, not to route around it.

**Upkeep:** when a review settles a new convention or overturns one, update **both** the one-liner
here and the full entry in `docs/reference/` in the same change. Session memory may restate rules
from here; this repo is the authoritative copy — if they disagree, trust the repo and fix whichever
is stale.

**Scope:** the `strata-*` crates and app-level work. The Freya fork (`crates/freya`) carries its own
`AGENTS.md` with the upstream author's conventions (`just` not raw cargo, `crate::` not `super::`,
doc comments over inline comments, no em dashes, `KeyExt` on components) — follow that file when
editing fork code, and §6 here for how the fork relates to the app.

---

## 1. The engineering bar

- **Generic capability, not hardcoded subsets** — the real mechanism, not a stub that passes today's case.
- **Real end-states, not placeholders.** No TODO scaffolding as the deliverable. One sanctioned
  exception: a deliberately **inert control** whose capability another task owns (§5).
- **Native Rust tooling, not stray scripts.** Schema/codegen/tests live in the crate.
- **Verify from source before agreeing.** Check the fork or the crate before confirming an API —
  Alex's assertions included. Same bar for your own claims.
- **Framework-native idiom — never pattern-carrying.** Find the Freya/freya-query shape first: no
  adapters, echo fields, parallel ids, or shims. The deleted Dioxus app's patterns stay gone.
- **Model impossible states out of existence; fail loud on the rest.** Expected absences get
  defaults; unrecoverable faults are surfaced (`ProjectLoadFailed`), never a silent blank fallback.
  Never shape a production signature or add an `Option` to satisfy a test — build the test's store
  inline and pull deps from context.
- **No over-engineering.** Private app: use `pub` freely; don't hand-annotate per-field visibility.
- **A path is qualified in the `use` and nowhere else.** Import the *item*, use its bare name;
  importing a module to qualify through is the same rule broken. Exceptions: visibility modifiers,
  intra-doc links, and the `std` aliases whose module segment disambiguates (`io::Result`). On a
  genuine collision, alias with `as` — never reach through the crate root.
- **Valin-shaped.** Follow [`marc2332/valin`](https://github.com/marc2332/valin) for module layout,
  per-window data scoping, and stateful tabs.

## 2. Architecture invariants

Things that must not regress. Full text: [docs/reference/INVARIANTS.md](docs/reference/INVARIANTS.md).

**Engine, query, results**

- **The engine is a direct-call async facade.** No UI-side runtime, channels, request ids, or
  router. DataFusion is touched **only** in `strata-core`.
- **Results are freya-query off the tab's SQL.** The store holds specs, never results. A Run
  subscription is built **only** through `QuerySpec::query`; cache-entry lifetime is subscriber
  presence, held for background tabs by the request keepers. Never manage entry lifetime imperatively.
- **An expensive, opt-in *result* is freya-query keyed by the request; the store holds the request.**
  A re-scan is a new nonce; invalidating is dropping the request. Never a results field, dedup set,
  or spinner flag. The `Query` is cache identity, so it is built in **one** place.
- **One entry point per expensive action, with the confirm in front of it** (`ProfileActions::ask`).
- **An internal table is an ordinary def whose data Strata owns, and `TableOrigin` is a flag on
  that def rather than a second kind of thing.** CTAS spools into `.strata/tables/<slug>/` and
  registers through `register_external`, so the store, the persist funnel and replay need no new
  code. The flag answers three questions only — may a write target it (`Engine::is_internal`,
  never a second catalog), does a drop delete data, can Configure edit it (no: the item is
  *absent*). The def travels and the data does not, and the failed row says so in its own words.
  The spool is the **parsed plan**, never re-rendered SQL.
- **A table is dropped in one place, on both origins, and a confirm is a gesture in front of that
  place — never a second implementation of it.** `ddl::tables::drop_table` resolves the target,
  deregisters, deletes `.strata/tables/<slug>/` **only** for an internal def, and names the
  dependent views without cascading; the catalog pane reaches it through `Engine::drop_table` after
  its store-first write, a typed `DROP TABLE` through the router. The pane's own `deregister`
  orphaned an internal table's data forever. The two wordings (`ddl::drop_intent` before,
  the report after) are the engine's, so the card cannot promise what the report then contradicts.
- **A table's data is discarded by rename, and the drop that does it is background work.** The
  directory moves to a `.tmp-…` sibling and is only then walked (`ddl::tables::discard`), so an
  interruption leaves what `tidy_strata_dir` already sweeps rather than a half-emptied directory
  under a live table name; the rename is the operation and the removal is housekeeping, so a
  failure to finish it is logged, never reported as a failed drop. And because one `INSERT` is one
  file with no compaction, that delete is not instant — so it holds a `BackgroundGuard`
  (`Lifecycle::background`, shared with export) and the close confirm asks before a window takes
  the runtime away mid-delete.
- **A write statement only ever reaches files Strata owns, and the gate is the *parsed* target.**
  `INSERT` asks `Engine::is_internal` about the target its plan names and otherwise runs
  DataFusion's own INSERT path — one appended IPC file per statement, no compaction, and the
  schema check is DataFusion's. The plan that was judged is the plan that runs; `Blocked` carries
  every refusal's wording, including the two only a plan can name.
- **A typed `CREATE EXTERNAL TABLE` is Table Config's registration written down, and its `OPTIONS`
  are the table's reader — never the store's.** `ddl::external` reads the parsed statement into a
  `TableDef` and hands it to `register_external`; DataFusion's `ListingTableFactory` stays unused
  because the **def** is the durable artifact. Read exhaustively with no `..`, so every clause a
  def cannot carry is refused **by name**, `STORED AS` included (no fallthrough, no minted
  `Unknown`); an internal table's name is fenced off. `OPTIONS` is **two vocabularies wearing one
  syntax** — a `format.` key the def has a field for is read onto it (the key set *is* the def), a
  store namespace or a client option is refused toward Connections **on the key alone**, because
  the value may be a secret, and everything else is refused by name so the mechanism stays total.
  A `LOCATION` with a scheme is `project::split_remote` — `resolve_source` read backwards — onto a
  connection the project **has** (`Engine::connections`, membership not liveness), refused by name
  otherwise so DataFusion's "no suitable object store" never lands on a table row; the lookup
  **resolves** case-insensitively and answers with the connection's own spelling, because the
  registry does the first and every other surface addresses it by the second. The def reaches the
  funnel through `register::table_spec`, not a second copy of it. Configure's LOCATION toggle is
  unaffected: it exists so a typed *path* is never re-read as remote.
- **A view is Save's artifact, and typed view DDL is a second gesture into that funnel — one
  body, views indistinguishable by origin.** `ddl::views::create` serves ⌘S and the typed
  statement, so either gesture edits the row the other made; the statement never runs natively
  (DF's `CREATE OR REPLACE VIEW` silently replaces a *table* of that name, and the store
  write-back needs a `ViewMeta`). `ViewDef` is `{ name, sql }`, so the arm arrives at the folded
  name plus the definition **query's** canonical rendering — and because the statement is rebuilt
  around that query, every clause it can carry is refused **by name** from a destructure with no
  `..`, or `CREATE TEMPORARY VIEW` would create a permanent one. A drop names its readers in the
  table drop's own words (`ddl::left_invalid`) off the **aliases** half of `PlanDeps` — raw, so it
  over-reports on purpose — and never cascades. `Blocked::CreateView`/`DropView` stay as the agent
  path's refusals.
- **A `COPY … TO` may not land in storage Strata owns, and the gate is the *resolved* target.**
  The project's `.strata/` and the snapshot spool are refused; a stray file under an internal
  table's directory is read back as phantom rows by that table's next scan, and everywhere else on
  the disk is the user's own.
- **A typed `COPY` is DataFusion's own write behind the two checks the Export window used to stand
  in for, and the Export window is unchanged.** `ddl::copy::copy_to` plans once, gates that plan
  and drives it — no text re-rendered, so the plan judged is the plan that runs. The bare-word
  partition check is `export`'s, shared; the NULL-partition refusal is `export`'s wording reached
  by a **pre-flight count** over the planned input, since a typed COPY has no snapshot's free
  counts — one extra scan, exact zero or decline. A `__snap_` source is the router's
  `ReservedName`; the effect is `None`; `Blocked::CopyTo` stays as the agent path's refusal. And
  `keep_partition_by_columns` is stated in the statement's own `OPTIONS`, never as a session `SET`
  nothing restores.
- **A typed `SET` is a session overlay in front of Settings, and the overlay wins for its keys
  until `RESET` or restart.** Neither runs natively: native `SET` applies `runtime.*` live
  (bypassing `restart_owed`) and native `RESET` restores *DataFusion's* default, not the Settings
  one. `ddl::session` applies through `set_config`'s own `ConfigOptions::set` and records in
  `SessionScope`; a `RESET` drops the entry and re-applies `config::effective`. Owned,
  `runtime.*`, `format.*` and the parser **dialect** refuse toward Settings — on `RESET` as much
  as on `SET`; the last two are one rule (a key the app reads from the Settings store cannot have
  a session value, or two layers answer differently about one buffer). The
  overlay is engine-wide, and `set_config` skips a key it holds, so a Settings Apply records the
  baseline the eventual `RESET` lands on rather than overwriting what the user typed.
  `restart_owed` unchanged. And **writing an option is only half of applying it** — every writer
  also calls `refresh_config_dependent_udfs`, or `SET …time_zone` moves `SHOW` and leaves `now()`
  in the build-time zone.
- **A created function is a SQL macro, its catalog is swappable, and the name it may take is fenced
  against the built-ins.** `CREATE FUNCTION` runs over DataFusion's own `FunctionFactory` seam,
  installed on every engine; the UDF implements only `simplify`, substituting the call's arguments
  into the stored body. `Definition::read` is the one judgement, called by the arm for its wording
  and by the factory to build from. The body is an expression over the arguments and nothing else —
  a bare `Column`, a subquery or a `$n` past the arity is refused — and the **standard spelling does
  not plan**: DataFusion plans the body against an empty schema, so `bind_parameters` rewrites the
  bare `x` into its own `$x` placeholder on the parsed statement, before planning. `AS '<string>'`
  and every clause the planner drops silently are refused off that statement, from a destructure
  with no `..`. **A built-in is refused to both statements**, because `DROP FUNCTION` deregisters
  across all five registries and nothing can put one back; `Functions::created` is what names the
  difference, and `registered_function` asks **all five** — a three-registry fence read the
  higher-order-only names as free. The drop's own statement is read too, never trusted to the
  planner, which discards every name past the first. The catalog is re-walked by the statement that moved the registry and by nothing
  else, and there is **no revision counter beside it** — `FunctionsChanged` bumps the catalog epoch,
  which every consumer already keys on.
- **`PREPARE` runs natively because DataFusion owns the plan; the fence and the mirror are ours,
  and the fence can be nowhere else.** `verify_plan` descends into a `Prepare`'s input and an
  `Execute` has none, so a DML/DDL body is refused at `PREPARE` or never. The mirror exists only
  because `prepared_plans` is `pub(crate)`, and it is written **after** the dispatch so a duplicate
  name keeps DataFusion's error. `EXECUTE`'s widening is a `ReadPolicy` on the dispatch
  (`sql::read_policy`), never a mode `Engine::query` offers. Both carry
  `StoreEffect::PreparedChanged` — nothing persists, but a name resolves now that did not. A
  restart clears the lot by construction: a new `Engine` is a fresh `SessionScope`.
- **An append re-reads the table's facts; it does not re-register it, and it leaves the views
  alone.** Re-registering replaces the provider, and *that* is what strands the `Arc` a view
  captured — which is why a table Refresh re-creates them. An `INSERT` cannot change the shape a
  view captured (the sink schema-checks first) and the old provider re-LISTs per scan anyway, so
  `refresh_table_rows` → `Engine::table_meta` → `table_registered` is the whole fold: no
  re-inference, no view churn, no epoch bump, no `Loading` flash. Still read from the files,
  never added up store-side.
- **A re-scan means "list the sources again", so this engine runs no list-files cache.** DF 54
  turns one on by default with an infinite TTL, which silently serves ↻, Configure's re-inference
  and `CREATE OR REPLACE` the previous file set. `ENGINE_KEYS` defaults it to `0` and
  `build_runtime` applies that before any override. **The per-file statistics cache is the
  opposite call** — keyed per object, invalidated on size/mtime, so it spares only an unchanged
  file — and `register_external` must hand it over by hand (`ListingTable::with_cache`), as
  `register_listing_table` already does for snapshots.
- **A reader that outlives one Run pins the snapshot it reads** (`Engine::pin_snapshot`, RAII).
  Never a staleness check or warning instead. **A hold that protects spawned work belongs to that
  work, not to the call that started it** (`ExportHold`, weak so the runtime the task rides is not
  kept alive by it): a guard living in the caller's future releases when a UI scope drops it,
  leaving the detached write to be retired out from under itself.
- **The snapshot is Arrow IPC, so a result's type survives it.** Parquet cannot write a union at
  all; exact null counts now come from the write pass (`query::SnapshotStats`).
- **A stopped run is not a failed one, and `engine::stopped_on_purpose` is the only thing that knows
  which is which.** Three strings, not one. Never string-match the engine's prose at a call site.
- **An engine's config is a launch value; a live change is `set_config`, and a runtime key is a
  restart** — which is the `ProjectRoot` remount, not a second path. A **removed** key goes back to
  its `ENGINE_KEYS` default; `restart_owed` measures against `built_runtime`.
- **Strata owns the catalog and schema providers, for identity and visibility — never lifecycle.**
  One catalog, one schema (`register_schema` refuses, so `CREATE SCHEMA` is impossible by
  construction; `CREATE DATABASE` cannot be — the `CatalogProviderList` has no way to say no, so
  the router is its gate). One map keyed by `fold_ident`; `table_names()` hides `__snap_` while
  `table()` still resolves it, which is the *only* enumeration path DataFusion has and so is what
  makes `datafusion.catalog.information_schema` safe to default **on**. Everything else is
  `MemorySchemaProvider` verbatim. Lifecycle is intercepted in front of `ctx.sql` — a sync
  `register_table` with no caller identity can neither spool a CTAS nor authorize a `DROP`.
- **One classification with a capability axis, in front of dispatch.** `classify(stmt, Capability)`
  answers `Query` / `Intercept(StmtKind)` / `Refuse(Blocked)` off the parsed statement, both
  surfaces in one match arm. `Capability::Agent` stays read-only and message-identical; the
  editor's refusals shrink to a short list and the older `Blocked` variants stay as the agent
  path's messages; default stays deny. A `__snap_` name is refused to **every** statement the user
  types, read or written — a plain `SELECT` included, because reading a snapshot hands back another
  tab's result with `__strata_ord` showing and Export then writes that to a file, around the fence
  that exists to stop exactly this; `EXPLAIN` descends to its inner statement, and
  `register_external` and `ddl::views::create` backstop the write half at the two funnels a def can
  also arrive through. Every interception is a second gesture into a funnel that already exists.
- **`Engine::run` routes; only its query arm touches the snapshot lifecycle.** One statement per
  Run, `Query` delegating to `query()` byte-for-byte, `Intercept` to `ddl::execute` under the
  in-flight bracket `explain` shares, `Refuse` to the squiggle's own message before DataFusion
  can plan it. A statement's outcome is a **value the app folds** (`StoreEffect` → store channel
  → `persisted_defs` → `catalog_settled`), one fold for every effect, and its log entry belongs
  to that fold because only the fold knows whether the def was written.
- **A chart renders the result in result order; it computes nothing SQL can say.** `Engine::chart`
  is a projected, ordinal-ordered, capped read plus a long→wide pivot — no aggregation, no
  bucketing, no imposed order (the histogram's binning and the scatter trendline's `Engine::trend`
  fit are the two sanctioned exceptions; the fit is its own read keyed by the two encoded
  columns, so the toggle never re-reads the points). Over a cap, or two
  rows in one pivot cell, it refuses, naming the user's own `GROUP BY` as the fix. An engine-side aggregation pipeline
  was built and withdrawn; the reasons and the scan-order measurements are the full entry —
  re-litigate neither. A column's **chart role** comes from the Arrow `DataType` in `column_info`
  (its measure arm *is* the read's own `is_numeric` gate), never from a type's spelling or from
  `Kind` — and a time column is **two** roles, `Instant` and `Clock`, identical on an axis and
  different wherever a stride is, because a day-wide `date_bin` over a `Time` column is refused;
  and a chart read's cache identity is `(snapshot, query, **display config**)`, because
  axis labels render through `datafusion.format.*`.
- **A chart image is the chart, so the capture and the paint are one draw body.** Copy Image
  renders the canvas's own `Rc<Frame>` through the same `marks::draw` (a canvas + a
  `FontCollection`, never a `CanvasContext`), which **returns** its hit regions so a capture
  cannot overwrite the plot's. No paint pass: the font collection is a root context. Fixed
  1600x900 at 2x, background filled first, read back as unpremultiplied RGBA, nothing on disk.
  The fork's clipboard grew images rather than the app growing a save-to-PNG stopgap, **inside
  its existing shape**: the integration still provides a `Box<dyn ClipboardProvider>` into the
  root context. The trait is the fork's own now and covers images; copypasta was **replaced** by
  arboard rather than run beside it, because text and images are one clipboard.
- **A chart refusal names its fix in prose, and the strip has no control behind it.** The
  *Aggregate in SQL* press was built and cut: sound mechanism, wrong surface (no tool puts it
  among the encoders). The placement was re-litigated with a surface that isn't the strip —
  the **Shape panel** (Chart 09): a modal working panel off the results toolbar composing
  visible SQL into a new unrun tab, its aggregate vocabulary UI-local text
  (`results/shape/compose.rs`), never an engine type. The strip is still not the place.
- **A chart config is intent; resolving it against the result is a read-time fallback, never a
  write.** Unset channels take the schema's defaults and a reference this result cannot answer
  falls back at read time (X is a three-state `ChartX`: "not chosen" and "the row index" are
  different answers; the default mark reads the *charted* axis, not the column list). `resolve` →
  `encode` is the one construction site; the per-mark option sets make an invalid encoding
  unreachable rather than reported.
- **The chart's sort is a view transform over the settled data, and its comparison is total in
  both directions.** Never in `ChartQuery`, so flipping it repaints rather than re-reads; the
  comparator takes a direction flag, because reversing it moves the gaps to the head of the chart.
- **A chart's controls are repaints, and the bin count is the one exception — because the engine
  does the counting.** `bins` reaches `ChartQuery` and is clamped to the *shared* `MAX_BINS` at
  both ends of the wire; the box bounds its input and re-echoes on blur, and parses wide before
  it clamps. An empty box is the engine's `√n`. `hidden`, `log_y` and `sort` never reach the read.
- **A hidden series keeps its slot, and the order is sorted-then-hidden.** Blank the values, never
  drop the series, or a legend press recolours the chart; sorting after hiding would reshuffle a
  `ByYDesc` axis. Keyed by name, dropped by `resolve` for a mark that cannot un-hide, ⌥-press
  edits the set rather than rebuilding it — and the legend survives the all-hidden notice, which
  names it, but no other, which would key colours for a plot that is not drawn.
- **A log axis never refuses; it says which of two reasons sent it back to linear.** One
  `ValueCoord` with two arms, offered only where a mark plots position rather than extent,
  decade-rounded with the next decade out when a bound sits on one. A histogram's empty bins are
  not a blocking zero; an overflowed **ratio** is a hang, because that is what plotters iterates;
  and nothing positive at all is no banner, because there is no chart under it to explain.
- **The crosshair rules through the hovered mark, and its pieces are absolute siblings of the
  plot.** Freya repaints every node every frame and a canvas re-runs `on_render` each pass, so a
  pointer-tracked crosshair replots the whole chart per mouse sample; riding on `hover` is free.
  The value is carried on the `Hit`, never inverted back out of the pixel row, and the plot
  frame comes back in `draw`'s own answer rather than a second slot.
- **A snapshot read has no order of its own; order is the ordinal column.** Reads that need order
  `ORDER BY __strata_ord` (unsorted reads entire, user sorts as the tie-break) and every reader
  projects it away — export must never write it. Measured: above 10 MB a bare `LIMIT/OFFSET` read
  is nondeterministic (`SNAPSHOT_SPEC.md` §9).

**Data, values, rendering cost**

- **JSON is read by our own `FileFormat`** (`engine::json_poly`), and a replaced reader inherits the
  replaced reader's **diagnostics**. A `FileSource` must handle its own projection.
- **A view of a value is bounded where the value is *encoded*, never afterwards — and it expands
  breadth-first.** Collapse the value, not its parent; fixed depth with the budget as a backstop;
  an empty container is its own summary. A `render` body never serializes a whole value — it reads
  a **synchronous** cache, not a `use_memo`.
- **An inspector reads the Arrow arrays; only a *document excerpt* goes through text.** Address by
  entry index, resolve with O(1) slices, clip **before** materialization.
- **A recursive `Debug` is not a cheap way to get a type's name.**
- **A virtualized list scrolls its cross axis already; a row that `fill`s is what stops it.** Verify
  a layout question with a `torin/tests` case before building on the answer.

**Diagnostics, logs, problems**

- **Diagnostics are a reconciliation, not an event.** Pure function of buffer revision + catalog
  epoch, stamped per tab; `stale_tabs` is the whole work list and one driver drains it. Never a
  second producer, never enumerate entry points. The catalog is a **gate** as well as an input.
- **A log is recorded by its observer; there is no producer to register with.** Whichever layer
  watched the fact calls `log_event`. Never re-derive an event, never let a log entry be the only
  copy of a live fact. A cancel is logged at the cancel; an entry carries a level, not an origin.
- **Problems holds *conditions*, at two scopes; a run failure is the results pane's.** The test is
  "is it true now, and does it retract itself" — reconciliation, remembered condition, or event.
  The rail badge must total **every** scope from the same functions the scopes use.

**Agent access**

- **An agent's tools are the app's own semantics, and the gate in front of them runs before
  dispatch.** `run` asks `Engine::policy_verdicts` and fails closed, never rewrites SQL, reports a
  stop as a status, and asks `Engine::snapshot_live` rather than reading prose. `read_page` does
  **not** pin.
- **The vocabulary is public methods and `#[tool]` is a wrapper over them; the model-facing
  manifest is derived from the router that serves MCP.** A wrapper only resolves `Caller` and
  holds `Busy`, then delegates to the method (`_as` for a session-scoped one); the in-process
  caller is `Caller::Owned` by construction. `manifest()` reads `tool_router().list_all()` —
  never a hand-kept list, never an in-process shim that re-implements a body.
- **An agent drives the app through the app's own funnels, and works in a surface of its own; only
  a *gate* may be skipped, and only when the gate is a question for the user** (the T2 confirm). The
  run is dispatched straight at the engine on the query session's own `WsId`, bracketed by the
  window (ownership check + record, then the outcome); registration is per **mount**, keyed by a
  minted id; a settle **names its run** by a sequence number the dispatch minted, never "the
  newest"; and the channels are **two**, because a connection ending is sent from a `Drop`.
- **An agent that is not *in* the window does not write the window's state; it gets a surface of its
  own — and the scoping is a type, not a check.** `StrataTools` *is* one agent, minted per client
  connection and retracted on drop, so a handle it does not own answers exactly as one that never
  existed. A surface's state belongs to whoever is looking at it: "shared, last-writer-wins" is a
  fine rule for *content* and a bad one for *attention*. Promotion is a press, into a **new** tab.
- **An agent's identity comes from the request, and a teardown that cannot happen yet is owed to
  whoever finishes last.** `Caller` mirrors rmcp's own lifecycle predicate — never the value's
  lifetime, never `Mcp-Session-Id` (not the discriminator, and absent on the branch that
  breaks), never `peer_info` (`Implementation::default()` reads `rmcp`), and there is no
  `legacy_session_mode` stopgap. A blank stateless identity is **refused** the session-scoped
  tools, never pooled. The idle sweep skips a busy agent and runs once more from
  `AgentServer::drop`. A close racing a dispatch is a **tombstone** — but it still aborts the
  engine immediately; only the *row* waits for the last settle. `is_running` is *any* run in
  flight, and the pane reads it rather than restating it.
- **Poll only what nothing on our side can observe, and name the reason where the poll is.**
  `try_read` never a wait; the timer exists only while the feature is on; staleness bounded and stated.
- **A second deployment of the vocabulary answers the same questions from what it already has,
  and owns nothing of the app's.** The headless host's catalog **is** the registration pass's
  outcomes, its one project is not looked up, it reads no app config and scaffolds nothing — and
  its CLI branch is taken before anything app-global, with logging on **stderr** because stdout
  is the transport's.
- **The assistant's brain is one table and a per-send value; whether a knob exists is ours,
  what a rung means is the provider's.** `PROVIDERS` is the only place a kind's label, URL/key
  policy, effort rule and adapter are written; `Selection` is handed in with every send, so
  several panes on several providers is several values and not a mode. **Effort is offered per
  *model*** (`Efforts::{Never, Always, Only}`), because reasoning is a model capability and a
  per-kind answer both hides working controls and offers broken ones; `Only` is **default-closed**,
  so falling behind what a provider ships costs a knob rather than a refused request. A rung the
  model does not offer is a **refusal**, never a silent drop.
- **A model is picked from what its provider serves, and the list it is picked from is a
  satellite refreshed where it is shown.** No free-text model box anywhere; the offer is
  `Listings::offer` — reported **∪ {the current pick}**, unfiltered — so an endpoint with no
  `/models` cannot strand a working setup and no static name list can hide a new model. The
  cache is `strata_core::models` beside the config, never a config field (a fetched list is not
  something the user edited), loaded once at startup with **no dial-out there**; a stale list
  renders immediately and one background refresh per shown provider replaces it. Names to the
  satellite, outcome to the window's `Probe` — one request, two keeps, never two caches. A
  changed URL or key drops both on the same line (`SettingsCtx::forget_provider`).
- **A cancelled turn is a drop, and the conversation it leaves must still be sendable.**
  Dropping the tool future *is* the engine's abort; the outstanding tool calls are answered
  before the turn settles, or the next send is a request every provider rejects. Cancelled is
  never failed.
- **A statement the user can run is a tool call, not a formatting convention — and the check in
  front of it is the *editor's* policy.** `offer_sql` is the assistant's own tool, never on the
  router; it validates before the card exists, which is what lets it hand over a write the
  assistant is itself refused. Explanatory SQL stays an ordinary code block.
- **A window holds conversations, the pick is per conversation, and a step card is a citation.**
  `state::chat`'s `Chats` is capped, seeded through `seed_pick` (a provider that is no longer
  enabled is not a pick); a turn's blocks stay in **arrival order**, every figure on a step card is
  the engine's own, and an `offer_sql` card is executable *instead of* a step card, never beside
  one. Promotion is `actions::open_sql` — never a write to the user's buffer.
- **A conversation survives its window, and what has to survive is both lists — the turns the pane
  paints *and* the `Conversation` the model reads back.** `.strata/chats/<uuid>.json`, one document
  per conversation, gitignored; the seam is `Conversation::{to_json, from_json}`, JSON-valued so
  `genai` stops at `strata-agent`'s edge. Written after the turn's settle, at the stop press, and
  at teardown (synchronously, `use_autosave`'s shape), dirty ones only — and a **pick** is dirtying,
  through the one `Chats::repick` funnel. A task writing this subtree's state after an await must
  be cancellable by it: the presses use scope-bound `spawn`, and the turn task holds `Chat::running`
  until its record is on disk (`Chats::finish`). Reopening is a **read** — no run, no scan, no
  network, one `validate` per offer card — and a stale card degrades **silently** to a code block,
  a mark that is never stored. Eviction demotes to the shelf and *answers* what it shed; retention
  is `Ai::max_chats`, rotated on load; Clear and the per-row delete ask through one confirm at the
  **window root**.
- **A turn is cancelled by dropping its task, and a dropped run still settles.** The task owns
  AS-02's `Running`, whose drop guard is the cancel and the engine's abort; the reply keeps what
  streamed, marked stopped. One layer down, `SettleOnDrop` sends the stop settle in the engine's
  own `CANCELLED` wording, so no satellite is left holding a `Running` row.
- **The in-app assistant is held like any other agent and told apart only where the user is owed
  a different sentence — and the mark is minted, never claimed.** `StrataTools::in_app` sets
  `Agent::in_app`, which rides the call that opens a session to every `Host`; `held` is the
  unfiltered view `list_query_sessions` and the log's attribution read, and `sessions_of` is the
  one place the line is drawn — for the close confirm, which has to name the assistant as itself.
  Keying on the identity would let any MCP client claim its way across that line. **No surface
  lists agents**: the Agents pane and the header's status dot were removed, so the MCP server is
  present and unshown, and a server that cannot bind reports only through tracing. The satellite
  therefore holds **only what the bookkeeping reads** — a run is a `seq` and an outcome, and the
  SQL travels to the engine and not through `AgentAsk::RunStarting`.

**Stores and state**

- **The catalog is the `ProjectState` store, not a query.** Never build a `FetchCatalog` capability.
- **Def/runtime split.** Pure serde defs in `strata-model`; `Reg<T>` rows in the store. Tables/views
  keyed by **name**, saved queries by **`Uuid`**, connections by **`url()`** (scheme *and*
  authority — never the bucket, which two providers can share) — and a connection's `Reg<()>` is
  honest, because connecting *asks the bucket* rather than only building a store.
- **A connection registers a bucket, and it registers before anything that reads one.** Connections
  are `register_pass`'s first phase; a whole-catalog ↻ re-connects and a single table's Refresh
  does not. The def stores the authority and derives the scheme from the provider; **Ambient and
  Named profile are two providers**, because naming a profile on `aws-config`'s default chain
  leaves `Environment` in front of it; and **no arm of `engine::store` takes a secret** — a profile
  name and a key file path, never a key. Identity is the **URL** and the sort is the **address**,
  so `upsert_connection` replaces on one and inserts by the other; an edit that moves either half
  **deregisters the old URL itself**, and the editor's Save asks for a whole-catalog pass.
- **Connecting asks the bucket, because a description can be well-formed and wrong.** `connect` is
  `prepare` (naming rules, client options, registry key, built store) then `reachable` — one
  page off `list(None)`, so it costs one request however large the lake — never
  `list_with_delimiter`, which drains the whole paginated stream to build its `ListResult`.
  This **overturns** the earlier rule that connecting learns nothing: that traded a round trip per
  connection per project open for a status dot that meant "a struct was built", and a mistyped
  region registered green while every table under it failed on `object_store`'s bare-redirect
  message. A wrong region is refused **by name**, naming the region. It asks whether the connection
  is **described** right, never whether it may do everything: `Generic` (bare redirect) and
  `NotFound` refuse, `PermissionDenied`/`Unauthenticated` **register** — a prefix-scoped
  `s3:ListBucket` and a `GetObject`-only public bucket both 403 at the root while working
  perfectly, and `connect` is the pass's first phase so there is no table prefix to probe with.
  Rejected credentials therefore still fail at the first table, exactly as before the probe.
  **HTTP is exempt** — its store
  lists over WebDAV `PROPFIND`, which most file origins do not implement, so probing one would
  refuse working connections. The connection editor needs **nothing of its own**: Save writes the
  def, asks for the pass, and watches its row, which is where this refusal lands. A Save-time
  `check_connection` was built and withdrawn — redundant with the pass, and it put a ten-retry
  network call behind a button three interaction tests press (7s → 308s).
  `connect` therefore does network I/O: a test about *keying* goes through `store::settle`, not
  through `connect`, or the suite dials out to buckets nobody owns.
- **A table reads through a connection by naming it, and the composition happens once, in
  `resolve_source`.** `TableDef::connection` is the connection's `url()` and the only thing that
  says a table is remote; its sources are bucket-relative exactly then, never relativized, and
  `resolve_source` takes the connection so the local rule cannot be reached for by mistake. The
  LOCATION toggle is an explicit choice, never a scheme parsed out of a path, and a forget's
  confirm names the tables over the bucket and the views behind them.
- **A connection's address is its provider's own, and every rule about it lives in one place.**
  `address`, not `bucket`: S3 and GCS name a bucket and take the provider's scheme, HTTP names a
  **whole origin URL** and a path is refused rather than trimmed — as is **userinfo**, because a
  `https://user:pass@host` pasted into that box would put a password in the committed
  `project.json`. `Provider::check_address` is the
  one copy of the two providers' (different) naming rules, called by the store *and* the editor;
  `client_config` is `object_store`'s `ClientConfigKey` map, on the def because one HTTP client
  serves all three, offered from `CLIENT_KEYS` and refused by `check_client_config`. `allow_http`
  is never offered — S3's endpoint toggle, and on HTTP derived from the typed scheme.
- **History is a satellite** (`.strata/history.jsonl`), never a store field. Only successful runs —
  rows *or* an intercepted statement; Clear unwrites the file and keeps the `seen` guard.
- **History is a list of queries, not of presses — and dedupe comes before the cap**, keyed by the
  same `util::collapse_sql` that renders the preview.
- **Silent corruption is refused, never warned about — and the refusal is checked against read data,
  not declared metadata** (the Hive NULL-partition gate reads the footer, proceeds only on exact zero).
- **A secret Strata must keep lives in the OS keystore, and config holds a reference to it — which
  is a property of the types, not a rule to remember.** `strata_core::secret`: `SecretRef` is a
  minted id that rides `settings_merge!`; `Secret` derives no `Serialize`, has no `Display`, and
  redacts its `Debug`. Empty is not a secret, absence is not an error, `open_keystore` runs once in
  `main`, and `APP_ID` is the bundle id `bundle-macos.sh` reads. Never a plaintext fallback.
  In memory it is **zeroed, not guarded** — mlock/mprotect would cover one link of six and read as
  stronger than it is; exposure is managed by lifetime (read per use, never cache).
- **One app-global config store.** Disk is a startup input read **once** — no file watching, ever.
  `write_config` is the sole write path. Settings is a **channel**, not its own global.
- **The config file is read three ways and written atomically, and a file this session could not
  read is never written over.** Absent is a first launch, unparseable is kept aside as
  `.corrupt` and then replaced, unreadable latches writing off for the process. Never
  `unwrap_or_default()`: a write follows within seconds of launch, so one conflated failure
  persists the defaults over every setting the user has.
- **A draft of shared state commits a per-field diff against its seed, never the whole struct**
  (`Settings::merge_onto`, exhaustive via `settings_merge!`). "Anything to apply?" is `draft != seed`.
- **The theme is pure derived state — deliberately not stored.** Copy `theme.peek().name` out before
  `theme.set(...)`.
- **An uncommitted value that must be live everywhere is a second *input* to the derivation, never a
  stored result.** Keep the slot narrow; dropping it is the revert.
- **A theme is roles; a component's colour is a role reference in one static table.** The file
  authors the closed role vocabulary + syntax + fonts + typography and nothing per-component;
  an over-shared role is split, never worked around with a literal.
- **Panel layout lives on `SessionState`** — `Chan::Layout` (structure) + `Chan::LayoutSize` (sizes,
  unsubscribed). Keep panels keyed with fixed `.order()`.
- **Each edge of the shell offers one pane at a time, and a rail is what picks it.** The right
  side is `Layout::right: Option<RightPane>` — inspector *or* chat, never both — exactly as the
  left is `Option<SidebarPane>`. A rail toggle collapses the lit pane; a surface that *names* one
  opens it (`open_right_pane`, never `toggle_right_pane`).

**Windows and lifetimes**

- **A window's project subtree is keyed on the project folder; there is no reopen-in-place path.**
  Never re-point a live store at another project. Anything surviving a re-root lives on the *window*.
- **A window that belongs *to* another window is a child window, and its lifetime is ours.** It must
  not count as a workspace window, and closing-with-the-owner goes through Freya's own path.
- **A window's lifetime must be at least as short as the shortest-lived thing it holds — and for a
  child window that is a *mount* of the project subtree, not a window id.** Take a
  `platform::owner::Subtree` and call `use_owner_pin`; never grow a third copy of the rule.
- **Which window an open lands in is one decision in one place** (`platform::open::decide`, pure).
  Own project = no-op; already-windowed = focus. Both outrank the preference.
- **Every path that destroys a window's work asks on the same terms** — one `CloseTarget`, one
  dialog. The predicate is the engine's own answer, never derived from mounted UI. A question
  already answered is not re-asked (`use_engineless_close`).
- **Nothing blocking runs on the render thread, and a read the user has to wait for is an *arm*, not
  a freeze.** `task::offload`, a thread per call. Cancelling is dropping the answer, never stopping
  the work. A value needed before a window exists gets a deadline, and its consumer must be safe
  against the empty answer.
- **Window geometry** is `Platform::root_size` + `Platform::window_position`, both logical. Restore
  only at window **creation**; there is no runtime resize/move from the app.

**Settings, keymap, input**

- **A setting the user edits through more than one gesture gets one funnel, and the policy lives
  next to the resolution it has to agree with** (`keymap::propose` → `apply`, in strata-core). A
  reset is a proposal; a steal is expressed as the bindings it changes. An override is only "custom"
  if it takes effect.
- **An app-global surface that follows the focused window is pointed by *every* window, and the
  obligation rides the call each window already has to make.** `use_file_menu` lives inside
  `use_register_window`, which takes a `MenuScope`; scope and chord are one enabled state, applied
  together.
- **A menubar accelerator is state, not decoration — and it must be disarmed while a chord is being
  captured.** `sync_chords` off a destructured `MenuChords`; `suspend_accelerators` for the capture.
- **An app-wide flag held to protect one window's listener is released on losing focus, not only on
  finishing.** When a flag's scope is wider than the state justifying it, its condition must include
  whatever makes that state reachable.
- **A name two surfaces have to agree on is generated from one table, not typed twice — and
  navigating to something is never editing it.** The category is never restated; the engine's
  properties are indexed off `ENGINE_KEYS` entire.
- **A free-form list setting is edited as rows and committed as a map.** Ids from a counter, never
  the name. The list lives on `SettingsCtx`, not the pane.
- **No command bus.** Distributed `on_global_key_down` per feature (`keymap::on_command`),
  precedence = document order, a modal barrier = an early-mounted consuming listener. Never a
  root-level handler registry — fix the fork limitation instead (§6).
- **The command palette is a *registry of offers*, not a dispatch layer — and it is not a function
  of the keymap.** Every command's body is one call into a funnel that already exists; where that
  logic was inline, it **moves** to the funnel rather than being copied. `CommandRoute::key` renders
  the hint and nothing else. Adding a command is one method.

## 3. Freya component & UI conventions

Full text: [docs/reference/FREYA_UI.md](docs/reference/FREYA_UI.md).

- **Reusable UI is a `Component`** — `struct` + `#[derive(PartialEq)]` + `fn render(&self)`. `mod.rs`
  builds children by **struct literal**.
- **Builder pattern**: chain; never store an element in a variable to mutate later. `.maybe()`,
  `.map()`, `.maybe_child()`.
- **Standard components first.** `Button::new().flat()`, `Select`, `Input`, `Table` — never
  hand-rolled lookalikes. Icon-button clusters are **28×28**. A missing component *state* belongs on
  the component's theme **in the fork**, never as a token on the consuming surface. But the test is
  whether the gap is in the *component*: what a table has no opinion about stays composed in the app.
  Don't restate at a call site what a variant already resolves.
- **Every padding, gap and corner radius comes off the scale in `components::metrics`, and the
  three exceptions say so at the site.** `SP_1`…`SP_9` / `R_XS`…`R_4`, the design's own token
  names — constants, never theme fields. Name a step locally (`const CELL_INSET: f32 = SP_4;`),
  never restate its number. Exceptions: `pill(extent)`, `HAIRLINE`, alignment nudges. The same
  module holds the fixed sizes more than one surface agrees on; rehoming never renumbers.
- **A surface with its own component theme reads colours from that theme, not also from the
  roles.** `use_roles()` only where no component theme covers the surface; the semantic tones
  (success/warning/error/info) only through the shared `tones()` hook.
- **A shared theme's fields are named for the role they play, not for whoever needed one first, and
  a component's own dress never becomes one.**
- **Fonts are never hardcoded.** Typography role components; `InputTypography` around `Input`s.
  Mixed-style inline text is a `paragraph()` of spans. Theme-consuming hooks run a **fixed** number
  of times per render.
- **Event props follow `Button`'s shape**: `Option<EventHandler<Event<T>>>`. `Callback<A, R>` only
  for value-returning callbacks.
- **One handler per underlying event name.** A second registration silently **replaces** the first,
  and the sugar family shares names with the primitives (`on_secondary_down` → `on_pointer_down`).
  Check `freya-core/src/elements/extensions.rs` before adding any `on_*`; branch inside one handler.
- **A panel has no usability floor, only a stub floor — and a chrome row folds rather than
  spilling.** RustRover's model, because the canvas declares `min-width: 1180px` and has no narrow
  states. Space is given up in a stated order (proportional pane first and entirely, then pixel
  panels equally); pressure never collapses a panel, only a drag does. One fold policy for every
  row (`components::toolbar`), arithmetic over the item list, each item declared once. `Overflow`
  has no `Scroll` and defaults to painting *outside* the box, so `SpaceBetween` over
  `Content::Normal` overlaps — use `Content::Flex` + a flexing, ellipsizing leading run.
- **A border is painted, never laid out — a bordered box whose children have backgrounds needs
  padding equal to the stroke.** Not CSS's border box.
- **A size lands on the node the parent lays out** — a component that wraps its control sizes the
  **wrapper**. Tell: a fixed width works and a relative one doesn't.
- **`Size::flex` is only divided by a parent whose `content` is `Flex`.** Check this first when a
  "push to the right" spacer misbehaves.
- **A focused `Input` owns the keyboard, so a surface built around one handles its keys in
  `on_pre_key_down`** — and that is what makes it a real modal barrier. Resolve chords through
  `keymap::resolve`. Keep a `GlobalKeyDown` barrier too, on a **different node**.
- **A completion is an edit at the caret: replace the token's span, then put the caret after what
  was inserted.** Both surfaces, one rule — the editor's `replace_range` and the composer's
  `@`-mentions over `Input::caret`. Never a policy about where the caret goes on an external
  write; convert bytes ↔ UTF-16 once, at the `Input` boundary.
- **A disabled control gates its handlers; it does not go `interactive(false)`** (which suppresses
  `pointer_leave` and strands a hover).
- **A built-in control's press reaches its ancestors, so never wrap one in a pressable parent** —
  make the label a *sibling*.
- **A settings-style surface is built from `components::form`, never from its own rows.**
  `Form` > `Row` > control, with the register a `Variant` on the form. Where canvases genuinely
  differ, name the difference in `form/mod.rs`'s "known divergences" rather than averaging it.
  A row can be **addressed** (`Row::anchor` + `form::reveal`).
- **A field backing a draft publishes on every keystroke, and normalizes its box when it is left.**
  The change comparison belongs in **state**, never captured (`use_side_effect` builds its closure
  once; use `use_reactive`).
- **Pointer events carry NO modifiers.** Track modifiers via global key handlers; reset defensively.
- **`stop_propagation` vs `prevent_default`**: `prevent_default` in `on_pointer_down` suppresses the
  follow-up `on_press` — do double-click detection inside that same handler.
- **`VirtualScrollView` memoizes its builder closure**, so captured snapshots go stale. Each child
  reads shared state reactively.
- **A root-scoped task outlives the project subtree, so it asks before it writes one**
  (`State::is_alive` / `RadioStation::is_alive`, both fork additions). Cancelling on unmount is
  the other answer and the right one for work that should stop; a drop that is deleting data has
  to finish, and only its *reporting* is skipped.
- **A task spawned from a handler belongs to the scope that pressed it, so a press that unmounts
  its own control cancels its own work** — silently, before the first poll. A menu item that closes
  its menu, a dialog button that clears its own slot, a Stop that flips back to Send: all three
  shipped broken. `spawn_forever` is not the escape (root-scoped, it writes subtree `State` after
  an await and panics on a freed box). The press records the intent in a `State`; a
  `use_side_effect` in a scope that outlives the control performs it — with the intent **in** that
  state, never captured, because the effect's closure is built once.
- **Two siblings on the same layer have no paint order — set a layer.** A layer's nodes are an
  unordered set, so "declared second" is not "painted second"; the covered element reads as
  though it had alpha. `Layer::Relative(1)` for a sibling, `Overlay` only to clear the window.
- **A `canvas` paints from a slot, and repaints only when asked.** `RenderCallback`'s `PartialEq`
  is always true, so the tree keeps the first render's closure — put the frame in a `State` the
  callback peeks, and request a redraw from the effect that fills it.
- **Reactivity**: `state()`/`.read()` subscribe; `.peek()` does not; `.set()`/`.write()` need `let mut`.
- **Logical units everywhere.** Never multiply/divide by the scale factor in component code.
- **Naming**: plain nouns for structs, no role suffixes; DI handles end in `Ctx`.
- **User-facing text reads like a standard IDE** — terse plain sentences, single-quoted identifiers,
  no em-dashes/backticks/ellipsis/glyphs, no conversational hedges. Merge near-duplicates.

## 4. State: where things live

The decision procedure (full design: `docs/FREYA_STATE_ARCHITECTURE.md`, notes in
[docs/reference/FREYA_UI.md](docs/reference/FREYA_UI.md)):

- **State owned by one tab** → a field on `QueryTab`, under its **own granular `Chan` variant per
  concern**. Channel granularity is the leak-prevention mechanism (`request` on `Chan::Request(id)`,
  so keystrokes never wake the results pane).
- **Shared reactive state with a small, known, shallow consumer set** → **struct-field props**
  (`State<T>` is `Copy` + `PartialEq`).
- **Context** is reserved for DI handles (`EngineCtx`, theme) and deep/open-ended trees (`Selection`).
- **A second surface that needs a settled query's outcome subscribes the query again** — same
  capability, same keys, same `stale_time`. Never a mirror on a store. Caveat: `enabled` is part of
  cache identity, so there is no "watch without running".
- **Never a shared map/registry value** threading every tab's data through one value — that's the
  rejected runs-by-id store in disguise.
- **Inside the fork**, `thread_local!` for shared component state is an antipattern; use the
  lazily-initialized root-context pattern or `State::create_global`.

## 5. Cross-task ownership

Cross-cutting capabilities (clipboard/copy, export, keyboard routing…) get **one** shared
implementation owned by their backlog task in `.claude/tasks/`. When your feature touches a
capability another task owns:

- Ship the UI affordance **inert** — no handler behind it — and add a "wire into X" note to
  **both** task files. Whether it also *looks* unavailable is a design call, not a rule: a menu item
  is **parked** (`MenuButton::enabled(false)`) because a menu is a list of things
  you can do right now, while a surface's **primary call to action keeps its full dress** (the
  inspector's scan card) because greying it out misrepresents the canvas the surface is built to.
  Nothing is parked today — the helper that spelled it went with the last task that needed one,
  which is the second bullet applied to this bullet's own machinery.
  Either way the capability arrives with the task that owns it, and nothing at the call site changes
  but the handler.
- Do **not** build the shared mechanism early, do not fold a local one-off, and leave **no
  unreferenced pre-work**. Record the intended shape in the owning task's file instead.

## 6. The Freya fork: when and how to change it

Full text — including the recovery for an unpushed gitlink:
[docs/reference/WORKFLOW.md](docs/reference/WORKFLOW.md).

- **Fix limitations in the fork, not around it.** When a design starts reaching for a workaround
  (a registry, a scale-factor correction, a duplicated token), the right move is usually a semantic
  fix in the fork. Platform-specific half in its own `freya-winit` module (`cfg`-gated, documented
  no-op elsewhere), primitive on `RendererContext`, discoverable API on `WinitPlatformExt` — so app
  code never touches objc2 or a raw winit handle.
- **Follow the fork's own `AGENTS.md`**; keep changes upstream-shaped (themed tokens, doc comments,
  examples).
- **After changing the fork, push it** — the committed gitlink must exist on the fork remote or
  fresh clones and CI can't init the submodule. This has broken a worktree outright before.
- **Worktree traps — use the `freya-submodule` skill.** `git worktree add` does not update
  submodules. Every worktree has its **own** `crates/freya`: when editing fork files by absolute
  path, confirm the path goes through *your* worktree.

## 7. Git, worktrees, and verification

Full text: [docs/reference/WORKFLOW.md](docs/reference/WORKFLOW.md).

- **Formatting is the `fmt` skill, never `cargo fmt --all`** — `--all` includes local path deps, so
  it reformats the fork (measured once: 344 files, 4006 deletions, none intended, and invisible in
  `git submodule status`).
- **Build + `schema_in_sync` is the check.** After any theme change:
  `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`.
- **Clippy is part of that check, and a lint wrong for this codebase is allowed once at the
  workspace rather than at every site it fires.** `cargo clippy --workspace --all-targets --locked
  -- -D warnings`; the curated set is `[workspace.lints]` in the root `Cargo.toml` (the base plus a
  hand-picked readability/complexity list, deliberately not the whole pedantic group), thresholds
  and knobs are `clippy.toml`. Every member inherits it — the vendored editor too. Reach for a
  threshold before an allow and an allow before an `#[allow]`: an inline suppression is for a fact
  about **one** site, and it carries the reason it is true there.
- **`cargo test` needs a container runtime**, because the connections integration test drives a
  real MinIO and is deliberately not `#[ignore]`d — an ignored test is one nobody runs. Point
  `DOCKER_HOST` at it if it is not on the default socket; CI gets one from
  `atomicjar/testcontainers-cloud-setup-action`.
- **Read cargo's own exit status, never a pipe's.** `cargo test … | tail -20` and
  `cargo test … | rg 'test result'` both report the *last stage's* status, so a run that failed to
  compile reads as a pass and a filter that misses the failure line reads as a clean suite. Both
  happened during the pre-release review, one of them twice. Redirect to a file and check `$?`, or
  read the `test result:` lines themselves.
- **A change you wrote is reviewed by critics who cannot see why you wrote it** — the
  `adversarial-review` skill: isolated read-only lenses handed artifacts and the contract but never
  the intent, then a refutation gate that defaults to killing a finding. In front of the build
  check, never in place of it. Each lens must name its strongest candidate; a `CLEAN` verdict after
  the gate is still a result.
- **Effort is the user's dial and the panel is not on it.** `low|medium|high|max` buys reasoning
  effort and panel width together (1 voter, then a 3-voter majority, then `max`'s red-team); its
  floor is one voter, never zero, and isolation and whole-file reading are fixed at every tier. A
  **workflow**, because only `Workflow`'s `agent()` takes a per-call `effort`. The verdict is
  computed in the script from the tally, and the tier is reported verbatim.
- **A voter reads a batch of candidates, and dedup comes before the panel.** `voters ×
  ceil(sites/10)`, never `voters × sites`; per-candidate voting billed 165 agents on a 7-file diff
  where the batched, deduplicated shape bills 18 (6 critics + 3 x ceil(32/10)). Convergence is the promotion signal — count it
  once, do not pay for it six times. Cap a lens at 12 candidates and log the drop.
- **The merge keys on position *and* claim, and promotion runs before the red team.** Two lenses
  citing one line is routine, not agreement: merging on `file:line` alone deletes one claim unjudged
  and promotes the survivor for a convergence that never happened. Cluster by content-word overlap,
  biased to **under-merge** — a missed merge costs a panel slot, a wrong one destroys a finding.
  Promote first so `max`'s severity correction is the last word.
- **Discovery fails closed, not just the panel.** A critic returning `findings: []` is a clean
  result; a critic returning *nothing* is an absence of evidence, and collapsing the two lets a
  review where every critic died report `CLEAN` — the worst thing the tool could say. All critics
  dead is `FAILED`, never an empty findings card.
- **Scope is four disjoint readings, and a description is a claim.** An uncommitted change sits in one of four
  disjoint states and each git command sees exactly one: committed (`git diff
  "${CLAUDE_CODE_BASE_REF:-origin/HEAD}...HEAD"`), staged (`git diff --cached`), unstaged (`git
  diff`), untracked (`git ls-files --others --exclude-standard`). Miss one and that state reviews as
  empty and returns `CLEAN` over unreviewed code. `git status --porcelain` is the inventory only —
  no content, and it abbreviates directories. Untracked files have no hunks, so mark them
  whole-file. Run the commands one per line, never chained with `&&`: a short-circuit swallows every
  state after the failure, and `origin/HEAD` exits 128 wherever `git remote set-head` never ran. A
  non-zero exit means that state is **unread, not empty** — the two print the same nothing and only
  one is safe to call clean. Never edit the command by substitution; check any replacement against
  all four states.
  A PR is `gh pr view` + `gh pr diff`, and its description goes in the **contract** as a claim to
  audit, never in the scope as context to believe.
- **A stage that cannot verify fails closed; a stage that only corrects keeps and marks.** The panel
  drops a finding it could not verify — reporting an unverified one is the worse error. The red team
  only ever lowers a severity or removes, so a missing verdict there keeps the panel-confirmed
  finding, marks it `redTeamed: false`, names the batch that never answered, and reports
  `adversarialPhase: 'partial'` with the uncovered count. Never let a phase claim coverage it did
  not deliver.
- **Findings go through `ReportFindings`, and the script hands over the exact shape.** `report` is
  returned ready to pass, sorted most-severe first; each row carries `CONFIRMED` (unanimous panel)
  or `PLAUSIBLE` (one voter refused). The severity tally and the `BLOCK`/`CONCERNS`/`CLEAN` gate
  go in prose beneath the card, which has no field for either. Never print the list twice.
- **CI runs that same check on every PR** — `cargo clippy --workspace --all-targets --locked --
  -D warnings` then `cargo test --workspace --locked`, on **macOS**, with `submodules: true`,
  asserting the gitlink **before** compiling. `-D warnings` is scoped to the clippy invocation;
  the toolchain step's `rustflags: ''` stays, so a dependency's warning cannot fail the build.
- **Only the tests that need the container runtime queue for it, and the split is a test target.**
  Two jobs: `minio` runs `--test object_store_minio` entire (so a test added to that file needs no
  workflow edit) and carries the queue, the cloud agent and the release step; `test` is the same
  `--workspace` run with those tests `--skip`ped, and queues behind nothing. Never split by taste
  or by package. The lists cannot drift silently: `test` has no runtime, so a renamed or added
  minio test runs *there* and fails loud.
- **The container runtime is a single shared worker, so the job that uses it serializes repo-wide —
  and it queues rather than cancels.** A job-level, constant-named concurrency group with
  `queue: max`; the per-ref workflow group keeps the superseding. Never `queue: single` here — a
  silently cancelled run on main is no coverage of main.
- **A cloud session outlives the job that opened it, so the job releases it — and the test still
  waits out a handover it cannot watch.** `action: terminate` with `if: always()` (a *cancelled*
  job is the worst case), plus a bounded retry in `object_store_minio.rs` on the capacity refusal
  **only**. Serialization alone was shipped once and was not enough. Anything else still panics.
- **The release path is a script CI calls, never a pipeline written in YAML.** Signing degrades
  honestly and says which rung it took; the tag is created **after** the build.
- **The version lives in one file and is reached through one script** (`scripts/version.sh`, which
  writes as well as reads). A bump is refused without the release box; the commit is pushed after
  the build and never rebased.
- **The app bundle is self-contained**, and that is a claim each new asset has to keep — naming a
  new font family or weight in a theme means embedding it in the same change.
- **One Strata window across every session — enforced** by `.claude/hooks/block-second-strata.sh`.
  A refusal, not a kill — and it matches the built binary and a bundled `Strata.app` as well as
  `cargo run`, because those open the same window the rule exists to prevent.
- **No destructive git — enforced, not merely agreed.** `git checkout`/`restore`/`reset`/`clean` are
  blocked by a `PreToolUse` hook that reads the whole command string **normalized** — quotes
  stripped, line continuations folded — so chaining behind `&&`, `;` or `$(…)` does not get past
  it, and neither does a quoted verb or a backslash-newline between the two words. It over-matches
  by design (a sentence *about* one of these verbs is refused too, this bullet included); a
  rephrase is the cheap error and a miss is the expensive one. Both hooks **fail closed** without
  `jq`, since a hook that cannot read the payload must not answer "allow". Use `git switch`, `git stash`, `git diff`, or ask. Any other
  delete/overwrite of work you didn't just create: standalone, explicitly described, and not at all
  with substantial uncommitted work in the tree unless you have asked.
- **Task files are the working contract.** Keep the `.claude/tasks/` file true — corrections, wiring
  notes and ownership seams — as part of the change. The same goes for `docs/reference/`.

## 8. High-risk areas

- **The editor's hover/pointer stack** (`hover`/`update_hover`, per-line pointer handlers, the
  diagnostics squiggle popup) is delicate; generalising it has broken diagnostics before, and every
  hover-docs/signature-popup variant was reverted. If function/symbol docs come up again, extend the
  **autocomplete** surface (signatures already render as the completion row's dim detail); treat any
  change to the hover model as prototype-behind-a-flag, and re-verify diagnostics.
- **Modifier tracking** (§3) and **global listener order** (§2, no-command-bus) both depend on fork
  semantics — if either misbehaves, suspect a fork-level fix before an app-level patch.
