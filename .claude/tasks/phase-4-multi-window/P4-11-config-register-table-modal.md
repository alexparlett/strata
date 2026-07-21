# P4-11 · Config / register-table modal

**Phase:** 4 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** U14 / D7 · **Depends on:** —

## Goal
The register/edit-table modal: multi-path sources, format, Hive partitions.

## Current state
Not built. Core: `register_external` / `TableSpec`.

## Build (to `Configure.dc.html`, DEV_TASKS U14/D7)
- Multi-path **SOURCE PATHS** with browse + per-path counts; a **REQUIRED badge + resolution tooltip**;
  correct status order (below import-options, above Hive); drop the subtitle.
- Format selection; **Hive partition** detection (typed, with the string-cast warning).
- The LOCATION toggle + remote object stores belong to the **Connections workstream** (W7) — leave a hook.

## Acceptance
- [ ] Register a table over one or more paths/globs with format + Hive partitions; REQUIRED badge + tooltip.

## Freya / references
- Design: `Configure.dc.html`. Core `register_external` / `TableSpec`. DEV_TASKS U14/D7. LOCATION → W7.
