# P4-12 · Import (read) options (CSV/JSON)

**Phase:** 4 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** D8 · **Depends on:** P4-11

## Goal
Format-specific import-read options in the config modal, wired into registration.

## Current state
Not built. Designed in v19 (`Strata.dc.html` §~2205–2313; the import VM in `strata-windows.js`).

## Build (DEV_TASKS D8)
- A format-specific block in the config modal: **core** groups + a collapsible **ADVANCED**, data-driven
  inputs — **CSV** delimiter / header / null / quote / skip / comment; **JSON** settings (e.g.
  schema-infer rows). Non-CSV/JSON formats show nothing.
- Wire the controls → `TableSpec` → `register_external`.

## Acceptance
- [ ] CSV/JSON show their option groups (core + ADVANCED); the values reach `TableSpec` and affect the read.

## Freya / references
- Design: `Strata.dc.html` import section + the import VM in `strata-windows.js` (`imp*`/`csv*` fields).
  Core `TableSpec` / `register_external`. DEV_TASKS D8.
