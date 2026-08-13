# QE-02 · `regexp_extract_all` UDF

**Workstream:** Query ergonomics · **Status:** ⬜ · **Depends on:** nothing (shares the module
QE-01 creates; whichever lands first creates `engine/udfs.rs`)

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

1. `RegexpExtractAll` in `crates/strata-core/src/engine/udfs.rs`, `ScalarUDFImpl`.
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

`crates/strata-core/src/engine/udfs.rs` · `crates/strata-core/src/engine/mod.rs` (register) ·
`crates/strata-core/Cargo.toml` (only if `regex` needs declaring) · tests beside the module.
