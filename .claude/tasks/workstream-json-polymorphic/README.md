# Workstream — Polymorphic JSON (WJ)

Reading JSON whose fields **disagree across records** — a type-discriminated union — and then
being able to query inside the result.

Two halves, deliberately split, because they are independently valuable and wildly different in
size. **01** makes JSON text queryable and is a dependency line plus a registration call. **02**
makes the union-typed file *register at all*, and owns a `FileFormat` / `FileSource` /
`FileOpener` from here on.

## The failing case

`config.json` has a field (`content`) that is `["...", true]` in some records and `{...}` in
others. Registration fails at schema inference:

```
Failed to infer schema: Arrow error: Json error:
Expected object json type, found: Array(Scalar({Utf8, Boolean}))
```

Two things this is **not**, both checked against arrow-json 58.3 source before building anything:

- **Not arrow merging same-named fields at different depths.** `collect_field_types_from_object`
  (reader/schema.rs) recurses into a *separate* inner `HashMap` per object, and `InferredType::merge`
  merges two `Object`s key-by-key within one level. Two fields called `content` at different depths
  never meet. The conflict is one key at one level.
- **Not a read-shape problem** (`JsonShape::Array` vs newline-delimited). `json_shape_error` already
  translates that one, and it is a different arm entirely.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| 01 | Postgres-style JSON accessors over Utf8 columns | ✅ | — | — |
| 02 | `PolyJsonFormat` — union-tolerant JSON reader | ✅ | — | 01 (for the payoff, not to build) |
| 03 | Table Config silently caps JSON schema inference at 1000 | ✅ | — | 02 |
| 04 | Should the SQL parser default to the postgres dialect? | ✅ | — | 01 |

## Why the order

01 does not depend on 02, and shipping it alone is already useful — any JSON *already* landing in a
Utf8 column (a stringified nested column, a text field holding a document) becomes navigable. But
02 without 01 is half a feature: the reader's whole output for the polymorphic field is a Utf8
column of raw JSON, and without accessors the user can `LIKE` against it and nothing more. That is
the reasoning that put both in scope rather than 02 alone.

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core ✓]` logic in `strata-core`.
