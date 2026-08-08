# ED-04 · Internal tables, engine half: def shape, CTAS spool, replay

**Workstream:** Editor statements · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** ED-02 (ED-03 should land before or with this)

## As built — four corrections to the plan below

1. **The inner query is never rendered back into text.** The plan said "a `COPY (<inner query
   text, sliced verbatim>) TO …` rendered internally". As built, the *parsed statement* goes to
   `SessionState::statement_to_plan` and its `CreateMemoryTable.input` becomes the input of a
   `LogicalPlan::Copy` node built directly — so the query that runs is the query the user wrote,
   by construction rather than by the fidelity of a round trip. Slicing was rejected on evidence:
   sqlparser's `Spanned` impls carry `todo` gaps and its `Location` is character-based, which is
   the same offset-arithmetic-over-judged-text that `PolicyRefusal` already refuses to do
   (`validate.rs`). It also removes work: DataFusion's own `CreateTable` arm **exhaustively**
   refuses fifty-odd unsupported clauses (`TEMPORARY`, `LOCATION`, `PARTITION BY`, …) and already
   resolves a declared column list against the query, casting and renaming to it. Ours are the two
   it plans without enforcing — constraints and column defaults — plus duplicate result column
   names, which is reachable through a join even though a duplicate *projection* is not.

2. **DataFusion 54 runs a list-files cache by default, and it had to be turned off.** 1 MiB,
   **infinite TTL**, keyed by table path (`CacheManagerConfig::default`). `CREATE OR REPLACE
   TABLE` failed outright against it — registration re-listed the directory and got the file names
   from before the rename. It is not only this task's problem: the catalog's ↻, the Configure
   window's re-inference and D5's whole "a re-scan picks up new files" promise are all "list the
   sources again", and every one of them was being served the previous answer. `catalog.rs`'s doc
   asserted the opposite ("we run no `ListFilesCache`"), which is now true again because
   `build_runtime` makes it true: `ENGINE_KEYS` names `0` as the default for
   `datafusion.runtime.list_files_cache_limit`, the builder applies it before any override, and
   `build_runtime` therefore always builds a runtime rather than short-circuiting to DataFusion's.
   The key stays user-settable — a project over a slow bucket with a fixed file set is what it is
   for. One consequence rides with it: `datafusion.runtime.list_files_cache_ttl` configures
   nothing while the limit is `0` (`CacheManager::try_new` builds no cache at all), so that key's
   description now states the dependency. Having a TTL implicitly switch the cache on was
   rejected — one key must not change another's meaning.

3. **`register_external` backstops the reserved namespace with `Blocked::ReservedName` verbatim**,
   so a hand-edited `project.json` and a typed statement are refused in the same words.

4. **A create over a `Reg::Failed` external def's name succeeds and replaces it.** The engine
   resolves the namespace against itself (`ctx.table_provider`), and a def the engine refused has
   no provider — the store's namespace is not reachable from `strata-core` and building a shadow
   copy of it would be the second catalog the invariant forbids. The end state is defensible: the
   user named a table they wanted to exist, the row visibly changes origin, and nothing on their
   disk is touched. Stated in `ddl::tables::create` where it happens.

## Goal

Internal tables exist: `CREATE TABLE` / CTAS spools to `.strata/tables/<slug>/` as Arrow IPC,
registers through the existing funnel, folds into the store as an ordinary def, and replays on
open — headless host included — with zero new replay code. `docs/STATEMENTS_SPEC.md` §6.1 + §7.

## Current state

- Table creation is hard-wired to one function: `register_external`
  (`crates/strata-core/src/engine/catalog.rs:68`) — the `SourceFormat::Arrow` arm exists.
- The def→spec projection is `table_spec` (`strata-core/src/register.rs:54`); replay is
  `register_pass`/`register_project` (both hosts).
- Verified (spec §2): DF's native CTAS is RAM-whole `MemTable` — unusable; the Arrow sink writes
  LZ4-frame IPC; `ArrowFormat::infer_stats` returns unknown.

## What to build

**Defs (`strata-model/src/catalog.rs`):** `TableDef` gains
`#[serde(default)] origin: TableOrigin { External, Internal }` — a flag, not a new type (single
namespace kept; old `project.json` loads unchanged; a def is one list entry either way).

**Engine (`strata-core`):**
- `TableSpec` gains `internal: bool`; `table_spec` maps it from `origin` (one line);
  `register_external` records internal folded names in an engine-side set — derived state rebuilt
  by every pass, answering only "may a write statement target this" (never a second catalog).
- `Engine::set_data_dir(root)`: the absolute `.strata/tables` root, set at project open by the
  app and the headless host; CTAS refuses politely when unset.
- `engine/ddl.rs::ctas`: refuse constraints/defaults/`TEMPORARY`/duplicate result columns and a
  `__snap_`-prefixed target name (`Blocked::ReservedName` — spec §4; `register_external`
  backstops the same rule at the funnel, which also covers a Configure-typed or hand-edited
  def); resolve `IF NOT EXISTS`/`OR REPLACE`/plain-exists against the namespace;
  spool via an internally rendered
  `COPY (<inner query text, sliced verbatim>) TO '<data_dir>/.tmp-<nonce>/' STORED AS ARROW`
  (streaming; the sink's count column is the report's row count); rename tmp → final (atomic);
  zero-row and column-list-only CREATE write one empty IPC file carrying the schema; then
  `register_external` → `TableMeta` → `StoreEffect::TableUpserted`.
