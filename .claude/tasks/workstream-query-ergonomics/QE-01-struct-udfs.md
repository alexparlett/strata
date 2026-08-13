# QE-01 · Struct UDFs: `struct_keys`, `struct_entries`, `struct_get`, and the `to_json` fallback

**Workstream:** Query ergonomics · **Status:** ⬜ · **Depends on:** nothing

## Goal

Four scalar UDFs that make an object-keyed Struct enumerable and walkable, **Arrow-side
first, JSON text only as the fallback** (corrected in planning review — the first draft was
JSON-only, paying a serialize-and-reparse round trip the common case never needs):

- `struct_keys(s) → List<Utf8>` — the keys **this row** has, read off the struct's null
  bitmaps. Total: works on any struct, costs no serialisation.
- `struct_entries(s) → List<Struct{key: Utf8, value: V}>` — key/value pairs with the value
  still typed Arrow, offered when the struct's field types unify to one `V`. That is exactly
  the UUID-keyed map-as-object case (same-shaped values by construction), so
  `unnest(struct_entries(cb))` walks the whole map typed end to end — no JSON re-parse per
  access, and downstream field access stays ordinary struct access.
- `struct_get(s, key) → V` — access by a **computed** key, same unification rule as
  `struct_entries`. DF's `get_field`/dot access take only a literal key, and a computed key
  (a UUID read from elsewhere in the row) is what drove the feedback's walk; without this,
  dynamic access is `unnest(struct_entries(…))` plus a filter, or the JSON round trip.
  Unknown key at runtime → null (the JSON accessors' answer, not an error).
- `to_json(x) → Utf8` — serialise any Struct / List / Map / scalar subtree to JSON text.
  The escape hatch the two `V`-typed functions refuse toward (a heterogeneous struct has no
  single Arrow return type), feedback item 2's direct ask, and — being plain Utf8 with no
  `arrow.json` extension metadata — the working spelling where a value must unify in a
  recursive CTE (ledger item 4).

Together these close feedback items 1 and 2, in the form item 1 itself proposed ("a
keys/entries function would fix it").

**Alternative under evaluation — `datafusion-contrib/datafusion-variant`** (on Alex's radar;
surveyed 2026-08-13): 0.1.0 pins **exactly our stack** (datafusion 54, arrow/parquet-variant
58.3) and ships `cast_to_variant`, `variant_object_keys`, `variant_get`, `variant_to_json`
(plus construct/insert/delete and `json_to_variant`). Over the hand-rolled plan it has one
strict win — Variant carries per-row structure, so **no shape-unification requirement**: a
heterogeneous struct gets dynamic access instead of a refusal toward `to_json`. It is the
Spark/Iceberg-blessed shape for exactly this pathology. **Build step 0 below is the spike**;
adopt if it passes, hand-roll otherwise. What the spike must verify from source and fixture,
not assume: (a) maturity — its own README says pre-stable until its tracking issue closes;
(b) `variant_get`'s path argument accepts a **computed** key (a literal-only path kills the
headline case); (c) `cast_to_variant` survives the fixture's deep Structs, and what it does
to `JSON_TEXT_KEY` conflict columns (JSON text would embed as a string — `json_to_variant`
re-parses, but no raw-text column exists at scan time); (d) the result side — a projected
Variant column reaches `serialize`/`value_tree`/the inspector as opaque binary, so either
those learn a variant arm or the guidance is "wrap in `variant_to_json` before projecting";
(e) cost — `struct_keys` off null bitmaps is cheaper than constructing variant binary when
the question is keys-only; measure on the 19,311-key struct. Under adoption the deliverable
shrinks to registration + whatever the spike found missing (likely `struct_keys` kept for
the cheap keys-only read, and thin naming decisions); the refusal-toward-`to_json` design
below applies only to the hand-rolled path.

**Considered and not built** (survey 2026-08-13, so the next reader doesn't re-shop the
list): the array/list family (already in DF 54's `datafusion-functions-nested`),
`arrow_typeof` (core DF), a Struct→Map cast or map_* equivalents (isomorphic to
`struct_entries`, but Arrow `Map` is DF's sparsest function territory — List+Struct is the
better-supported target), jq-style deep descent (no demonstrated need; QE-03's `matching`
answers it schema-side, `to_json` + the json family value-side), and union accessors
(`json_poly`'s conflict state is already text). The rule: a UDF here traces to a
demonstrated field need — registration is one line, so there is no economy in speculating.

## Current state (verified 2026-08-13)

- `datafusion_functions_json::register_all` runs in `build_context`
  (`crates/strata-core/src/engine/mod.rs:1985`); the crate's `json_from_scalar` **rejects
  Struct and List input** (`plan_err!("Unsupported type for json_from_scalar…")`), which is
  the refusal the feedback hit. Its `json_object_keys`/`json_get` work on JSON text only.
- `engine::json_poly` infers every JSON object as a `Struct`
  (`json_poly/infer.rs:247-258`); keys are unioned across the file, so **an absent key is a
  null field in that row** — which is what makes the null-bitmap reading of `struct_keys`
  the honest per-row answer. Caveat shared by all three functions: a source-level explicit
  `null` is indistinguishable from an absent key (both are Arrow nulls); state it in each
  function's description once, identically.
- Strata registers no custom UDFs today. The pattern is settled: implement `ScalarUDFImpl`,
  one `register_udf` beside `mod.rs:1985`; `functions::snapshot` walks the registry so
  completion, signature detail, the docs panel and the agent's `list_functions` follow with
  no further wiring (docs/reference/ENGINE.md). The built-in fence (`registered_function`,
  all five registries) protects the new names from `CREATE`/`DROP FUNCTION` automatically.
- Return-type derivation: `struct_entries` computes its return type from the input
  `DataType` at planning time — `List<Struct{key: Utf8, value: V}>` when all field types
  unify to `V`, otherwise a **planning-time** `plan_err` naming `to_json` as the fallback
  (never a runtime surprise). `struct_keys` is `List<Utf8>` unconditionally.
- Name check: none of the four collides with DF 54 built-ins or the JSON crate's set
  (`map_keys`/`map_values` exist but take Arrow `Map`, which `json_poly` never produces).

## Build

0. **The `datafusion-variant` spike** (half a day, decides the rest): add the dep in a
   scratch branch, register its UDFs, run the fixture chain — enumerate `contentBlocks`
   keys, access by a key computed from another column, serialize a subtree — and answer
   (a)–(e) above. Record the verdict and the evidence in this file either way; on adoption,
   steps 1–3 shrink accordingly and the dependency note goes in the workstream README.
1. New module `crates/strata-core/src/engine/udfs.rs` (Strata's own built-ins — QE-02 joins
   it) holding all four `ScalarUDFImpl`s.
   - `struct_keys`: per row, the field names whose child is valid at that index — null
     bitmap walk, no values touched. Null struct row → null (not empty list: "no object"
     and "object with no keys" are different answers).
   - `struct_entries`: same validity walk, values taken by zero-copy slice/take from the
     child arrays into the unified `V`. Field types that differ only by nullability unify;
     anything else refuses at plan time.
   - `struct_get`: the unification check and refusal are `struct_entries`' — one shared
     helper, one wording. Per row: resolve the key against the field list (exact name, the
     file's own spelling — folding is a SQL-identifier concern, not a data concern), take
     that child's value; no such field or null struct → null.
   - `to_json`: null in → null out. Candidate implementation: arrow-json's writer with
     `explicit_nulls` unset so absent keys are omitted (verify), or `serde_json` over the
     array — pick whichever survives the fixture's shapes. `json_poly`'s conflict-text
     columns (`JSON_TEXT_KEY`) hold text that is already JSON and must pass through
     verbatim, not re-quoted — check `json_poly/normalize.rs` for the marker.
2. Register all four in `build_context` beside the JSON crate's `register_all`, same
   warn-not-fatal shape; include DataFusion `Documentation` on each (the registry walk
   reads `documentation()`).
3. Tests in the module: per-row keys against a fixture where rows have different key
   subsets; entries typed end-to-end through `unnest` (the unnest-outer-ref limit, ledger
   item 8, may force a subquery projection — the test records the working spelling);
   `struct_get` with a key computed from another column, an unknown key (null, not error),
   and a mixed-case key matched exactly; the plan-time refusal on a heterogeneous struct
   from both `struct_entries` and `struct_get`, one wording, naming `to_json`;
   `to_json` on struct/list/scalar/null and a `JSON_TEXT_KEY` column passing through
   unquoted-again; `to_json` unifying against plain Utf8 in a recursive CTE (the ledger-4
   shape); one end-to-end over a `json_poly`-registered `sample/config.json` slice proving
   the feedback's job — enumerate `contentBlocks` keys and reach each entry's fields —
   in pure SQL with no JSON text in the plan.

## Acceptance

- Key enumeration over the fixture's UUID-keyed struct is `struct_keys`/`struct_entries` —
  no serialisation in the plan; the values keep their Arrow types through `unnest`.
- A heterogeneous struct refuses `struct_entries` at planning time, names `to_json`, and
  the `to_json` chain works where it lands.
- `to_json`'s output unifies in a recursive CTE — one test proves it.
- Dynamic access by a computed key is one `struct_get` call — no unnest-and-filter, no JSON
  in the plan.
- All four appear in completion and `list_functions` with descriptions, registry-walk code
  unedited.
- Full check green (`clippy` + tests; container tests untouched).

## Files

`crates/strata-core/src/engine/udfs.rs` (new) · `crates/strata-core/src/engine/mod.rs`
(register + module decl) · `docs/reference/ENGINE.md` (one line: Strata now has own built-ins,
where they live) · tests beside the module + one fixture test in `strata-core`.
