# DB-11 · Remote statements the server runs

**Workstream:** Database connections · **Status:** ⬜ · **Depends on:** DB-10 (the `read_only`
gate, `remote_target`, and the listing refresh)

## Goal

`CREATE VIEW pg.public.active AS SELECT …`, `CREATE MATERIALIZED VIEW`, `DROP VIEW`,
`DROP TABLE`, a plain `CREATE TABLE pg.public.t (payload jsonb, …)` — with the server's own
column types — and `UPDATE` / `DELETE` with the server's own affected-row count, all executed
**on the server**, over the connection the target names. DataFusion cannot plan any of these
against a remote catalog; the mechanism is dispatch, not planning: the statement the server runs
is the statement the user typed, with the catalog qualifier cut out. The split with DB-10 is by
**mechanism**, not by DDL/DML: what DataFusion can plan (INSERT, CTAS) goes there; what only the
server can run goes here. Same opt-in as DB-10 — a read-only connection refuses with the toggle
named.

## Current state (verified 2026-08-15)

- **The DDL statements are already parsed and routed.** `CREATE VIEW` (materialized included —
  it is the same `SqlStatement::CreateView`), `DROP VIEW`/`DROP TABLE` and `CREATE TABLE` all
  reach existing `StmtKind` arms; for them, no `classify` change and no new completion coverage.
  What changes is inside the arms: a remote-qualified target becomes a branch instead of
  `bare_name`'s refusal. `CREATE MATERIALIZED VIEW` on a **workspace** name stays refused
  exactly as today (`views::definition`'s clause table) — the workspace has no materialized
  concept; only the remote branch accepts it. **`UPDATE` and `DELETE` are the exception**: they
  fall to default deny today (`Blocked::Unsupported`), so they are the two genuinely new kinds
  this task mints — step 4 carries their cost.
- **The local view arm's exhaustive clause destructure does not apply remotely, by its own
  argument.** The destructure exists because the local arm *rebuilds* the statement around the
  query's canonical rendering, so an unread clause is a clause silently dropped. A dispatched
  statement drops nothing — `WITH CHECK OPTION`, storage parameters, `TABLESPACE`, every clause
  we do not model travels intact and the **server** is the clause gate. That is what makes this
  a generic capability rather than a clause whitelist.
- **The rewrite is a span splice, and the parser can carry it.** DataFusion 54's sqlparser
  (0.59) implements `Spanned`; an `Ident` carries its byte span in the original text. For each
  three-part name whose first segment folds to the target connection's catalog, cut from the
  catalog ident's span start to the schema ident's span start (the dot goes with it) — the rest
  of the statement is the user's own bytes, verbatim. Never an AST re-render (`Display`
  round-trip fidelity is exactly the bet we must not make) and never a plan unparse (the known
  DF 54 unparser gaps). sqlparser answers `Span::empty()` for some nodes — a name whose span
  cannot be trusted is a **refusal**, never a guess.
- **The raw-execution seam exists**: the crate's pool exposes a direct connection
  (`connect_direct` → `tokio_postgres` client) — verified in
  `datafusion-table-providers-postgres` 0.13.0. A new `db.rs` method wraps it: execute one
  statement on connection X, errors surfaced through `catalog::readable`'s existing peeling.
- **DB-03's two-list split pays again here.** `ViewMeta::remote` (mirrored freya-side as
  `ViewInfo::remote_deps`) records which workspace views read `pg.public.orders` qualified
  whole — so a remote `DROP` can name the workspace views it strands, in `left_invalid`'s own
  words, without cascading anything.

## Build

1. **The body check, in front of the dispatch.** Every relation named anywhere in the statement
   must resolve into the *target's* catalog — collected off the parsed AST, compared through
   `fold_ident` like `remote_target` does. A workspace name or another connection's is refused
   **by name**: a server-side view cannot read across sources, and a bare name is refused too
   (on the server it would resolve by search path — a different answer than the editor gives the
   same spelling; the refusal says to qualify it). If DB-09 lands first, its resolution runs
   before this check — note the interaction in whichever file lands second.
2. **The splice** (`ddl::` helper beside `remote_target`): strip the catalog qualifier from every
   same-connection name by span, refuse on any untrusted span, hand the resulting text to the
   `db.rs` execute seam. One helper, all six arms.
