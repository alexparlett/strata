# P2-25 · Nested-value tree (the cell view's inspector)

**Phase:** 2 — Workbench · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** P2-12, P2-24

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

## Build

### 1. `strata-core`: path-addressed materialization
The P2-24 walk already descends one level at a time and can measure a container without reading it
(narrowing a list to an index is an O(1) Arrow slice). What it lacks is a way to *start* somewhere:

- A path type (a struct field index / list index / map key index per step — index, not name, so a
  duplicate or reordered key cannot mis-resolve).
- `children_at(batch, col, row, path)` → the node's children as rows: key or index, a `Kind`, a
  bounded leaf rendering or a container's entry count, and whether the child is itself expandable.
- Keep the per-node cost bounded the way `cell_preview_json` is: sample with `items_at`, count the
  rest, and let the tree page a wide container (`… 19296 more keys` becomes a "show more" row).

Unit-testable without a renderer, like the rest of `serialize`.

### 2. The fork: a `Tree` component
There is no tree in `crates/freya/crates/freya-components/` — the closest is `accordion.rs` (a
single disclosure). So this is a **fork addition**, exactly the P4-07 `Table` precedent: build it
upstream-shaped rather than hand-rolling a lookalike in the app, and put whatever the component has
no opinion about (which node is selected, what a row renders) in the app.

- `VirtualScrollView`-backed over a flattened visible-row list, so 19,311 sibling keys cost what is
  on screen.
- Expanded-path state, chevron, indent guides, the IDE keyboard contract (↑/↓ move, → expand,
  ← collapse-or-parent, Home/End), a11y.
- Themed via `define_theme!` like every other component; a missing *state* goes on the component's
  theme in the fork, never as a token on the app surface.

### 3. The app: wire it into `cell_view`
Replace the `Readout` blob with the tree, fed by (1). Keep the modal's existing frame, header
(name + dtype badge + close) and dismissal paths — those match the canvas and are not in question.
Per-node copy is the natural affordance and belongs to the copy capability (P2-11), so route it
through `results::copy` rather than growing local clipboard wiring (AGENTS.md §5).

## Open questions
- Does a wide container page in-place ("show 30 more") or jump to an index? In-place is simpler and
  matches the sampled-text idiom.
- Should the tree replace the record view's field blocks too, once it exists? Deliberately deferred
  above — decide after using it.

## Acceptance
- Double-clicking `config.json`'s `contentBlocks` cell opens instantly and lets you expand a UUID
  key, then `content`, then read the leaves.
- Expanding a node materializes only that node: no measurable cost difference between a 2-key and a
  19,311-key sibling list.
- 19,311 sibling keys scroll smoothly (virtualized, not all mounted).
- Keyboard navigation works without the mouse.
- The `Tree` component lives in the fork, is themed, has an example, and the fork commit is
  **pushed** (AGENTS.md §6 — an unpushed gitlink breaks fresh clones and CI).
- `cargo test --workspace --locked` green on macOS; `UPDATE_SCHEMA=1 cargo test -p strata-freya
  schema_in_sync` if any theme changed.
