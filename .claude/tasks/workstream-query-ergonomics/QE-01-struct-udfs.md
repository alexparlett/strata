# QE-01 · Struct UDFs: `struct_keys`, `struct_entries`, `struct_get`, and the `to_json` fallback

**Workstream:** Query ergonomics · **Status:** ✅ · **Depends on:** nothing

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

**Alternative evaluated and REJECTED — `datafusion-contrib/datafusion-variant`** (spike run
2026-08-13, evidence below). The hand-rolled path stands; everything after this section is the
deliverable.

The survey that put it here was read off the **repository's** manifest, not the published
crate, and the two are two DataFusion majors apart:

| | published 0.1.0 | git HEAD `9e1c846` |
|---|---|---|
| datafusion / arrow | **52.1** / **57** | 54 / 58.3 |
| `variant_object_keys` | **absent** | present |

So `cargo add datafusion-variant` resolves a **second** DataFusion (52.5.0) and arrow 57.3.1
into the graph beside our 54 / 58.3 — the UDFs are `ScalarUDFImpl`s of another DataFusion and
cannot be registered on our `SessionContext` at all — and the release has no key-enumeration
function, which is the headline ask. Only the unreleased git HEAD builds against our pin, and
its own README says the crate is pre-stable until its tracking issue closes. The spike ran
against that HEAD as a dev-dependency (single copy of datafusion/arrow confirmed by
`cargo tree -d`) over a `json_poly`-inferred fixture of `config.json`'s shape.

What it answered:

- **(a) maturity** — adoption means a git-rev dependency on unpublished, self-declared
  pre-stable code, inside the four-crate lockstep set that already pins DataFusion 54.
- **(b) computed key — WORKS, and it is a real capability we do not have.**
  `variant_get(v, pick)` with `pick` a *column* returned the right value per row. Its
  columnar-path branch builds a one-row array per row and `concat`s them, so it is not cheap,
  but it is correct.
- **(c) `cast_to_variant` survives the fixture**, and per-row keys are right —
  `variant_object_keys` gave `[a1, b2]` / `[c3]` over a struct whose three fields are unioned
  across the two records, i.e. **no shape-unification requirement**, the one strict win.
  `JSON_TEXT_KEY` columns embed as **strings**, as feared: `note` came back
  `"\"plain prose\""` / `"{\"kind\":\"body\"}"`, so a conflict-state column stays opaque
  inside the variant unless the user knows to write `json_to_variant`.
- **(d) the result side — this is what decided it.** A projected variant column is
  `Struct{metadata: BinaryView, value: BinaryView}` with `ARROW:extension:name =
  arrow.parquet.variant`. Measured against our own readers: `column_info` says
  `dtype=Struct kind=Struct`, `cell_preview_json` prints the two hex blobs, `value_tree`
  shows a 2-child nest, CSV export writes the hex. The grid, the inspector and export would
  each need a variant arm, or every query has to end in `variant_to_json`.
- **(e) cost** — keys-only over a 5,000-key struct: **58.7 ms** for
  `variant_object_keys(cast_to_variant(cb))` against **19.75 µs** for the null-bitmap walk.
  The real document has 19,311 keys.

Also settled, so it is not re-argued: variant would **not** have recovered "explicit null vs
absent key". `cast_to_variant` drops the null fields too — the distinction is lost at
`json_poly` inference, before either implementation sees the data.

**Revisit when** the crate publishes a release on our DataFusion pin *and* the result-side
surfaces (`column_info` / `serialize` / `value_tree` / export / the snapshot's extension
metadata) have somewhere to put a Variant. Its dynamic access to a *heterogeneous* struct is
the thing worth coming back for.

**Learnings taken into the hand-rolled path** (module docs carry the same, at the site):

- **A key, not a path.** Its `path_from_scalar` carries a `List`-of-strings overload
  specifically "for keys that contain dots such as OTEL attribute keys like
  `http.response.status_code`" — dot-path parsing broke on real keys. `struct_get` takes one
  key, matched exactly, so that failure is unreachable.
- **Gather with `interleave`, never a per-row loop.** Its per-row singleton-and-`concat` is a
  large part of the 58.7 ms.
- **Validate off the `Field`, not the `DataType`** (`try_field_as_variant_array` reads the
  field's extension type). Same reason `to_json` reads `arg_fields[0].metadata()` for
  `JSON_TEXT_KEY`: the meaning is on the field.
- **Recorded, not built:** `variant_get`'s optional third argument, a type hint parsed by
  `DataType::from_str` (`variant_get(v, path, 'Int64')`) — how it gets typed access out of a
  heterogeneous object. Rejected here because it introduces Arrow type spellings as a second
  vocabulary beside `short_type` / `Engine::column_type`, and because the refusal toward
  `to_json` is this task's stated acceptance. It is the obvious first move if the
  heterogeneous case turns out to matter more than the feedback suggested.

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

0. ~~**The `datafusion-variant` spike**~~ — **done, rejected**; verdict and evidence above.
   The dev-dependency and the spike test are removed; nothing in the tree references the crate.
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
