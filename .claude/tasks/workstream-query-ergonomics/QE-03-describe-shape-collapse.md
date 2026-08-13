# QE-03 · `describe_table` shape collapse for keyed siblings

**Workstream:** Query ergonomics · **Status:** ⬜ · **Depends on:** nothing

## Goal

When a struct's children are N same-shaped siblings under data-bearing keys (the UUID-keyed
map-as-object pattern), `describe_table` renders **one representative shape with the count and
a few example keys** instead of paging N identical subtrees. Feedback item 10 names this as
the real win over any page-size change: "contentBlocks.<uuid>.variants[].eligibilityRule
×2134 — that one view would have told me the whole structure immediately." The same collapse
applies to `matching` search results, which today return thousands of hits differing only in
the key segment, 25 to a page.

## Current state (verified 2026-08-13)

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
