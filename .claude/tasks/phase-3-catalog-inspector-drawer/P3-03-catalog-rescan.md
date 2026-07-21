# P3-03 · Catalog re-scan

**Phase:** 3 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** D5 · **Depends on:** P3-02

## Goal
A refresh button re-infers catalog schemas.

## Current state
Not built. Core has `Command::RefreshCatalog` (schema-only re-infer; engine owns its schema).

## Build
Wire refresh as a freya-query **`MutationCapability`** (RefreshCatalog); its `on_settled`
invalidates `FetchCatalog` (or bumps the catalog `epoch`) so the sidebar refetches. Loading state
comes from the mutation. File sets / row counts / partition values are already live (DataFusion
re-LISTs per scan) — only inferred schema is frozen at registration.

## Acceptance
- [ ] Refresh re-emits `Registered` and updates the catalog + Events log; spinner shows during.

## Freya / references
- Core `Command::RefreshCatalog`. Design: `Sidebar.dc.html` refresh affordance.