- `StrataArrowFormat` wrapping `ArrowFormat`, overriding `infer_stats` to read exact row counts
  from IPC footers (metadata-only), used by the `SourceFormat::Arrow` arm — real
  `TableMeta.rows` for internal (and external Arrow) tables. Null counts deliberately not
  attempted.
- `tidy_strata_dir` sweeps `.strata/tables/.tmp-*`; `ensure_gitignore` adds `tables/`.

**App:**

- **No Configure on an internal table** (settled with Alex 2026-08-08, replacing this task's
  first draft, which had the window open read-only). Configure edits the sources, format and
  partition columns of a def that points at the user's files; an internal table has none of that
  to edit, ever — so the item is **absent from the row menu**, not parked. That is the catalog's
  own established treatment for an item that could never apply to a row kind rather than one that
  is merely unavailable right now: the view menu has no Refresh at all, and
  `each_row_kind_offers_its_own_menu` states the reason in as many words ("a view has no files to
  re-infer, so no Refresh"). Parking (`MenuButton::enabled(false)`) is for the other case — the
  in-flight one that item already uses for "Refreshing…".
  The internal table menu is therefore `View table`, `Profile table`, `Refresh table`,
  `Drop table`. Nothing is lost: the column list is on the row's own expansion, and Profile
  still answers everything about the data.
  **Drop the read-only-window work with it.** `ConfigureTarget::Edit` is set from exactly two
  places — the row menu, and `configure/views/footer.rs`'s post-save transition on a *New* table,
  which is external by construction (the palette only offers `New`). Remove the menu item and the
  window cannot receive an internal def at all, so an internal mood for it would be handling for
  an unreachable state. Make it structurally impossible; do not add a guard.
- **The catalog row says which origin it is** (gap found while building ED-03; the plan had the
  Configure window and nothing else). `entry.rs` picks its icon with
  `IconName::for_catalog(self.kind)` — kind only — so as written an internal table renders
  identically to one pointing at the user's own parquet. That is not cosmetic: it is the only
  thing standing between the user and ED-05's drop, where one origin deletes their data and the
  other does not. The distinction comes off `TableDef.origin`, which this task is what introduces,
  so it belongs here rather than in a later polish pass. Design call (icon variant vs. a row
  affordance) is open — do not invent a token; the design handoff's catalog surface is the
  reference, and AGENTS.md §3's "a missing component *state* belongs on the component's theme in
  the fork" applies if it turns out to need one.
- **Not** the drop confirm's wording or the sidebar drop's data deletion — those are ED-05's, with
  the editor's `DROP TABLE`, so one destructive action has one owner. See the note there.

**An internal table is a table row, and inherits every affordance one has.** `TableUpserted` puts
it in `ProjectState.tables` — which is required, because the store is the catalog and a table the
pane cannot see is not in the catalog at all. The consequence is the work: the row arrives holding
the whole table menu (`View table`, `Profile table`, `Refresh table`, `Configure`, `Drop table` —
pinned in order by `catalog/interaction.rs`'s `each_row_kind_offers_its_own_menu`), and three of
those five do not mean the same thing on a def whose data Strata owns. Settle each **here**, where
`TableOrigin` is introduced, rather than leaving the pane to discover it:

- **Configure** is a table's *only* edit surface (a view has "Edit query"; a table has this — the
  same test says so in as many words), and it is the one that goes: settled above, absent from the
  menu.
- **Refresh table** is fine and is *load-bearing* — re-inference against `.strata/tables/<slug>/`
  is how row counts move after an INSERT, and ED-05's `StoreEffect::RescanTable` depends on it.
  No change; noted so a later reader does not "fix" it.
- **Drop table** is ED-05's, above.

**The def travels and the data does not**, and the pane has to say so honestly. `project.json`
carries an internal def like any other, while `.strata/tables/` is gitignored — so a teammate who
clones the project gets a def with no data. The acceptance below already requires an honest
`Reg::Failed` row for that case; what it must not do is render it in the external-table
vocabulary ("could not read location …", which invites the user to go fix a path). The true
sentence is that the table's data is local to the machine that created it. One message, stated
where the other origin-dependent wordings are (ED-05's report), not a second vocabulary here.

## Acceptance

- CTAS over a large result completes without proportional RAM growth (streamed), lands the row
  in the sidebar **marked as internal**, persists the def, and the table is queryable in another
  tab after the epoch bump.
- `CREATE TABLE t (a INT)` yields an empty queryable table with the declared schema; a following
  restart replays it (schema from the IPC file, not the def).
- `IF NOT EXISTS` no-ops with a report; plain create over an existing name errors; `OR REPLACE`
  replaces; constraints/`TEMPORARY` refuse tersely.
- `CREATE TABLE __snap_1 (a INT)` refuses with the reserved-name message; a def hand-named
  `__snap_1` fails registration through `register_external` with the same class of error.
- Close and reopen the project: the internal table returns through the ordinary pass
  (`register_project` test in `strata-core` covers the headless half). A copy of the project
  without `.strata/tables/` shows an honest `Reg::Failed` row **naming the real cause** — the data
  is local to the machine that created it, not a path the user can go and fix.
- `each_row_kind_offers_its_own_menu` gains the internal-table case, asserting the item list
  `View table` / `Profile table` / `Refresh table` / `Drop table` in order — Configure absent, so
  its omission is pinned by the same test that pins every other row kind's, rather than being
  incidental to whoever edits the menu next.
- `TableMeta.rows` is exact for Arrow tables (footer-read test with a multi-batch file).

## Verification

`cargo test -p strata-core`; run the app end to end (CTAS → sidebar → restart → still there);
`git status` confirms data files are ignored.
