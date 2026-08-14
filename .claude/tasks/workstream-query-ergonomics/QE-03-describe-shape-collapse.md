# QE-03 · `describe_table` shape collapse for keyed siblings

**Workstream:** Query ergonomics · **Status:** ✅ (built 2026-08-14) · **Depends on:** nothing

## Goal

When a struct's children are N same-shaped siblings under data-bearing keys (the UUID-keyed
map-as-object pattern), `describe_table` renders **one representative shape with the count and
a few example keys** instead of paging N identical subtrees. Feedback item 10 names this as
the real win over any page-size change: "contentBlocks.<uuid>.variants[].eligibilityRule
×2134 — that one view would have told me the whole structure immediately." The same collapse
applies to `matching` search results, which today return thousands of hits differing only in
the key segment, 25 to a page.

Framing, for the "why not just return the schema" question: `describe_table` **is** the Arrow
schema — but `json_poly` has no Map arm, so data keys become schema fields, and the schema
itself is the 241k-field pathology the budget machinery exists to survive. The collapse is
JSON Schema's `additionalProperties` said in the tool's own vocabulary: one value shape, keys
unconstrained, count stated — without switching output formats (Arrow types outrun JSON
Schema's, and the path/drill-down contract already works). The deeper alternative — inferring
map-like objects as Arrow `Map` so every surface shrinks — is recorded against in QE-01's
considered-and-not-built list (record-vs-map is an inference-time heuristic; DF's Map function
coverage is its sparsest).

## Current state (verified 2026-08-13 — the state this was planned against, kept as the starting
point; `bounded_forest` is `walk` now and `SCHEMA_DEPTH` is 5, see **As built** below)

- The whole algorithm is `crates/strata-agent/src/describe.rs` (872 lines incl. tests):
  `SCHEMA_PAGE` 50, `MATCH_PAGE` 25, `SCHEMA_BUDGET` 16,384 bytes, sampling ladder
  `bounded_forest` (`:219-233`) with width decay 15/7/3 and `children_total` elision counts.
  The reference fixture is 19 top-level columns / 241,425 nested fields / one struct of
  19,311 keys; "even depth 1 grazes the cap on UUID-named keys" (`:33-38`).
- The contract every consumer relies on: **"a describe answer with no counting fields in it
  is a complete answer"** (`describe.rs:10-11`, tool doc `tools.rs:1049-1055`). A collapsed
  node must therefore carry a counting field — the signal survives.
- The `path` parameter's contract: segments are "a name a previous answer showed, exactly as
  the file spells it" (`describe.rs:279-283`); paths render as JSON arrays because names may
  contain dots.
- **The collapse must live in `describe.rs`'s wire projection, not in `column_info`**
  (`catalog.rs:1122-1146`): the sidebar's `flatten_cols` keys expansion state by real path
  segments, and `value_tree` addresses by entry index off live Arrow arrays — a collapse done
  on `ColumnInfo` would desynchronise both. `describe.rs` is the only consumer that wants it.
- Performance guardrails already learned here: never `format!("{dt:?}")` a recursive
  `DataType` (18 ms per call on the fixture struct — `catalog.rs:1156-1173`), and never build
  a quarter-million-node render just to measure it (`plausibly_complete`, `describe.rs:236-248`).
- The assistant's @-mention context reuses `describe_result` (`chat_send.rs:227-249`), so it
  inherits the collapse for free.

## Build

1. **Shape detection** in `describe.rs`: group a node's children by structural shape —
   compare `ColumnInfo` subtrees on dtype structure (names ignored below the top), cheaply:
   a recursive hash or key built without Debug-formatting. A group of ≥ `COLLAPSE_MIN`
   (propose 8; constant with a doc comment) same-shaped siblings collapses. Mixed children
   (e.g. 3 distinct shapes across 2,000 keys) collapse per group, largest first.
2. **Rendered form**: one synthetic child whose name marks it as a collapse (the wire needs a
   spelling the model can't confuse with a real field — e.g. a `keys_total: N` +
   `key_examples: [first 3 real names]` pair on `ColumnWire`, name rendered as a placeholder
   like `<key>`). The representative subtree then renders at the ladder's full depth — the
   budget the collapse frees is exactly what buys the depth the feedback wanted. The tool
   description documents the placeholder and that `path` drill-down still takes **real** keys
   (any of `key_examples`, or a name from `matching`).
3. **Search**: `search()` streams document-order matches; collapse hits whose paths differ
   only in segments that sit under a collapsed group — report one match with the placeholder
   segment and a `matched_keys` count. Keep `matched_total` truthful (total hits, stated).
