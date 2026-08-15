# DB-09 · Unqualified names resolve across the connections

**Workstream:** Database connections · **Status:** ✅ built (2026-08-15) · **Depends on:** DB-02

## Goal

`SELECT * FROM orders` against a connected Postgres, rather than `SELECT * FROM pg.public.orders`
every time — so the three-part name is what you type to reach *across* sources, not what you type
to work in one.

Asked for directly (2026-08-14): "We want users to be able to write `select * from orders` and not
`select * from pg.public.orders` every time."

## What was built, and why it is not what this file first said

This task was written around a **current database and schema** for the session — Trino's
`USE catalog.schema`, psql's search path — with a tree gesture, a status-bar indicator, a typed
`USE` arm and a `RESET`. It shipped as a **statement resolution pass** with none of those, on
Alex's own reframing (2026-08-15): *"instead of a set default for a session, could we extend the
planner so that if only one table over all the databases has that name we auto-inject the
qualifier, and only a clash errors?"*

That is the better design, for the reason this file had already identified as its whole risk.
`providers::in_workspace` answers `true` for every bare name, and four rules turn on that answer:

| Reader | What it does with `in_workspace` | What a moved default breaks |
|---|---|---|
| `is_snapshot_ref` | fences `__snap_` names in the workspace catalog | the fence's own argument stops holding |
| `ddl::bare_name` / `Engine::is_internal` | decides whether a write may target a relation | a bare INSERT target reads as workspace-owned when it is remote |
| `PlanDeps` / `ViewMeta` | records workspace scans **bare**, remote scans **qualified whole** | **the sharp one** — a view whose body says `orders` is recorded as a workspace dep while reading Postgres, so dropping an unrelated workspace table names a view that never read it, `view_problem` cries wolf, and forgetting the connection matches nothing |

A moved default makes all four wrong at once and the fix has to reach every reader. Resolving on
the **statement** leaves all four untouched: the plan carries the name the read actually reached,
so `PlanDeps` records `pg.public.orders` in its remote half for free and `in_workspace` did not
change at all. There is no mode, nothing to display, and nothing a restart has to clear.

## The rule

**A bare name is the workspace's wherever a statement can *make* one, and resolved across sources
wherever it only reads.**

`sql::qualify` (`crates/strata-engine/src/sql/qualify.rs`), run from `sql::parse_one` — the one
parse in front of both the router and the planner:

1. **The workspace wins**, asked of the schema provider's `table_exist`, which sees tables, views
   and the result spool. Nothing that resolves today changes meaning.
2. **Exactly one relation of that name across the connected catalogs** — the name is rewritten
   whole, every part quoted, in the spellings that reach it (the catalog as the connection
   registered it, the schema and relation as the server spells them). Views and materialized views
   included: the search asks the providers, and a connection's listing is
   `relkind IN ('r','p','v','m','f')`. The search runs in the schemas each connection **shows** —
   see the correction below.
3. **More than one** — refused, naming every candidate: `'orders' is ambiguous:
   'pg.public.orders', 'pg.analytics.orders'. Qualify it`.
4. **None** — left bare, which is the error DataFusion already gives.

