# WJ-04 · Should the SQL parser default to the postgres dialect?

**Workstream:** JSON · **Status:** ⬜ (open question, not a decided build) · **Depends on:** WJ-01

## The question
`datafusion.sql_parser.dialect` defaults to `generic`. Under `postgres` the editor gains operators
it does not have today — measured, the JSON containment operator:

```
generic  →  ParserError: Expected: end of statement, found: ?
postgres →  true
duckdb   →  ParserError
```

The mechanism is precedence, not tokenizing: sqlparser produces `Token::Question` and maps it to
`BinaryOperator::Question` in every dialect, but `GenericDialect` overrides `get_next_precedence`
and omits it, so the parser stops before the operator is consulted
(sqlparser-0.62 `dialect/mod.rs:928` vs `dialect/postgresql.rs:139`).

Strata is Postgres-flavoured by intent — the whole WJ-01 accessor family is Postgres syntax
(`->`, `->>`, `json_get`) — so `generic` is arguably the wrong default for what this app is.

## Why it is not a one-line change

**`engine/sql/lex.rs` hardcodes `GenericDialect`** — at `:212` for the tokenizer and at `:21` for
`is_identifier_part`. Its module doc says the dialect "matches the engine". Change the engine's
dialect and that claim silently becomes false: the lexer behind autocomplete, highlighting and
identifier detection would be parsing by different rules than the planner. That is the split worth
avoiding, and it is the reason this is a task rather than a default flip.

So the change is at least:

1. `lex.rs` reads the configured dialect instead of hardcoding one (it already has the engine's
   config available at snapshot time — check how it is reached).
2. The dialect becomes an input to the language service, so a config change re-lexes.
3. Decide whether the *default* moves, or only the coupling is fixed.

## Note: nothing is blocked today
The key is **already catalogued** (`engine/config.rs:302`), so a user who wants `?` can set
`datafusion.sql_parser.dialect = postgres` in Settings ▸ Engine right now. `json_contains` is the
spelling that works in every dialect and is what the docs name. This task is about the default and
the lexer coupling, not about unblocking anyone.

## What to check before deciding
- What else changes between `generic` and `postgres` in sqlparser 0.62 — this was verified for one
  operator and one baseline query only. The other dialect methods (identifier rules, string escapes,
  geometric types, `supports_*` flags) each differ and were not surveyed.
- Whether any existing test or saved query parses under `generic` and not `postgres`.
- Whether the editor's completion/highlighting behaves once `lex.rs` follows the setting.

## Acceptance (if it proceeds)
- `engine/sql/lex.rs` no longer hardcodes a dialect, and its doc comment's claim is true again.
- Changing `datafusion.sql_parser.dialect` re-lexes the editor rather than desyncing it.
- A decision on the default is recorded here with the survey that backs it.
