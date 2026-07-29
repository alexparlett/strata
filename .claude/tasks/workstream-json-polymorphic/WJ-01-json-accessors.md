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
| `doc ? 'a'` | ❌ `ParserError: Expected: end of statement, found: ?` — unavailable, use `json_contains` |

### The blocker, and its fix: a bare `->` used to panic

```
not implemented: See ARROW-8817.
  parquet-58.3.0/src/arrow/schema/mod.rs:851
query task failed: task 9 panicked with message "not implemented: See ARROW-8817."
```

**parquet-rs cannot write a `DataType::Union` at all**, and every run materializes its result to a
parquet snapshot (`engine::query`, `docs/SNAPSHOT_SPEC.md`) before a single row reaches the grid.
So this is not an *export* problem as first assumed — it is a **query** problem, and the failure is
a panic rather than an error.

This is a regression introduced by registering the functions: before, no query could produce a
Union column. `content -> 'type'` is the most natural thing a user will type after reading any
`json_get` documentation.

**Fixed by `query::flatten_json_unions`** — a projection on the logical plan, applied in
`materialize` right after the SQL is planned:

- a column whose type is the JSON union is wrapped in **`json_union_to_text`**, the crate's own
  answer to this (its doc comment names the parquet writer explicitly). Scalars render as
  `true` / `42`, strings are JSON-quoted, the array and object arms pass through **verbatim** —
  they are already raw source text inside the union, so nothing re-serializes them — and the
  JSON-null arm becomes a real SQL `NULL`.
- a union **nested** inside a struct or list (`struct(x -> 'a')`) has nothing to wrap, since
  `json_union_to_text` takes the union itself. It is refused by name instead, which is the one
  thing the panic could not do.
- a result with no union is returned untouched, so nothing is planned in the common case.

On the **logical plan** rather than per batch, deliberately: the snapshot, the grid's `ColumnInfo`,
the page reads and every later export then agree on one schema. A batch-level repair would leave
`df.schema()` claiming a type the file does not hold.

This also disposes of the three "needs a decision" items below without deciding them — a union can
no longer reach a result at all, so `Kind::from_arrow`'s fallthrough, `Cell.null` on a
`UnionArray`, and union export are all unreachable. The flattened column is `Utf8View`, which
`Kind::from_arrow` already reads as `Kind::Str`, correctly.

### Still open: the `?` operator does not parse

```
SELECT doc ? 'a'  →  ParserError: Expected: end of statement, found: ?
```

sqlparser reads `?` as a placeholder before the crate's `ExprPlanner` ever sees it. `json_contains`
is the working spelling. Small, and cosmetic next to the above — but it means the README's third
operator is unavailable here and should be documented as such rather than left to be discovered.

## The consequence behind it: `json_get` returns a Union

`json_get` (and therefore `->`) returns arrow's `JsonUnion` — a **`DataType::Union`**. That is the
same type WJ-02 rejects for the reader, arriving from the other direction, and the same constraint
applies: **Parquet has no union logical type**, so `COPY (SELECT content->'type' …) TO 'x.parquet'`
cannot be written.

Traced through every surface before assuming a break. Most of it already works:

- **The grid renders it.** Cells go through arrow's `ArrayFormatter` (`engine/query.rs`), which has
  a `DataType::Union` arm (arrow-cast display.rs:563). So `SELECT content->'type'` returns rows
  rather than failing the run. It renders in arrow's union form (`{str=foo}`), which is *legible
  but ugly* — the argument for steering at `->>`, not a defect to fix here.
- **Copy already handles it.** `serialize::is_nested` lists `Union(..)` beside Struct/List/Map, so
  the CSV/TSV/Markdown writers stringify it to compact JSON.

Three that *would* have needed a decision, all now unreachable — see the fix above:

- **`Kind::from_arrow`** (`strata-model/src/schema.rs`) has no Union arm and matches on the arrow
  type's *display string*, so `Union(...)` falls through the final `else` to `Kind::Str`. Reached
  via the single `catalog::column_info` builder, which serves query results as well as the catalog
  (`query.rs:366`). Not obviously wrong — the cell *is* rendered as text — but it is a fallthrough
  rather than a decision, and `dtype` still reads `Union(...)` in the inspector beside a string dot.
- **Nulls.** `Cell.null` comes from `cols[ci].is_null(r)`, and a `UnionArray` has no top-level null
  buffer, so that is always `false`. A JSON null therefore renders as arrow's union display of the
  null variant rather than the configured NULL text, and the grid will not dim it.
- **Export.** Parquet cannot hold a union at all. Decide between refusing with a message that names
  `->>` as the fix, or letting DataFusion's own error through. Prefer the former — but note this
  one is loud on its own, so it is a message-quality call, not the correctness case that
  AGENTS.md's "silent corruption is refused" rule is about.

`->>` / `json_as_text` / `json_get_str` return Utf8 and have none of these properties. Every one of
the three above is an argument for making those the documented path rather than for reshaping the
union handling.

## State of play

Done. Registered, reaching the function catalogue, and the union/parquet incompatibility is
handled at the snapshot boundary. The `?` operator is unavailable (sqlparser); `json_contains` is
the spelling that works.

## Acceptance
- `SELECT json_get_str('{"a":"x"}', 'a')` returns `x` with no table registered. ✅
- The `json_*` names appear in autocomplete with signatures, sourced from the registry snapshot. ✅
- The validator accepts `->` and `->>`. ✅
- **A `Union` result column never reaches the parquet snapshot writer.** ✅ — flattened to its
  canonical JSON text, one test per union arm; a nested one is refused by name.
- The `?` operator is documented as unavailable (`json_contains` is the working spelling). ✅
- `cargo test -p strata-core`.