3. **The arms.** `CreateView` (materialized or not), `DropView`, `DropTable`, and the
   plain-columns half of `CreateTable` each gain the remote branch: resolve `remote_target`,
   check writable, body-check, splice, dispatch, report in the server's terms ("View 'public.v'
   created on 'pg'"). CTAS with a remote target stays **DB-10's** arm (data movement, not DDL) —
   the `CreateTable` arm branches on "has a query" exactly as it already does locally. A remote
   `DROP TABLE`'s report names the workspace views left invalid, off `ViewMeta::remote`.
4. **`UPDATE` and `DELETE`, remote-only, as two new intercepted kinds** (asked for 2026-08-15 —
   and once `DROP TABLE` dispatches, refusing `DELETE` is not a safety line but a hole: dropping
   a whole table while its rows are untouchable is no policy at all). Each gains a `StmtKind`
   and a `classify` arm — `Capability::Agent` keeps refusing both, since the arm's `Blocked` is
   the agent's message — and ED-11's rule bills its price in the same change: the completion
   offer grows the two statement templates. The arm is the same shape as the DDL ones — resolve
   `remote_target`, check writable, body-check the SET/WHERE subqueries, splice, dispatch — and
   the report is the server's own affected-row count (`execute` returns it). A **workspace**
   target is refused with its own sentence naming where the statement works: workspace tables
   are append-only IPC files DataFusion cannot update in place, and `Blocked::Unsupported`'s
   generic wording would stop being honest once the same verb works one qualifier away. No
   listing refresh, no epoch bump — no enumeration moved (the RescanTable-style row-count
   re-read is def-side machinery a remote relation does not have; the tree shows no remote row
   counts). No WHERE-less guard: the typed statement is the intent and the toggle is the belt,
   the same terms as every other statement here — `DROP TABLE` dispatches on those terms
   already, and a confirm only DML gets would be a second, inconsistent surface.
5. **Settle**: the same listing re-enumeration + epoch bump DB-10 built, plus evicting the
   connection's cached per-relation provider on a `DROP` (a stale provider would keep answering
   scans for a relation the server no longer has — the reconciliation message exists, but the
   cache must not pre-empt it). A dropped relation that workspace views read settles those rows
   `Failed` through `catalog::view_error` on the next pass, exactly as a server-side drop does
   today — no new machinery, the drop just makes it happen sooner and with warning.
6. **Still refused, by name**: `ALTER`, `TRUNCATE`, `MERGE` and everything else stay
   default-deny (`Blocked::Unsupported`) — `TRUNCATE` is a WHERE-less `DELETE` with nothing new
   to say, `ALTER` is a large surface with its own listing-refresh questions, and the splice
   mechanism generalizes to any of them if asked for; this file is where that note lives.
   `CREATE`/`DROP FUNCTION` keep DataFusion's own qualified-name refusal. The agent stays
   read-only, verified as in DB-03/DB-10.

## Acceptance

- With a writable connection: a remote `CREATE VIEW` lands, shows in the tree without ↻, and is
  queryable; `CREATE MATERIALIZED VIEW` likewise (and the same statement on a workspace name is
  still refused); a `CREATE TABLE` with a `jsonb` column lands with the server's type; `DROP`s
  remove, evict the provider cache, and a re-query gets the reconciliation's sentence, not rows.
- A clause we do not model (`WITH CHECK OPTION`, or a storage parameter) survives dispatch
  verbatim — assert the server-side definition contains it.
- A body naming a workspace table, another connection's, or a bare name is refused by name; a
  read-only connection refuses with the toggle named; a spliced statement is byte-identical to
  the typed one outside the removed qualifiers (unit-testable without a container).
- A remote `DROP TABLE` names the workspace views reading it and does not cascade.
- `UPDATE pg.public.t SET … WHERE …` and `DELETE FROM pg.public.t WHERE …` land and report the
  server's affected-row count, confirmed by read-back; the same verbs on a workspace table are
  refused by the sentence naming where they work; completion offers both templates.
- `Capability::Agent` verified unchanged.
- Container phases in `tests/postgres_federation.rs` for the create/query-back/drop cycle and
  the clause-fidelity assertion; the splice and the refusals unit-test against parsed statements.

## Files

`crates/strata-engine/src/ddl/{mod,tables,views}.rs` ·
`crates/strata-engine/src/sql/validate.rs` (the two new kinds) · the completion statement
templates (ED-11's pool) · `crates/strata-engine/src/db.rs` ·
`crates/strata-engine/tests/postgres_federation.rs` ·
`docs/STATEMENTS_SPEC.md` (§4's table gets the new remote column answers),
`docs/CONNECTIONS_SPEC.md`.
