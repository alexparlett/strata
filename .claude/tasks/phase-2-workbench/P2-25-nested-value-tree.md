# P2-25 · Nested-value tree (the cell view's inspector)

**Phase:** 2 — Workbench · **Status:** 🟡 · **DEV_TASKS:** — · **Depends on:** P2-12, P2-24

## Goal
Double-clicking a nested cell must let you **dig into the value** — expand a key, descend, read a
leaf — on a document-shaped row (`sample/config.json`: 19 struct columns, 241,425 nested fields, one
column with 19,311 keys under a single top-level key). A collapsible tree, materializing only the
paths that are open.

P2-24 made that surface *fast* and, on its own, useless: a bounded render is a dead end without
something to expand it. This is the other half, and P2-24's acceptance is not met until it lands.

## Why a tree and not folding in the code editor

Both were on the table. The editor route looked attractive because `strata-code-editor` already has
`read_only` as a first-class mode (`editor_ui.rs:116`) and a JSON grammar is one dep
(`EditorLanguage::new` takes a tree-sitter handle, so `tree-sitter-json` + one entry). It was
rejected on a **category mismatch**, not on cost:

> Folding is a view transform over text you *have*. Progressive rendering is about text you *don't
> have*.

IntelliJ folds a 62MB JSON file because the whole file is in its buffer. Our value is not text at
all — it is Arrow arrays, and turning it into text is precisely what we cannot afford (128MB for one
row, 1.29s). A "fold" whose content has never been materialized is not a fold; it is a placeholder
node with a lazy child list — a tree, wearing a text editor's clothes.

The asymmetry that settled it: **choosing the tree makes folding unnecessary** (the tree *is* the
navigation), while choosing folding still leaves you needing placeholder-folds, i.e. a tree anyway.

And the cost of folding is not small, because it breaks an invariant the editor relies on globally.
`editor_ui.rs:805` is `VirtualScrollView::new(|line_index, _| …).length(lines_len)` where
`lines_len = syntax_blocks.len()`, and `line_index` is used **directly** as the syntax line
(`get_line`), the selection key (`EditorLine::Paragraph`), the cursor-row compare, the gutter number
(`line_index + 1`) and the hover target (`update_hover`). Display row *is* buffer line, in five
places — one of them the pointer/hover stack AGENTS.md §8 flags as having broken diagnostics before.
Folding means threading a display↔buffer mapping through all of it.

## Settled decisions

- **Tree only. No `Text` tab** for now. A DataGrip-style `Tree | Text` value panel is the obvious
  end-state and the read-only editor makes it cheap, but it is a second surface to dress; revisit
  after the tree is in use.
- **Dress is derived, not designed** — no canvas covers a tree. Take row height, hover/selected
  fills and the chevron from the existing `sidebar_row` + `Table` vocabulary, and per-node dtype
  colour from `components::type_palette` (`kind_color`). A theme pass comes later; do not invent
  tokens on the consuming surface (AGENTS.md §3).
- **The record view keeps P2-24's sampled text.** It is a scan of nineteen fields at once, where
  sampled entries plus counts is the right read. The tree is for the cell view, which is where you
  go to inspect *one* value.

## What was built

### 1. `strata-core::engine::value_tree` — the read model
`cell_root` / `cell_children(path, skip, take)` / `cell_len`. A node is addressed by a path of
**entry indices** (not names: a duplicate or reordered key cannot mis-resolve, and a list has no
names), resolved with O(1) Arrow slices. Windows carry **absolute** indices, or a path built from a
second page would address the wrong entry.

Measured on `config.json`'s `contentBlocks` (19,311 keys):

| | |
|---|---|
| root node | 3.8 µs |
| 30 rows of 19,311 | 13 µs |
| the **last** 30 | 11 µs |
| descend into entry 12,345 | 9.6 µs |

The last-30 figure is the one that matters: same cost as the first 30, so it is O(window), not
O(position) — which is what makes a virtualized tree viable at all.

**No JSON**, which is the design and not an optimisation. A tree already carries the structure
JSON's braces exist to express, so encoding to text would be work done only to be re-parsed by the
eye — and it *loses* what the tree needs, since a leaf arrives quoted and a node's type lives in the
Arrow schema rather than anywhere in the JSON. Leaves are formatted by the same `ArrayFormatter` the
grid formats a cell with, and clipped by the same `util::clip`, so a value reads identically in the
grid, the record view and here.

