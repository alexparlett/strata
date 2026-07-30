# P2-24 · Record view — large nested cells

**Phase:** 2 — Workbench · **Status:** 🟡 · **DEV_TASKS:** — · **Depends on:** P2-10, P2-12

## Goal
Opening the record view (or the nested-cell view) on a row with large nested columns took one to
two seconds and froze the window while it did. Make it open instantly regardless of how much data
the row holds.

## What was actually wrong

Found on `sample/config.json` — one row, 19 columns, all structs, 241,425 nested fields. Three
separate problems, and the serializer is the least of them.

**1. It ran inside `render`.**
`views/workbench/results/record_view.rs` called `cell_pretty_json` in the component's `render`, so
the work happened on the render thread. That *was* the freeze. It also re-ran on every re-render of
the component, not only when the row changed.

**2. It ran for every column, unconditionally.**
The field list is a plain `for (ci, col) in self.data.columns.iter().enumerate()` with no
virtualization. Opening the view on a `config.json` row serialized the whole 62MB document — `nba`,
`contentVariants`, `pricing`, all 19 — whether the user looked at any of them or not.
`datagrid/row.rs` had the same call for the nested-cell view; not in a `render`, but in a press
handler, which is the same thread and froze just as hard.

**3. Each call was three passes.**
`serialize::cell_pretty_json` went arrow → JSON bytes (`ArrayWriter`) → `serde_json::Value` →
`to_string_pretty`, allocating a full `Value` tree in the middle.

And the punchline: every result lands in a **190px-tall scroll box** (the canvas's `cellViewOpen`
card). Tens of megabytes were materialized to fill something that shows about ten lines.

## What was built

### 1. The budget is enforced *during* encoding — the correction to this task's own plan
The original plan said to bound the output and explicitly **not** touch the pipeline, on the
grounds that "once the output is bounded, the input never gets large enough for it to matter."
That premise is false for the shape that motivated the task: the input to `cell_pretty_json` is one
cell, and one cell here *is* 30MB. Bounding after the fact leaves the whole
arrow-json + `serde_json::Value` materialization in place and still costs about a second, so it
fails the acceptance. §3's reasoning is only sound if the bound is applied at the encoder — which
means replacing the pipeline, not preserving it.

So `strata_core::engine::serialize::cell_preview_json` (renamed from `cell_pretty_json`, which had
no other caller) **walks the Arrow arrays** and never materializes the value:

- Nested containers (`Struct`, the five `List` flavours, `Map`) are descended by hand. Narrowing a
  list to "the items at this index" is an O(1) Arrow slice, so a 5171-item list costs nothing to
  *measure* and only its rendered items to show.
- **Leaves are encoded by arrow-json's own `make_encoder`** (public API, `NullableEncoder::encode`
  writes one value at one index into a `Vec<u8>`). So a number, decimal or timestamp reads exactly
  as the copy path renders it, with no second rendering of our own to drift. Strings and binaries
  are the one leaf that can be arbitrarily large, so they are clipped *before* encoding — encoding
  one to measure it would be the cost the budget exists to avoid.
- The **output's shape is the whole design**, and it took three rejected versions to get right. It
  is one fixed depth, with each container's entries **sampled** and the rest counted:

  ```json
  {
    "contentBlocks": {
      "0004d823-2c30-42b6-b28d-4a960fc2f03c": {
        "content": { … 2 keys … },
        "name": "lozenge - exclusive to you"
      },
      … 19296 more keys
    }
  }
  ```

  Each rejected version, because none of these is obvious in advance:

  1. **Collapsing the container, not its values.** The first cut rendered a container that would not
     fit as `{ … 19311 keys … }`. On a real document that is a *dead end*: `config.json`'s
     `contentBlocks` column has one top-level key, so the whole 62MB rendered as two lines. The level
     the reader is looking at is exactly the one thrown away. IntelliJ folds a **value** and still
     lists its parent's keys, and a preview has to make the same call — hence sampling entries.
  2. **Maximising depth.** Iterative deepening (keep the deepest uniform level that fits) sounds
     thrifty and is backwards: on a wide document the deepest level that fits is a *narrow* one, so
     the preview walked five levels down the first branch and never showed the second key. The depth
     is now **fixed** at `PREVIEW_DEPTH` and the budget is only a backstop — a target to stay under,
     never one to fill.
  3. **A flat per-container cap.** One number either wastes the budget deep or shows three entries at
     the level you scan. `items_at` halves it per level from `PREVIEW_ITEMS` down to
     `PREVIEW_ITEMS_MIN`, so breadth goes where a reader looks and the floor guarantees you always
     land on content rather than a count.

- An **empty container is its own summary**: the emptiness test comes before the depth test, or
  `{ … 0 keys … }` stands where `{}` belongs.
- **Depth 0 ignores the budget.** It is one count marker or one clipped scalar either way, and a
  budget smaller than that should still leave something to read rather than nothing.

Measured on the real `sample/config.json` row (release-opt test build), which is the whole
justification for the shape above:

| | time | bytes |
|---|---|---|
| all 19 columns, bounded (the record view's open cost) | **1.6 ms** | 22 KB |
| the same row, unbounded (`row_pretty_json`, still Copy's path) | **1.29 s** | 128 MB |

That 128MB figure is also why bounding *after* serializing was never going to work: the JSON of one
row of this file is twice the file.

One bug the real file found that no amount of design review would have — a column that is
**all-null infers as `DataType::Null`**, whose nulls are *logical*, so `Array::is_null` reports
false for every index and arrow-json's `NullEncoder::encode` is `unreachable!()`. arrow's own writer
never trips it because it tests nullity through the encoder it built; a walk that calls `encode`
directly has to do the same. Fixed by asking `NullableEncoder::is_null`, which covers every such
type rather than naming the one we hit. (Same lesson as `json_poly`: the normalization rules were
found by running the file.)

Three smaller decisions, each recorded because they are visible in the output:

- **Nulls are explicit** (`"plan": null`). arrow-json's default elides a null struct field, so
  today's cell view showed a null field as an *absent* one. `row_pretty_json` already opted into
  explicit nulls deliberately; a preview is for reading, and "missing" and "null" are different
  facts.
- **`row_pretty_json` stays unbounded.** The plan's parenthetical asked for a budget there too, but
  its only caller is Copy row as JSON, where the complete value is the entire point. A bounded twin
  would be an unreferenced helper, which AGENTS.md §5 rules out; if a *view* of a whole row ever
  appears, it gets one then.
- The clip marker is `…`, matching `query::truncate_cell`, which is the house style for a clipped
  **data** value (AGENTS.md §3's no-glyphs rule is about message register, and the grid's own
  display cells already read this way).

### 2. Out of the per-render path — and **not** through Freya's `use_memo`
`RecordView` owns a single-entry synchronous cache (`PreviewMemo`, keyed on the `Rc<GridData>` by
`Rc::ptr_eq` plus the batch row). Switching rows costs one pass over that row's nested columns;
every other reason the component re-renders (hover, theme, a clamp at the page edge) costs nothing.

`use_memo` was built first and rejected, for the reason `find.rs`'s `PageMemo` already records:
it settles **asynchronously**. `Runner::handle_events` returns the moment a scope is dirty and only
polls tasks once none is, so the render following a prev/next paints with the memo's *previous*
value — the wrong row's JSON, for a frame. A value derived during render needs a synchronous cache,
and this is now the second one in this module for the same reason. (It also sidesteps a hook-count
problem: the field list is a loop, so a memo per column is not expressible.)

The cache holds the `Rc<GridData>` it read, which is what makes `Rc::ptr_eq` a safe identity test —
no address can be reused while the cache is the thing keeping it alive.

The nested-cell view's press handler (`datagrid/row.rs`) needed no memo — it runs once per
double-click — but it did need the bounded read, since a press is on the same thread as a render.

### 3. No virtualization of the field list
It was on the table as fix 2 and turned out not to be needed: with each cell bounded to ~4KB, all
19 columns together are microseconds. A `VirtualScrollView` over the fields would be complexity
bought for nothing.

### 4. The `Value` round-trip in the copy path is deliberately untouched
`row_pretty_json` and `PrettyJsonWriter` still go through `serde_json::Value`. They are copy/export
paths, off the render thread and asked for the whole value, so the round-trip is not hot there.

## The full value stays reachable
Copy-row-as-JSON (`results/copy.rs`) and the grid's right-click Copy as JSON both produce the
complete value through `write_selection` / `row_pretty_json`, and neither is on the render path. The
summary is a *view* concern; nothing about the data became unreachable.

## What is NOT done: the CellView tree — and why this task cannot close alone
The bounded preview is acceptable in the **record view**, which is a scan of a row: sampled entries
plus counts is what you want from nineteen fields at once. It is **not** acceptable as the only way
to see a value, and a first attempt to ship it as such was rightly rejected — *any* bounded render
is a dead end without something to expand it. Stated plainly: **P2-24's own acceptance is
unsatisfiable as written.** "Opens instantly" and "the value stays inspectable in the view" cannot
both be met by bounding, however good the bounding is. The original plan deferred the tree
("do not start here") while depending on it, which is the contradiction that produced two lines of
output on a 62MB document.

So the double-click **cell view** gets a real lazy tree — see
[P2-25](P2-25-nested-value-tree.md). The walk built here is most of its engine: it already descends
one level at a time and can measure a container without reading it, so per-node expansion is the
same primitive with a path argument. The rejected alternative was folding in the code editor; that
task file carries the reasoning.

## Acceptance
- ✅ The record view opens on a `sample/config.json` row with no perceptible delay.
- 🟡 The nested-cell view (double-click) is the same speed, but its surface is wrong until P2-25 —
  a bounded blob is not an inspector.
- ✅ Neither serializer is called *per render* (`cell_pretty_json` no longer exists; the record view
  reads its cache, the grid a press handler). Read literally — "not from a `render` body" — the
  clause holds for `row_pretty_json` and is satisfied in spirit but not letter for the preview: the
  cache is consulted in `render` and computes on a miss, because the alternative (`use_memo`) paints
  the wrong row for a frame. See §2.
- ✅ A large nested value renders as sampled entries plus a count of the rest, not a bare container
  count and not a mid-token truncation.
- ✅ The summary budget and its wording are unit-tested in `strata-core`
  (`engine::serialize::tests` — the counts, the floor, the clip, the byte bound, empty containers,
  explicit nulls, maps).
- ✅ `cargo test -p strata-core` and a clean build.
