# P5-05 · Per-surface design audit (Freya drift pass)

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** Part 1 · **Depends on:** phases 2–4

## Goal
A final surface-by-surface audit of the Freya app against the `.dc.html` canvases — the DEV_TASKS
Part-1 "align vs build" pass, redone for the Freya build.

## Current state
DEV_TASKS Part 1 audited the *Dioxus* app. Once the Freya surfaces exist, re-audit each against its
canvas and file the residual drift.

## Build
- Walk each surface (launcher, header/rail, sidebar, editor/tabs, results grid/toolbar/status,
  inspector, drawer, settings, export, config) against its `.dc.html`; list concrete drift and fix the
  cheap aligns.
- Fold anything structural back into the owning phase task.

## Acceptance
- [ ] Each surface checked against its canvas; residual drift listed and the quick wins fixed.

## Freya / references
- The `.dc.html` canvases (`.claude/design-handoff/`), DEV_TASKS Part 1, `DESIGN_SPEC.md` §14.
