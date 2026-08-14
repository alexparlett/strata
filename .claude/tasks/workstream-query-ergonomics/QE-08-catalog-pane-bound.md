# QE-08 · The catalog pane survives a keyed struct

**Workstream:** Query ergonomics · **Status:** ⬜ · **Depends on:** DB-05 (the surface this
lands on) · QE-07 (the shared grouping, for the collapse half; the cap half needs nothing)

## Goal

Expanding a column in the catalog pane must never hang the window. Today it does: on the
real `config.json`, expanding `contentBlocks` (19,311 same-level children) **froze the app
until it was force-killed** (2026-08-14). Two fixes, layered:

1. **A floor** — a per-container cap on rendered children with an "N more" row. This is not
   a stopgap the collapse later deletes: even a collapsed, virtualized tree needs a floor
   against a genuine 5,000-distinct-field record, so the cap survives every redesign.
2. **The collapse** — a data-keyed struct renders as its shape groups (QE-07's shared
   grouping): ~15 rows saying `<key> ×9545` with the representative shape expandable under
   each, instead of 19,311 UUID rows. The same answer `describe_table` gives, in tree form.

## Why this is a task and not a fix on the current pane

**DB-05 is rebuilding the catalog pane into the data-sources tree, in an active session, as
this was written (2026-08-14).** Fixing the old pane would collide with that build and then
be deleted by it; editing DB-05's task file mid-session would collide with the session. So
this file carries the coordination instead: **this task starts after DB-05 lands, targets
the new tree, and the bound requirement transfers to whatever that tree's rendering is.**
If DB-05's tree turns out to already virtualize per-container, the cap may shrink to a
guard; the collapse half stands regardless, because virtualization fixes the hang but still
shows a wall of UUIDs — the freeze and the uselessness are two different defects.

## The freeze, diagnosed (against the pre-DB-05 pane — anatomy, not a patch site)

- The pane is a plain `ScrollView`, not virtual (`catalog/mod.rs:127`); its own test helper
  says off-screen children stay in the tree (`catalog/interaction.rs:231`).
- The entry body flattens **all** expanded rows and mounts one `ColumnRow` element per row
  (`catalog/entry.rs:487-535`): `flatten_cols` recurses into every expanded container with
  no bound (`catalog/columns.rs:38-68`), then `.children(rows.into_iter().map(...))` mounts
  the lot. 19,311 mounted elements is what wedges Torin — the expand click itself triggers
  the rebuild, so the click is the hang.
- Aggravator: `rail_height = rows.len() × 25.0` (`entry.rs:504`) — a rect ~483,000 px tall.
- The freeze is on main and predates QE-03; nothing in PR #163 touches it.

## Build (against DB-05's tree — re-verify every site above first)

1. **The cap.** A per-container rendered-children bound (constant ~200, named, doc'd) and a
   terminal "N more columns" row stating the elided count. The row is honest chrome, not a
   control — reaching an unshown column is search/inspector territory (if DB-05's tree grew
   a filter box, say so here; do not build one in this task).
2. **The collapse rows.** Where a container's children group into keyed sets
   (`strata_core::engine::schema_shape`, QE-07), render one row per set — placeholder name,
   `×N` count, the representative's subtree expandable beneath it — then the singular
   children. Collapse only past the cap (the same *cutting not projection* rule QE-03
   settled: a container that fits its cap shows real names). Expansion state for a set row
   needs a stable key that is not a real path segment — derive it from the set's shape,
   not its first key, so a file edit that reorders keys does not flap the expansion.
3. **Tests.** An interaction test over a synthetic 20,000-key struct asserting the expanded
   row count is bounded (≤ cap + chrome) and the "N more" row states the remainder; a
   grouped fixture asserting set rows with counts; the real-file check joins QE-07's
   `#[ignore]`d probe (a hand-run, stated-reason ignore — see QE-07 §Build 4 for the
   doctrine caveat).

## Acceptance

- Synthetic 20,000-key struct: expand completes without jank, bounded row count, remainder
  stated.
- Real `config.json` (hand-run): expanding `contentBlocks` paints promptly; the level reads
  as ~15 shape rows + singular leftovers, not a UUID wall.
- A genuine wide record below the cap (e.g. `placement.placements`, 49 children, no shared
  shape) renders every real name exactly as before.
- Full check green.

## Files

DB-05's tree module (path known once it lands; today's sites are
`crates/strata-freya/src/apps/project/views/sidebar/catalog/{entry,columns,mod}.rs`) ·
`crates/strata-core/src/engine/schema_shape.rs` (consume only — owned by QE-07).
