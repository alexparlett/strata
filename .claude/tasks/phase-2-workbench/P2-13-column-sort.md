# P2-13 · Column sort

**Phase:** 2 — Workbench · **Status:** ✅ **built** · **DEV_TASKS:** Rz6 · **Depends on:** P2-01/03

## Goal
The header sort chevron cycles asc → desc → clear and re-sorts the result.

## Built
- `results/sort.rs` — `SortState`: the per-press sort intent (owned by `ResultsBody`, threaded
  as props like `FindState` — it resets with every Run and survives paging, the same view-intent
  semantics the original Radio sketch wanted). Column identity is the **schema index** (Dioxus
  `ColSort` parity — names can collide across aliases); the cycle (unsorted → asc → desc →
  clear, another column restarts asc) is a pure function, unit-tested. Cycling clears the
  page-local selection and jumps back to page 1 in the same press (the pager-jump invariant).
- `ResultsBody` resolves the index to the engine's `(column name, ascending)` at the settled
  schema and folds it into `PageSpec.sort` — part of the snapshot read key, so every direction
  of every page caches forever; the engine applies `ORDER BY` over the **whole snapshot** at
  page-read (nulls last, real Arrow-type ordering — already landed with P2-01/03). A sorted
  page 1 is a real read (the Run's native page 1 only short-circuits unsorted).
- `datagrid/header.rs` — the chevron is live: up = asc, down = desc / unsorted, invisible until
  header hover (mounted throughout, so the name row never shifts), accent while active, comp
  tooltip, `stop_propagation` on the down so grabbing it never column-selects. `ChevronUp`
  joined the icon set.

## Acceptance
- [x] Clicking a header sorts the whole result (not just the page); chevron reflects asc/desc/clear.

## Freya / references
- `datagrid/header.rs`. `QuerySpec.sort` (P2-01/03) + core `.sort()` at page-read.
