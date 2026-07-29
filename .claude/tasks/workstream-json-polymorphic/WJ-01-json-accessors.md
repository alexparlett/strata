# WJ-01 · Postgres-style JSON accessors over Utf8 columns

**Workstream:** JSON · **Status:** ✅ `[core ✓]` · **DEV_TASKS:** — · **Depends on:** —

## Goal
`json_get` / `->` / `->>` / `?` and friends, so a Utf8 column holding JSON **text** can be
navigated in SQL rather than only pattern-matched.

Independently useful (any stringified nested column becomes queryable), and the thing that makes
[WJ-02](WJ-02-polymorphic-json-format.md) worth building — that reader's entire output for a
polymorphic field is Utf8 JSON, and without accessors the user gets `LIKE` and nothing else.

## The decision
Take [`datafusion-functions-json`](https://github.com/datafusion-contrib/datafusion-functions-json)
rather than write UDFs. Checked before committing to it:

- **Version tracks DataFusion's major** — `0.54.2` (2026-06-17) declares `datafusion ^54`, matching
  our pin. The coupling is the point: it moves in lockstep with a DataFusion bump rather than
  drifting.
- **No arrow of its own.** It reaches arrow through DataFusion's re-export, so there is no second
  arrow in the graph to disagree with the 58.3 we resolve today. Its whole dependency set is
  `jiter`, `log`, `paste`, `serde_json` — the last of which we already have.
- Not an ASF release (the crate says so itself); it is the DataFusion-contrib org's.

## What it provides
`json_contains`, `json_get`, `json_get_str` / `_int` / `_float` / `_bool` / `_json` / `_array`,
`json_as_text`, `json_length`, plus the operators `->` (`json_get`), `->>` (`json_as_text`) and
`?` (`json_contains`). Casts are rewritten to the typed variant, so
`json_get(x, 'a')::string = 'ham'` plans as `json_get_str(x, 'a') = 'ham'`.

## Build

### Registration — one call, in the right place
`build_context` (`engine/mod.rs`), beside the catalog/schema naming and **not** in `catalog`:
these belong to what an engine *is*, not to what a table brings. `json_get('{"a":1}', 'a')` is a
valid query with nothing registered at all.

Warned rather than fatal on failure, which is not the usual "fail loud" exception being waived —
the failure genuinely cannot be silent. A registration that did not happen surfaces as
`Invalid function 'json_get'` on the first query that needs one, which names itself better than a
panic during engine construction would.

### The language service — nothing to do
This is the part worth knowing before planning the work: `engine::functions::snapshot` builds the
`FunctionCatalog` by walking the **live registry** (`ctx.udfs()` → `signature()` / `return_type()`
/ `documentation()`). So registering the UDFs is the entire integration — all ten reach
autocomplete, signature help and the docs panel with no per-function work, no table to maintain,
and no chance of the completion pool and the engine disagreeing about what exists.

## Verified behaviour (run against a live engine)

| Expression | Result |
|---|---|
| `json_get_str(doc, 'a', 'b')` | ✅ `deep` |
| `doc ->> 'n'` | ✅ `7` |
| `(doc -> 'n')::bigint` | ✅ `7` — the cast rewrite works |
| `json_contains(doc, 'a')` | ✅ `true` |
| `json_length(doc, 'arr')` | ✅ `3` |
| validator on `->` / `->>` | ✅ no diagnostics |
| `doc -> 'a' -> 'b'` (bare `->` in the select list) | ✅ JSON text — **used to panic the query task**, see below |
| `doc ? 'a'` | ⚠️ parses **only** under the postgres dialect — see below |

### A bare `->` used to panic, and no longer can

```
not implemented: See ARROW-8817.
  parquet-58.3.0/src/arrow/schema/mod.rs:851
```

`json_get` returns a sparse `Union` — the crate's stand-in for Postgres `jsonb`, which Arrow has no
equivalent of. **parquet-rs cannot write a union at all**, and at the time every run materialized to
a *parquet* snapshot before the grid saw a row, so a bare `->` took the query task down. Registering
these functions is what made that reachable: before, nothing could produce a union column.

The first fix was `query::flatten_json_unions`, a **storage gate** — project the union to text, and
refuse the cases that could not be projected (a union nested in a struct, a dictionary-wrapped one,
and later a zero-field struct). It was found incomplete twice in one review, which is the argument
that eventually replaced it.

**The snapshot is Arrow IPC now** (see AGENTS.md §2), which stores unions natively, so the gate and
all of its refusals are gone. What remains is `query::json_unions_as_text`, doing one job and no
longer a correctness boundary: arrow renders a union as `{str=x}` / `{int=7}`, and nobody typing
`content -> 'type'` wants to read that. `json_union_to_text` gives back exactly the JSON the value
came from, so it changes how the column reads and not what it holds.

### The `?` operator is dialect-gated, not broken

```
generic  →  ParserError: Expected: end of statement, found: ?
postgres →  true
duckdb   →  ParserError
```

Not a placeholder-vs-operator problem as first assumed. sqlparser tokenizes `?` as
`Token::Question` and maps it to `BinaryOperator::Question` either way; what differs is
**precedence**. `GenericDialect` — DataFusion's default, and ours — overrides
`get_next_precedence` and omits `Token::Question`, so the parser stops before the operator is ever
consulted. `PostgreSqlDialect` includes it (sqlparser-0.62 `dialect/postgresql.rs:139`).

`datafusion.sql_parser.dialect` is **already a catalogued engine key** (`engine/config.rs:302`), so
anyone who wants `?` can set it to `postgres` in Settings ▸ Engine today. `json_contains` is the
spelling that works in every dialect, and is what the docs should keep naming.

Changing the *default* is [WJ-04](WJ-04-postgres-dialect-default.md) — it is a broader change than
it looks, because `engine/sql/lex.rs` hardcodes `GenericDialect`.

## The union, and where it now lands

`json_get` (and therefore `->`) returns arrow's `JsonUnion` — a `DataType::Union` with seven arms
(null / bool / int / float / str / array / object), the crate's stand-in for Postgres `jsonb`.
Traced through every surface:

- **The grid** renders it via arrow's `ArrayFormatter`, which has a `DataType::Union` arm
  (arrow-cast display.rs:563) — but as `{str=foo}`, which is why `json_unions_as_text` projects it
  to JSON text before it gets there.
- **Copy** handles it already: `serialize::is_nested` lists `Union(..)` beside Struct/List/Map, so
  the CSV/TSV/Markdown writers stringify it to compact JSON.
- **The snapshot** stores it natively (Arrow IPC), so nothing has to be refused.

Two loose ends, for a union that reaches the grid **without** passing through
`json_unions_as_text` — which only projects *top-level* JSON-union columns:

- `Kind::from_arrow` (`strata-model/src/schema.rs`) has no Union arm. `catalog::short_type` reduces
  `Union(Sparse, …)` to the base word `Union`, and the prefix chain has no match for it, so it
  falls through to `Kind::Str` — a string-coloured type dot beside a `dtype` reading `Union`.
- `Cell.null` comes from `cols[ci].is_null(r)` in `batches_to_rows`, and a `UnionArray` has no
  top-level null buffer (nulls live in the null-typed arm), so it is always `false`. A JSON null
  would render as arrow's `{null=}` instead of the configured NULL text, and would not dim.

**Where they actually bite.** Not a nested union, as first thought: `catalog::column_info` recurses
(`nested_children`), so a union inside a struct picks up `Kind::Str` on its *child* entry in the
inspector — cosmetic — while `Cell.null` only applies to top-level columns and so never fires. The
live path is a **top-level union that is not the JSON union**: an Arrow/IPC source file with a real
union column, which the removal of the parquet gate made reachable where it used to be refused.
Reasoned, not measured — no such file was constructed.

A `Kind::Union` arm plus a null check closes both. Not worth pre-empting: nothing produces such a
column today except an Arrow file someone deliberately built.

## State of play

Done. Registered, reaching the function catalogue. The union/parquet incompatibility that this task
uncovered no longer exists — the snapshot is Arrow IPC. The `?` operator is unavailable
(sqlparser); `json_contains` is the spelling that works.

## Acceptance
- `SELECT json_get_str('{"a":"x"}', 'a')` returns `x` with no table registered. ✅
- The `json_*` names appear in autocomplete with signatures, sourced from the registry snapshot. ✅
- The validator accepts `->` and `->>`. ✅
- **A `Union` result column round-trips.** ✅ — rendered as canonical JSON text for the grid
  (one test per union arm), and stored as itself, including nested inside a struct.
- The `?` operator is documented as unavailable (`json_contains` is the working spelling). ✅
- `cargo test -p strata-core`.
