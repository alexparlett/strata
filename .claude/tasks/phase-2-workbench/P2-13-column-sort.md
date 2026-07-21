# P2-13 · Column sort

**Phase:** 2 — Workbench · **Status:** 🟢 `[core ✓]` · **DEV_TASKS:** Rz6 · **Depends on:** P2-01/03

## Goal
The header sort chevron cycles asc → desc → clear and re-sorts the result.

## Current state
`datagrid/header.rs` renders a chevron button that is **decorative** (the sort action is wired later).

## Build
1. Click cycles asc → desc → clear; store `sort` on the tab's **view intent** (Radio — survives
   paging, reset on a new result).
2. `sort` is part of the **snapshot** read key (P2-01); the engine applies `ORDER BY` over the
   snapshot at page-read (nulls last, real Arrow-type ordering); sorting resets to page 1.

## Acceptance
- [ ] Clicking a header sorts the whole result (not just the page); chevron reflects asc/desc/clear.

## Freya / references
- `datagrid/header.rs`. `QuerySpec.sort` (P2-01/03) + core `.sort()` at page-read.
