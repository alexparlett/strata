# P2-24 · Record view — large nested cells

**Phase:** 2 — Workbench · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** P2-10, P2-12

## Goal
Opening the record view (or the nested-cell view) on a row with large nested columns takes one to
two seconds and freezes the window while it does. Make it open instantly regardless of how much
data the row holds.

## What is actually wrong

Found on `sample/config.json` — one row, 19 columns, all structs, 241,425 nested fields. Three
separate problems, and the serializer is the least of them.

**1. It runs inside `render`.**
`views/workbench/results/record_view.rs:238` calls `cell_pretty_json` in the component's `render`,
so the work happens on the render thread. That *is* the freeze. It also re-runs on every re-render
of the component, not only when the row changes.

**2. It runs for every column, unconditionally.**
The field list is a plain `for (ci, col) in self.data.columns.iter().enumerate()` with no
virtualization (`record_view.rs:215`). Opening the view on a `config.json` row serializes the whole
62MB document — `nba`, `contentVariants`, `pricing`, all 19 — whether the user looks at any of them
or not. `datagrid/row.rs:146` has the same call for the nested-cell view.

**3. Each call is three passes.**
`serialize::cell_pretty_json` goes arrow → JSON bytes (`ArrayWriter`) → `serde_json::Value` →
`to_string_pretty`, allocating a full `Value` tree in the middle.

And the punchline: every result lands in a **190px-tall scroll box** (the canvas's
`cellViewOpen` card). Tens of megabytes are materialized to fill something that shows about ten
lines.

## Build

### 1. Bound the serialization — the real fix
Give `cell_pretty_json` (and `row_pretty_json`) a byte or line budget with an explicit elision
marker, so a large struct renders as a **summary**:

```json
{
  "nbas": [ … 5171 items … ],
  "templateRules": { … 12 keys … }
}
```

This is faster *and* more readable than 30MB of pretty JSON nobody can scroll through. The 190px
box was drawn for `{"a":1,"b":2}` — it predates anyone having a cell this size, and honouring its
intent means summarising rather than truncating mid-token.

In `strata-core::engine::serialize`, so the budget and the elision wording are unit-testable
without a renderer.

### 2. Move it out of `render`
Memo the result on `(batch_row, ci)`. Switching rows then costs one pass; an unrelated re-render
costs nothing. Even bounded, per-cell serialization does not belong in a render pass.

### 3. Deliberately **not** optimising the pipeline
The `Value` round-trip in `cell_pretty_json` looks like the obvious target and is not worth
touching: once the output is bounded, the input never gets large enough for it to matter. Fixing it
first would be optimising the step that stops being hot.

## The full value stays reachable
Copy-row-as-JSON (`results/copy.rs`) and copy-cell already produce the complete value and are not
on the render path. The summary is a *view* concern; nothing about the data becomes unreachable.

## Later: a lazy tree
If inspecting nested values in documents like this becomes a real workflow, the right surface is a
collapsible tree — top-level keys, expand to descend, only the expanded path materialized. That
reads better than a text blob at any size, and it is how every JSON viewer handles big documents.
It is a real component build, and the bounded summary above gets most of the benefit first; do not
start here.

## Acceptance
- The record view opens on a `sample/config.json` row with no perceptible delay.
- The nested-cell view (double-click) is the same.
- Neither `cell_pretty_json` nor `row_pretty_json` is called from a `render` body.
- A large nested value renders as a summary naming what was elided (item/key counts), not a
  mid-token truncation.
- The summary budget and its wording are unit-tested in `strata-core`.
- `cargo test -p strata-core` and a clean build.
