# DB-07 · Column inspector + profiling for remote tables

**Workstream:** Database connections · **Status:** ✅ (2026-08-14) · **Depends on:** DB-05

## What was built, and the four decisions the building settled

1. **The selection widened to an enum, not a generalized string.** `ColRef` is
   `{ owner: ColOwner, path }` where `ColOwner` is `Entry { kind, name }` or `Remote(RemoteRef)`
   (`strata-model/catalog.rs`). A remote relation has no `Reg` row, no def and no one-segment
   name, so a `CatalogKind` beside a `String` cannot say where it is. `RemoteRef` is the
   **catalog name** plus schema plus relation — never the connection's URL — because every
   question asked of a remote relation is asked in SQL, and a URL can say neither the columns nor
   the `FROM`.
   **An empty `path` is the owner itself**, resolved by the panel to its first column: it is what
   a reveal leaves behind when only an introspection could name a column.
2. **The remote expression set is a `Profiled` value that decides the expressions *and* the
   renderer together**, because both turn on the one fact (`engine/profile.rs`). The median is
   dropped: `approx_percentile_cont` has no PostgreSQL spelling, DF 54's `PostgreSqlDialect`
   offers scalar overrides only, and a federated subplan has no per-expression fallback — so
   including it would not cost a median, it would fail the whole scan of any remote table with a
   numeric column. `stats_footnote` says so under the numbers, so the omission is stated rather
   than left to be noticed. `percentile_cont … WITHIN GROUP` was **not** substituted: it is an
   ordered-set aggregate the unparser has no expression to emit, so it would have been an
   assumption, and this task's whole point was not to make one.
3. **The tree's relation opens onto its columns, and the pane holds the one subscription.** The
   walk is synchronous and is the only place the tree's shape is decided, so it cannot await —
   and a virtualized row's scope is a slot, so a row cannot hold the subscription either. The
   walk therefore *returns* the relations it drew open (`Walked`), the pane subscribes to their
   columns keyed by `(relations, catalog epoch)`, and hands the answer back as an input on the
   next pass. Opening a relation costs one extra pass, during which the row is showing its
   loading note anyway. A per-row subscriber writing into a pane-local map was rejected: that is
   the shared-registry value in disguise, and it needed a re-walk tick besides.
4. **`reltuples` was not built, and that is a refusal rather than a cut.** The free tier for a
   remote column is the schema and nothing else. A row *estimate* has only one place to go — the
   ROWS row — and `completeness()` **divides by** it, so an estimated denominator under an exact
   null count is precisely the two-reads-as-one fault this panel refuses everywhere else
   (`the_bar_never_divides_one_read_by_another`). Stating it honestly would need either a second
   surface or an inexactness flag on `rows`, for a number a scan answers for real. Recorded here
   so it is not re-proposed as an oversight.

## Two corrections from the adversarial review — do not re-litigate either

5. **`Kind::Str` is not "a text column", and the remote set stops at a distinct count because of
   it.** The first version gave every `Kind::Str` column MIN and MAX on remote, which reads
   correct and is not: `Kind` comes from the **mapped Arrow** type, and DB-02 maps every type the
   connector cannot represent to `Utf8` (`UnsupportedTypeAction::String`) — so `jsonb`, `xml` and
   PostGIS geometry all arrive as `Kind::Str`. PostgreSQL has no `min(jsonb)`, and because a
   federated aggregate is all-or-nothing that would have failed the scan of *every* remote relation
   holding a json column, which is the exact failure `Profiled` was introduced to prevent. The
   mapping is lossy in precisely the way that matters, so the server type cannot be recovered at
   profile time and the bound has to be drawn at the kind. The distinct count survives (it needs
   only equality, which `jsonb` has); a type with no equality operator at all — `xml` — still fails
   loudly, which is the workstream's own accepted envelope for a rare type. Pinned by
   `a_remote_string_column_is_counted_but_never_ordered` and by the integration test's `tags JSONB`
   assertions.