**The search runs in the schemas each connection shows** — a correction, made while looking at the
built thing (Alex, 2026-08-15: *"I dont get why sessions would conflict if its not configured as an
enabled schema?"*). The first version searched every registered schema, reading
`PgStore::schemas` *scopes display, never resolution* literally. That rule is about a name the user
**wrote**: `pg.analytics.sessions` must keep resolving with the schema hidden. It says nothing
about where an *implicit* search looks — and searching everywhere made a hidden schema refuse a
query about a visible one, naming a relation the tree does not list. The tree is the statement of
what the user works with, so that is the scope. The set is one live cell shared between the
connection and its catalog provider (`db::Shown`), written by `connect` and by the Schemas… picker
through `Engine::show_schemas`: the picker writes the def **without reconnecting** (a display
choice must not rebuild a pool), so a copy taken at connect would be stale the first time it was
read.

Resolvable positions are named per statement kind, with the catch-all deliberately narrow: a kind
the pass does not name keeps today's meaning. CTE names and registered table functions are held
back. Two carve-outs, and they are **not** the same kind of thing:

- **A create target is never resolved**, permanently — `CREATE TABLE orders` names a relation that
  does not exist yet, so there is nothing to resolve *to*, and resolving it would read a plainly
  local intent as "make it on the server".
- **A write target is read but not rewritten** — `INSERT`'s only, refused in `ddl::in_database`'s
  existing sentence rather than as a name that does not exist. **This is temporary and DB-10 owns
  it** (Alex, 2026-08-15: *"I want the write to dispatch just like read does"*). A write target
  addresses a relation that already exists, so it resolves exactly as a read does; the refusal is
  what that rule looks like while writing to a database is impossible at all. When DB-10 makes a
  connection writable, `Pass::write_target` becomes a rewrite and the arm answers about a
  read-only connection in its own words, whether or not the user typed the qualifier.

## Consequences that had to move with it

- **The read path takes the statement, not the buffer.** `Engine::read`, `run_and_snapshot`,
  `materialize` and `explain::run_explain` are handed what the router judged;
  `query::plan_statement` is `SessionContext::sql_with_options` with the parse taken out (plan,
  verify, execute — the same three steps in the same order). Rendering a resolved statement back
  to text to keep the old signatures is the round trip `ddl::copy` avoids for the same reason.
  This removes a second parse rather than adding one.
- **`Engine::parse_one` is not spawned onto the runtime.** It is a parse and some map lookups,
  and it has to land before the first await or `query` no longer publishes its in-flight entry on
  the first poll — leaving `DispatchGuard` nothing to retract when a caller goes away mid-run.
- **`views::create` parses and plans rather than calling `ctx.sql`**, so a view's body resolves
  like any other read and the registered plan records the dependency it truly has. The def still
  stores the SQL the user wrote.
- **The pass reads `datafusion.catalog.default_catalog`/`default_schema`, not `CATALOG`/`SCHEMA`.**
  The question is "would the planner have found it", and the planner asks the config. Same two
  values in every Strata engine; what it buys is that a context built any other way cannot have
  its own default read as a database connection.
- **`sql::validate` runs the pass before classifying** and turns a refusal into a squiggle on the
  name (`lex::byte_span`, lifted out of the resolver so both spell one rule).
- **The two surfaces that answer about names had to learn the same question** — found by using it
  (Alex, 2026-08-15: *"the bare name while it works isnt tied into auto complete or validation"*),
  and both were the same root cause: they asked *is this in the workspace?* where the engine now
  asks *does this resolve?*
  - **The keyword-typo lint squiggled a working query.** `keyword_typo_hints` skipped a word
    `ctx.table_exist` knew, so a resolvable `orders` read as an unknown word one edit from `ORDER`.
    It was invisible before only because a *table not found* error covered the same span; resolving
    the name removed the error and left the hint standing. It asks `qualify::resolves` now — the
    same rule, one name at a time, and an ambiguous name counts as known because the statement pass
    has a better sentence for it.
  - **Completion offered a database's names only behind a qualifier** (DB-06's shape, right while
    bare names were always the workspace's). `push_relation_targets` now offers each connection's
    relations where a relation goes, at `T_SECONDARY` so the project's own still rank first — which
    is the precedence rule written into the ranking. **A row is named by what you would type**
    (`offered_name`): bare where that resolves, three-part where the project's catalog or a second
    shown schema holds the name. That is also what makes the pool's one-row-per-name rule hold
    across sources — labelled bare, a connection's `users` was silently swallowed by the
    workspace's.

## Acceptance (all covered)

Unit, against a fake catalog (`sql/qualify.rs`): the qualification, the workspace winning, the
ambiguity refusal, a CTE left alone, a create target left alone while its own body resolves, the
refused write, both halves of the `__snap_` rule, and a project with no connection being untouched.

Integration, against the real container (`tests/postgres_federation.rs`, phase
`unqualified_names`): a bare read of a remote table and of a remote **view**; a schema the
connection does not show neither capturing a bare name nor being reachable by one, while its
qualified name still resolves; `Engine::show_schemas` bringing it into reach and taking it back out
**with no reconnect**; the refused write; ambiguity across two *shown* schemas, with both
qualification and hiding one as the fixes; a workspace table taking the name back while the
qualified name still reaches across; and **the dependency assertion this task exists for** —
dropping a same-named workspace table does not name the view created over the remote one.

## Not built, deliberately

- **No `USE`, no current-database state, no status-bar indicator, no `RESET`** — there is no
  session state to display or clear.
- **A bare name can still change meaning** when a workspace table takes it. Completion is the
  answer: every row is named by the spelling that reaches it, so accepting one always writes
  something explicit enough to keep working.
- **`SHOW COLUMNS FROM <bare>`** is not a read position (DataFusion rewrites it into an
  `information_schema` query of its own). `DESCRIBE` is.
- **Drops keep their own wording.** `DROP TABLE orders` for a name only a connection has still
  says the table does not exist; only `INSERT` was given the remote sentence, which is where the
  contradiction with a working `SELECT` was sharpest. DB-11 takes this over with the same rule a
  write gets — a drop addresses an existing relation, so it resolves — and its file records the
  one friction that creates (a span splice has no bytes for a name the pass resolved).

## Files

`crates/strata-engine/src/sql/{qualify.rs, mod.rs, validate.rs, lex.rs, resolve.rs,
complete/mod.rs, complete/tests.rs}` ·
`crates/strata-engine/src/{lib.rs, query.rs, explain.rs, db/mod.rs, ddl/views.rs, ddl/mod.rs}` ·
`crates/strata-freya/src/apps/project/views/dialogs/schemas.rs` ·
`crates/strata-engine/tests/postgres_federation.rs` · `sample/postgres/` + `sample/.strata/project.json` ·
`docs/CONNECTIONS_SPEC.md`, `docs/STATEMENTS_SPEC.md`, `docs/EXPLAIN_PLAN_SPEC.md`,
`docs/reference/{INVARIANTS, ENGINE}.md`, `AGENTS.md`, `README.md`.
