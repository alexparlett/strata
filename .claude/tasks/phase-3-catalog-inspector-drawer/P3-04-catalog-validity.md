# P3-04 · Catalog validity indicators

**Phase:** 3 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** D11 · **Depends on:** P3-02

## Goal
Flag invalid tables/views with a warning triangle (hover = reason).

## Current state
Not built. Core tracks `RegStatus::Failed` + `error` (tables) and `CatalogView.error` (views), and
`CatalogView.deps` supports a derived missing-dependency check.

## Build
1. **Tables:** surface `RegStatus::Failed` + error (missing file / bad path).
2. **Views:** surface `CatalogView.error` (SQL error / missing base at creation) **plus** a derived
   check — invalid if any of its `deps` (base tables) is absent or itself `Failed`.
3. Pure catalog computation, recomputed each render — no engine round-trip, self-heals.

## Acceptance
- [ ] A failed table and a view over a missing base both show a triangle with the right reason.

## Freya / references
- Core `RegStatus`, `CatalogView.{error,deps}`. Note DF-54 truth (dropping a table doesn't break a
  view until reload) — copy per DEV_TASKS D10/D11. Design: `Sidebar.dc.html` `.cat-warn`.