6. **The tree hands the walk an *accumulated* map, not the query's current value.** The remote
   columns query is keyed by the whole open set plus the catalog epoch, and freya-query starts a
   changed key at `Pending` with no carried value — so reading the entry directly blanked *every*
   already-drawn relation back to its loading note whenever any other relation was opened, or
   whenever any unrelated catalog pass moved the epoch. The pane merges each settled answer into a
   map it keeps, which is the same rule the inspector's STATISTICS zone already holds: **never show
   less than a moment ago.**

   A third, smaller one: `describe_remote` derived a relation's table/view kind from the built
   provider's `table_type()`, which for the crate's federated `SqlTable` is hardcoded `Base` — so
   every remote view read as a table and the inspector's label contradicted the tree's. It now asks
   `DbSchemaProvider::table_type`, the relkind-aware one, which costs nothing.

**Verification.** The remote expression set is pinned **twice**. `engine::profile`'s own tests
render every expression through DataFusion's `PostgreSqlDialect` and assert the median is the one
difference — no container needed, so a working tree cannot lose the claim. They also caught the
first version of that list being written from memory rather than measured: `count_all()` unparses
as `count(1)`, not `count(*)`, and `ident` renders a **quoted** identifier (`count("total")`), which
is both plain SQL PostgreSQL runs and what preserves a server's own column spelling. The expected
list now says what the unparser actually emits.

`postgres_federation.rs`'s `profiling` phase pins the other half — that the aggregate federates
into one remote node, that the server runs it (`jsonb` column included), and, in
`unsplit_expression_set_fails_on_the_server`, that the *unsplit* set does **not**. Without that
last one, deleting `Profiled` and profiling everything with the workspace set would leave the whole
suite green, because every other assertion only checks that the remote set works.

