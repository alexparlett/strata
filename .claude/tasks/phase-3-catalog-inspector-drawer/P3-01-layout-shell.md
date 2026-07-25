# P3-01 · Project layout shell

**Phase:** 3 · **Status:** ✅ · **Depends on:** — · **Unblocks:** the rest of Phase 3

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
- [x] All four regions render, resize, and collapse; sizes persist across a reopen.

## As built (PR #32)
- Layout folded into **`SessionState`** (not a separate store): `Layout` / `SidebarPane` /
  `DrawerTab` serde types in `strata-model`, a `layout` field + toggle methods on `SessionState`,
  persisted via the existing `SessionSnapshot` + `use_autosave` — so it **survives restart** with no
  new persistence wiring. Two channels, both deriving `Persist`: `Chan::Layout` (structure; shell +
  rail subscribe) and `Chan::LayoutSize` (sizes; peeked to seed, so a drag persists without churning
  the shell).
- Shell in `views/shell.rs` (nested `ResizableContainer`s); panels **keyed** so `Workbench` survives
  a sibling collapsing. Rail in `views/rail.rs`; sidebar/inspector/drawer shells under
  `views/{sidebar,inspector,drawer}/`.
- Rail buttons are standard **`ToggleButton`s** (generalised with `.width()`/`.height()`) reusing the
  `toggle_button` theme — **no new themed component, no schema change**. The design's left
  accent-bar indicator is omitted (stale in the `.dc.html`; the accent-soft active bg carries state).

## Freya / references
- Freya `ResizableContainer`/`Panel`/`Handle` (plan §5 — "resizable panels come free").
- Design: `Strata.dc.html` (shell), `ActivityRail.dc.html`, `Sidebar.dc.html`.
