# P4-10 · Export window (rebuild to canvas)

**Phase:** 4 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** D6 / U13 · **Depends on:** P4-01, P2-01

## Goal
The export UI, rebuilt to the v19 canvas — keep the export backend (`COPY … TO`).

## Current state
Not built. In Freya, export **reads the run's snapshot** (P2-01) — no seed handoff (plan §6).

## Build (DEV_TASKS D6, to `Export.dc.html`)
- **Data-driven per-format option groups** (core + an **ADVANCED** section) instead of hardcoded match
  arms. CSV delimiter as a **text input** (resolve `\t`/`\n`); compression via `Select`.
- Drop the extra "Null as" segmented + the embedded DESTINATION field (filename → a separate Save-file
  browser). Partition **chips** + a **warning banner/hint**. UPPERCASE section labels.
- Export streams the **snapshot** with the active sort/filter (P2-01), not a re-run.

## Acceptance
- [ ] Per-format options render data-driven with an ADVANCED section; destination via the file browser;
      export writes the snapshot via `COPY … TO`.

## Freya / references
- Design: `Export.dc.html` (+ the export VM in `strata-windows.js`). Core export (`COPY … TO`). Snapshot
  = P2-01. DEV_TASKS D6/U13. strata-forms available for the option groups.
