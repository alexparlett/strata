# P3-01 · Project layout shell

**Phase:** 3 · **Status:** ⬜ · **Depends on:** — · **Unblocks:** the rest of Phase 3

## Goal
The rail · sidebar · workbench · inspector · drawer frame, with resizable/collapsible panels.

## Current state
`apps/project/project.rs` mounts only the header + workbench. No side/bottom panels.

## Build
1. Wrap the workbench in Freya **`ResizableContainer` / `Panel` / `Handle`**: sidebar (left),
   inspector (right), drawer (bottom); each collapsible.
2. Add the activity **rail** (left edge) shell (buttons wire up per surface; Connections button = W7).
3. Persist panel sizes + collapse state to the per-window layout state (Radio station).

## Acceptance
- [ ] All four regions render, resize, and collapse; sizes persist across a reopen.

## Freya / references
- Freya `ResizableContainer`/`Panel`/`Handle` (plan §5 — "resizable panels come free").
- Design: `Strata.dc.html` (shell), `ActivityRail.dc.html`, `Sidebar.dc.html`.