4. Update the tests that pin current behaviour (`describe.rs:551` sampled UUID struct,
   `:578` floor, `:783` match page, `:812` match paging) alongside; add: collapse on the
   2,000-key fixture yields the one-shape answer within budget and deeper than the ladder
   managed before; a struct with 5 distinct children does **not** collapse; drill-down by a
   real key through a collapsed level resolves.
5. `docs/AGENT_ACCESS_SPEC.md` §AA-07 gains the collapse paragraph; `assistant/system.md`'s
   bounded-schema note mentions the placeholder.

## Acceptance

- On the fixture, describing the 19,311-key struct answers one representative shape with
  count + examples, several levels deep, inside `SCHEMA_BUDGET` — the "whole structure
  immediately" view.
- `matching` over a collapsed region answers shapes-with-counts, not 25 UUID paths a page.
- The no-counting-fields-means-complete contract holds (collapsed nodes always carry counts).
- Existing consumers unaffected: sidebar and value tree untouched, `column_info` unedited.
- Full check green.

## Files

`crates/strata-agent/src/describe.rs` · `crates/strata-agent/src/wire.rs` (`ColumnWire` /
`MatchWire` fields) · `crates/strata-agent/src/tools.rs` (tool description) ·
`crates/strata-agent/src/assistant/system.md` · `docs/AGENT_ACCESS_SPEC.md`.

## As built (2026-08-14)

The plan held; six things it did not say are settled here, four of them corrections.

- **The collapse is a *cutting* strategy, not a projection.** It runs only after the complete
  rung fails, never before it — because "same shape" catches ordinary schemas too. Sixty
  `Utf8` columns are sixty same-shaped siblings, and collapsing a schema that fits would trade
  the names, which are the whole answer there, for a count. So: an answer that fits complete is
  never collapsed, and the "no counting fields means complete" contract is untouched by
  construction rather than by care.
- **And that rule reads over the *forest*, not over the page** — the correction a review caught
  before merge. The complete rung is measured on one page, so gating on it alone let a wide
  keyed set escape whenever its page happened to fit: 19,311 UUID keys holding a two-field
  record are ~150 nodes and ~8 KB on page 1, comfortably inside both bounds, so
  `describe_table(t, path=['contentBlocks'])` answered 50 UUID names, 387 pages deep, while
  `describe_table(t)` collapsed the same struct correctly — one struct, two answers, decided by
  which way the caller reached it. A forest that is both **paged and collapsible** now skips
  the complete rung outright: being paged already means the answer is cut, and a collapse
  available in it says more than any page of it does. A collapsible forest that fits in one
  page is still never collapsed, which is the cutting rule intact.
- **A leaf never joins a set**, for the same reason at the other scale: a leaf carries nothing
  but its name, so a shape standing for two hundred of them says less than the
  `children_total` that already elides them. A set is containers only.
- **The walk root collapses too, before it pages.** `describe_table(t, path=['contentBlocks'])`
  — the drill-down the feedback actually took — is the same pathology one level down, and
  collapsing the *page* of 50 would have made `keys_total` a fact about the page. So `slots`
  runs over the whole forest and paging is over slots from there, which is what makes one
  collapsed answer the whole answer rather than the first of 387.
- **Shape equality is checked, not hashed.** `ColumnInfo` already derives `PartialEq`, so
  `same_shape` is an exhaustive destructure comparing everything but the name; the hash
  (`shallow`, one level deep) only buckets the candidates. A digest that *decided* membership
  would be a collapse claiming keys are identical on the strength of 64 bits, and hashing a
  quarter-million fields per rung is exactly the cost this module exists to avoid.
- **The search splits a set in two halves, because only one of them is shared.** The keys vary,
  so each is still tested by name and answers **as itself** — a caller searching for a key
  wants that key back, spelled as the file spells it. Everything below is identical by
  construction, so it is searched once through the placeholder with a multiplier carried down
  (nested sets multiply). That makes a row and a hit different things: rows are what pages,
  `matched_total` is still every field matched. Both are on the wire and both are documented.
- **`SCHEMA_DEPTH` moved 3 → 5, guarded by a per-rung node cap** — the one place this went
  past the plan's "renders at the ladder's full depth". At 3 the answer stops at the List's
  synthetic `element` level, one short of `eligibilityRule`, so the Goal's own quoted view was
  not reachable. The cap makes the deeper rungs free rather than risky: a rung is built against
  `NODE_CAP` (= `SCHEMA_BUDGET / NODE_FLOOR`) and abandoned the moment it passes it, and past
  that many nodes a rendering cannot fit whatever its names are — so no rung is ever refused
  that would have fitted, only the *building* is bounded. That guard is a fix in its own right:
  the depth-3 rung could already build ~16k nodes on a 50-column window to measure it, which is
  this module's founding defect at a smaller size.