Found while verifying it, and worth more than the feature: **`short_type` was `format!("{dt:?}")`**
with the first word taken off the front — but `DataType`'s `Debug` is *recursive*, so it rendered an
entire subtree as text to discard nearly all of it. One call on `contentBlocks` cost **18ms**, and
`column_info` makes it per field all the way down, so it was quadratic in the schema. Matching the
composite variants directly took the root node from 18ms to 3.8µs and **~19% off every query on that
file** (3.42s → 2.77s). That was never about the tree; it had been slowing catalog reads and every
result since the column model was written.

### 2. The fork: `Tree` + `TreeItem`
A fork addition beside `Table` (P4-07's precedent), virtualized over a **flat list of visible rows**
— which is what makes laziness expressible: a component owning a node hierarchy would have to be
handed every node up front. Selection and keyboard stay with the caller, as they do for `Table`.

**Horizontal scrolling needed no fork change.** `VirtualScrollView` already offsets its content by
the cross-axis scroll position, measures its horizontal scrollbar from the content size, and applies
X wheel delta; `ScrollView` uses the identical structure and its own comment says the content box is
*fill-sized to the viewport, its offset scrolls its children, not itself*. The only thing missing was
a row that **hugs** (`Size::Inner`) instead of clamping itself to the viewport with `fill` — a
`fill` row means the content never exceeds its box and there is nothing to overflow.

Three wrong turns are recorded because each cost a build and none is obvious:

1. **Adding a `cross_axis_scroll` flag that sized the content rect explicitly.** `is_scrollbar_visible`
   compares `inner_sizes` against the content rect's *own area*; giving that rect an explicit width
   makes `area == inner_sizes`, so the overflow test finds nothing. It widened the very box whose
   narrowness is what makes scrolling happen.
2. **`min_width(Size::percent(100.))` to keep short rows full width.** torin *adds* a percentage
   minimum to a hugged width rather than flooring with it — a torin test put a 900px row at 1400 and
   a 50px row at 550, each inflated by exactly one viewport.
3. **Hoping `Fill` children would resolve against a hugged parent.** They resolve against the space
   *available* to the parent (torin test: parent hugs to 800, `Fill` child stays 500), so "content
   hugs, rows fill it" is not expressible.

The row therefore just hugs, and the consequence is accepted rather than worked around: a short
row's hover fill stops at its content, so Strata's `tree` theme sets those fills transparent.

### 3. The app
`results::value_tree` is the tree model — the expanded-path set, the flat row projection, and
`PAGE`-at-a-time widening with a `… N more` tail row. Closing a node **forgets how far it was
paged**: reopening a container you had scrolled deep into should not still be thousands of rows long.

`cell_view`'s `Readout` blob is replaced by the tree. `CellValue` now carries the **`RecordBatch`**
rather than rendered text, which keeps P2-12's snapshot rule rather than breaking it: the arrays the
modal reads are the ones it opened with, so a later filter or page flip still cannot retarget it, and
a batch clone is an `Arc` bump per column. Expansion is written back **through the open slot** rather
than a state beside it — one answer to "what is open", disposed of when the modal closes.

## What is not done
- **Keyboard navigation.** `Table` has none either, so the component matches its neighbour; per the
  no-command-bus rule it belongs in the app as an `on_global_key_down`.
- **Per-node copy.** P2-11 owns the clipboard capability; it routes through `results::copy` when it
  lands rather than growing local wiring (AGENTS.md §5).
- **A resizable modal.** Attempted and removed at Alex's direction — the grip drew but never received
  a press, and it is his to do properly in a later task.
- **The record view still uses sampled text.** Whether it should read Arrow directly like the tree
  does is a live question: it currently formats leaves through arrow-json while the tree and grid use
  `ArrayFormatter`, so a timestamp is quoted in one and not the other. One value, two descriptions.

## Open questions
- Should the tree replace the record view's field blocks too? See above — it is the same question as
  the record view's leaf formatting.

## Acceptance
- ✅ Double-clicking `config.json`'s `contentBlocks` cell opens instantly and lets you expand a UUID
  key, then `content`, then read the leaves.
- ✅ Expanding a node materializes only that node: the last 30 of 19,311 costs what the first 30 does.
- ✅ 19,311 sibling keys scroll smoothly (virtualized), and a long value pans horizontally.
- ⬜ Keyboard navigation works without the mouse.
- ✅ The `Tree` component lives in the fork, is themed, has an example, and the fork commit is
  **pushed** (`0dcf2c36`).
- ✅ `cargo test --workspace --locked` green on macOS; `schema_in_sync` regenerated for the `tree`
  theme group.
