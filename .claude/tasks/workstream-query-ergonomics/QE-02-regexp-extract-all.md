# QE-02 · `regexp_extract_all` UDF

**Workstream:** Query ergonomics · **Status:** ✅ · **Depends on:** nothing (shares the module
QE-01 creates; whichever lands first creates `engine/udfs.rs` — QE-01 did)

## Goal

`regexp_extract_all(string, pattern[, group]) → List<Utf8>`: every match in the string, not
just the first — DuckDB's spelling and semantics. Feedback item 6: DataFusion 54's
`regexp_match` returns only the first match's capture groups, and the absence of a global
variant is what forced a recursive-CTE walk over JsonLogic strings in the field. With this,
multi-match extraction is `unnest(regexp_extract_all(col, pattern))`.

## Current state (verified 2026-08-13)

- DF 54's regexp family (via `with_default_features` in `build_context`): `regexp_like`,
  `regexp_match` (first match only), `regexp_replace`, `regexp_count`. No extract-all.
- Registration pattern and its free consequences (completion, `list_functions`, the
  `DROP FUNCTION` fence): as QE-01's Current state — one `register_udf` in `build_context`.

## Build

1. `RegexpExtractAll` in `crates/strata-engine/src/udfs.rs`, `ScalarUDFImpl`.
   Semantics (DuckDB's): two or three args — string, pattern, optional group index
   (default 0 = the whole match); returns the list of that group's text for every
   non-overlapping match, empty list for no match, null in → null out. Invalid pattern is a
   plan/exec error naming the pattern, not a panic.
2. Use the `regex` crate (already in the tree transitively; add the explicit dep to
   `strata-core` if the manifest lacks it). Compile the pattern **once when it is a scalar
   literal** (overwhelmingly the real case) and per distinct value otherwise — look at how
   DF's own `regexp_*` implementations cache compilation and match that shape rather than
   inventing one.
3. Register beside QE-01's `to_json`; include `Documentation`.
4. Tests: multiple matches, group index, no match (empty list, not null), null input,
   invalid pattern error, and one `unnest(regexp_extract_all(…))` end-to-end.

## Acceptance

- Multi-match extraction per row is one expression + `unnest`; the recursive-CTE workaround
  is dead.
- Appears in completion/`list_functions` with signature and description, no wiring edits.
- Full check green.

## Files

`crates/strata-engine/src/udfs.rs` · `crates/strata-engine/src/mod.rs` (register) ·
`crates/strata-core/Cargo.toml` (only if `regex` needs declaring) · tests beside the module.

## Built (2026-08-13)

`RegexpExtractAll` in `engine::udfs`, the fifth entry in the `register` array QE-01 created —
`engine/mod.rs` was not touched, since QE-01's one `udfs::register(&ctx)` call is already the
whole integration. `regex = "1"` declared in `strata-core/Cargo.toml` (already in the graph and
feature-unified: DataFusion's own regexp family turns it on through its default
`regex_expressions`).

Decisions worth not re-litigating:

- **The compile caching is DataFusion's own two functions, reused, not a third.**
  `datafusion::functions::regex::{compile_regex, compile_and_cache_regex}` are `pub`;
  `regexp_count_inner` is the shape they are used in. A one-element pattern (the literal case)
  is compiled **before** the row loop; a pattern column goes through the cache, keyed by the
  pattern text. The reuse is also why a mistyped pattern reads identically here and in
  `regexp_count` — the wording is DataFusion's.
- **A literal argument stays one element** (`literal_or_column` + `cell`) rather than
  `to_array(number_rows)`, which would copy the pattern string once per row to say the same
  thing.
- **No flags argument**, deliberately, though DuckDB has one: the `regex` crate takes them
  inline (`(?i)foo`), so a fourth argument would be a second way to say one thing. DataFusion's
  family only has one because Postgres does. Stated in the type's own docs.
- **The list item is nullable and the null means "this group took no part in this match"**
  (`(a)|(b)` against `ab`), never "no match" — the list stays one element per match. An empty
  string is a match a pattern can genuinely make, so the two cannot share an answer. Group 0
  never produces one, but the group may be a column, so the return type cannot promise that.
- **A group the pattern does not have is refused by name**, with the pattern and the count it
  does have — a silent null there would read as "no match".
- The string argument keeps its own Arrow type (`Utf8` / `LargeUtf8` / `Utf8View`, one generic
  body over `StringArrayType`); the return is `List<Utf8>` as specified.

Tests are seven, beside the module, over a `json_poly`-registered fixture rather than
`named_struct` literals (QE-01's rule): every match with no-match and null-input in the same
query, the group index and a non-participating group, the out-of-range group refusal, an
invalid pattern, a pattern **column** (the cache path), and the `unnest` walk end to end. List
contents are asserted through `to_json`, which renders `[]` and `NULL` distinguishably. The
catalog test grew to five names.
