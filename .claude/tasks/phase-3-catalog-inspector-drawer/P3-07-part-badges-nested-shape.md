# P3-07 · PART badges · nested JSON · shape detection

**Phase:** 3 · **Status:** ⬜ `[core ✓]` (thin) · **DEV_TASKS:** D9 · **Depends on:** P3-02

## Goal
Hive `PART` chips, parseable-JSON handling, and folder/JSON schema-consistency reporting.

## Current state
Not built (thin even in the Dioxus app). Lowest priority in this phase.

## Build
- **PART chips** on partitioned tables from partition metadata (Freya `Chip`).
- Parseable-JSON echo for nested columns.
- A folder/JSON **schema-consistency** report (mismatched shapes across files).

## Acceptance
- [ ] Partitioned tables show PART chips; a mixed-schema folder reports the inconsistency.

## Freya / references
- Freya `Chip`. Core catalog/partition metadata. Design: `Sidebar.dc.html`.