**The container phase has never been executed.** No container runtime was reachable from this
worktree: colima's VM listed `Running` while `colima status` failed with `error retrieving current
runtime: empty value` and its socket refused connections, and `~/.testcontainers.properties` points
`docker.host` at Testcontainers Desktop on `127.0.0.1:53100`, which answered 502 to both `/_ping`
and `/version`. So CI is its first run, and the `profiling` phase should be treated as unproven
until it goes green there. Everything else passes: `cargo clippy --workspace --all-targets --locked
-- -D warnings` and `cargo test --workspace` with CI's own two container skips.

## Goal

Selecting a remote table in the tree points the column inspector at it, and profiling works
on the same terms as a workspace table — opt-in, confirmed, one entry point — with a
**remote-specific expression set** federating to the server (the local set's median cannot;
Current state has the proof). Two structural decisions: the selection model widens in
`strata-model` to name a remote relation, and the profile request gets a window-side slot
because a remote table has no `ProjectState` row — the store grows nothing.

## Current state (verified 2026-08-13, corrected in review)

- The inspector reads the selected table's `TableMeta` columns; profiling is P3-09's shape:
  the row holds `Option<ScanId>` (a nonce minted per ask), the numbers live only in the
  freya-query cache entry that key names (`stale_time(MAX)`, `clean_time(MAX)`), a re-scan
  is a new nonce, and every trigger goes through `ProfileActions::ask` — the confirm on
  first scan, straight through on re-scan (INVARIANTS: "an expensive, opt-in result…" and
  "one entry point per expensive action").
- **Selection is `CatalogSelection = State<Option<ColRef>>`** (`state/catalog.rs:19`,
  consumed at `views/inspector/mod.rs:35,116`) — a variant-less struct
  `ColRef { kind: CatalogKind, owner, path }` in **`strata-model/catalog.rs:48-55`**; there
  is no `Selection` enum on this surface (the only enum of that name is the results grid's
  cell selection, unrelated). Widening it to carry a remote target is therefore a
  **model-crate change** with two consumers to audit, not "add a variant".
- **`ProfileActions` is `ProjectState`-row-bound end to end**
  (`dialogs/profile_confirm.rs:79-140`): `needs_confirm` reads `project.profile_scan`,
  `start` calls `request_profile` and *bails silently when the row is absent* — precisely
  the confirmed-cost-then-nothing regression the suite pins elsewhere
  (`catalog/interaction.rs:1086-1110`). A remote table has no row by design, so
  ask/needs_confirm/start/clear/reveal all need a second storage backing, not "one new arm".
- **The remote profile cannot simply federate `run_profile`'s SQL**: `aggregates()` gives
  every `Kind::Num` column a median via `approx_percentile_cont`
  (`engine/profile.rs:98-123`) — a DF-only aggregate with no Postgres spelling, and DF 54's
  `PostgreSqlDialect` has **no aggregate override hook** (scalar overrides only). Since a
  federated subplan has no per-expression fallback, profiling any remote table with a
  numeric column would die server-side. Audit the whole expression set the same way
  (`approx_*` anything is suspect); the remote profile needs its **own expression set**,
  restricted to aggregates the unparser renders into SQL Postgres runs.
- **`profile_sql` cannot render a qualified owner**: it wraps the whole name in
  `quote_ident` (`profile.rs:166-170`), which emits `FROM "pg.public.orders"` — one
  identifier, resolving nothing (mod.rs:1770-1776). The remote arm renders the owner
  segment-by-segment through the case-preserving helper DB-06 exports.
- Columns for a remote table come from the cached provider's Arrow schema (DB-02) — cached
  **after first touch**: the first selection of a table this session performs that one
  introspection, so the inspector budgets a loading state for it; subsequent selections are
  instant.
- Free stats (`free_stats`) read listing/file metadata — a remote table has none; the
  inspector's "free" tier for remote is whatever the Arrow schema says (types, nullability
  as declared) plus the server's own cheap facts if any (`pg_class.reltuples` is a
  free-tier row-estimate candidate — clearly labeled an estimate, and already in DB-02's
  connect-time listing query's reach).

- **The tree's relation rows are leaves, and that is this task's to change** (DB-05, as built):
  a relation draws no disclosure today because its columns are the same introspection this task
  performs, and a column row under it could not be *selected* until `ColRef` widens here. So the
  affordance arrives with the capability. The tree is virtualized since DB-05's follow-up, so that
  is **two edits in the walk, not a row-local disclosure**: `connection.rs`'s `database()` gives a
  relation `Node::branch(.., open, can_open, ..)` in place of today's `Node::leaf`, and pushes its
  column rows after it, off the same read the inspector uses; `relation_row` in the same file only
  draws what the walk decided.

## Build

1. **Selection** — widen the selection model in `strata-model` (the honest scope from
   Current state): `ColRef.owner` generalizes to a target that can name a remote relation
   (`(connection url, schema, table)`) with both existing consumers audited; tree selection
   sets it; the inspector renders the remote header (connection badge + qualified name) and
   the column list from the engine's cached schema — with a loading state for the
   first-touch introspection.
2. **`ProfileTarget` with one `Query` builder** — `ProfileTarget::{Workspace(kind, name),
   Remote { url, schema, table }}`, and the freya-query `Query` (keys, `stale_time`,
   `clean_time`) is **built in one place over the target** — never a second spelling beside
   the workspace one (INVARIANTS: "the `Query` is the identity, built in one place").
   The request storage generalizes with it: the workspace arm keeps the row's
   `Option<ScanId>`; the remote arm's slot is a window-level satellite
   (`(url, schema, table) → Option<ScanId>`), dropped with the window, on Forget, and on
   epoch move (a re-connect invalidates what the scan described). The store is untouched.
3. **`ProfileActions` generalizes over the target** — `ask`/`needs_confirm`/`start`/
   `clear`/`reveal` take `ProfileTarget` and read whichever storage backs it (the silent
   remote no-op in Current state is the failure this step exists to prevent — a confirmed
   ask that starts nothing is the pinned regression class). Same confirm component, wording
   naming the server ("Profiling scans 'pg.public.orders' on the database…").
4. **The remote profile expression set** — its own, in `engine/profile.rs` beside the local
   one: count / null count / min / max / avg where the unparser provably renders them, the
   median **dropped for remote** (or spelled `percentile_cont … WITHIN GROUP` only if a
   verified unparse path exists — never assumed), every included aggregate pinned by the
   integration test's EXPLAIN. The rendered SQL builds the owner through DB-06's
   segment-quoting helper, so "view as query" hands over a runnable statement.
5. **Free tier** — types/nullability from the schema; `reltuples` as a labeled estimate if
   it fits the existing free-stats card without a new surface.
6. **Docs** — the inspector/profiling notes in `docs/reference/SETTLED_TASKS.md`-adjacent
   docs stay true; INVARIANTS' profiling entry gains the remote clause (request in a
   window-side slot when there is no row — the rule generalized, not excepted).

## Known gap — a first-touch remote scan dispatches late, or not at all

Found by the code review after the adversarial one, and **left unfixed on purpose**: closing it
reopens a design decision rather than patching a line.

Nothing subscribes to a remote relation's profile query except the inspector's `ColumnPanel`, and
that is only rendered once the relation's columns have settled. So a confirmed scan records its
`ScanId` and asks the engine for nothing until the introspection lands — and if that introspection
*fails*, never: the panel shows the column error, the request stays in the satellite, and
`needs_confirm` now answers false, so a second press goes straight through and also does nothing.
The workspace side has explicit machinery for exactly this shape (`entry_row` mounts `ProfileWatch`
whether or not there is room to draw a spinner, because the subscription is what dispatches).

**Why a watcher cannot simply be added.** Building a `ProfileTarget::Remote` needs the table/view
`kind`, which only the introspection knows and which is **part of the query's cache identity** — so
a watcher mounted before the columns land would have to guess `Table`, key a *second* cache entry,
and dispatch a second scan for any remote view. Closing this properly means taking `kind` out of
the cache key and sourcing the profile verb elsewhere, which is a real design change with its own
tradeoffs (it was weighed once while building this task and settled the other way, for one
vocabulary across both arms).

**The exposure is now small**, because the inspector holds settled answers
(`use_remote_schemas`): any relation whose columns have ever been read dispatches immediately.
What remains is the *first* touch of a relation whose introspection is slow — the scan starts a
beat later — or fails, where it is recorded and never asked for.

## Acceptance

- Selecting a remote table shows a loading state at most once (the first-touch
  introspection), then its columns; re-selecting is instant; selecting back a workspace
  table is unchanged.
- Profile confirm → the remote expression set's numbers arrive, and the integration test's
  EXPLAIN shows the federated aggregation for **that** set (the median is absent by
  design); a confirmed ask that starts nothing is impossible by construction (the
  generalized `start` has no silent bail arm).
- "View as query" on a remote profile hands over SQL that runs (segment-quoted owner).
- Re-ask re-scans (new nonce); forget/↻ invalidates; nothing lands on `ProjectState`
  (grep-proof).
- Existing inspector/profiling tests untouched; new coverage for the remote arm with a fake
  engine listing (no network in UI tests).

## Files

`crates/strata-model/src/catalog.rs` (`ColRef`/selection widening) ·
`crates/strata-freya/src/apps/project/` (inspector views, `dialogs/profile_confirm.rs`'s
`ProfileTarget` generalization, the window-side request satellite) ·
`crates/strata-engine/src/{profile.rs (remote expression set + qualified rendering),
catalog.rs, db.rs (`reltuples`)}` · `crates/strata-engine/tests/postgres_federation.rs` (the
profile EXPLAIN phase) · `docs/reference/INVARIANTS.md`.
