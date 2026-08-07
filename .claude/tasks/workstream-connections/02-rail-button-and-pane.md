# Connections 02 · Activity-rail button + sidebar pane

**Workstream:** Connections (W7) · **Status:** ⬜ · **DEV_TASKS:** U2 / U3 · **Depends on:** 01, P3-01

## Goal
The rail entry point + the sidebar pane to manage connections.

## Current state
Not built. P3-01 provides the rail + sidebar; this adds the Connections view. Connections 01 built
everything below the surface — `ProjectState::connections` (a `ConnRow` per def, on its own
`ProjChan::Connections`), the registration pass phase, and `Engine::connect`. `SidebarPane::Connections`
and `IconName::Connections` already exist.

## Build
- An activity-**rail** toggle button (U2) that switches the sidebar to the **Connections pane** (U3).
- The pane lists connections with add / edit / remove, opening the editor (task 03).

## What Connections 01 handed over

- **The status dot is `ConnRow::reg`, not a probe of its own.** Green = `Reg::Ready`, amber =
  `Reg::Failed(why)` with the reason as the tooltip — the same shape a catalog row's triangle uses.
  `Reg::Loading` is mid-pass and reports nothing (see `reload_tables`' reasoning). Do not add a
  second liveness check: `engine::store::connect` already resolves the credential chain once, and
  that outcome *is* the dot.
- **Forget needs an `Engine::disconnect`** — not built, deliberately. DataFusion has
  `deregister_object_store` (`engine::store::connect` already calls it on its failure arm), and
  `register_pass` is additive by contract, so the removal gesture owns that call exactly as the drop
  confirm owns `Engine::deregister`. Without it a forgotten bucket stays queryable until the window
  is re-opened. Deregister by `ConnectionDef::url()`, which is the key it went in under.
- **The store mutators are 03's** (`upsert_connection` / `remove_connection`) — 01 left none,
  since nothing referenced them. If 02's Forget lands before 03's editor, Forget brings the remove.
- **Problems ▸ Project is an open call.** `ProjectState::registration_faults` deliberately covers
  tables and views only (its doc says so). A refused connection is the same kind of condition —
  true now, retracts itself on ↻ — so it would fit; deciding is this task's, because the pane is
  the surface the spec designed for it. Every connection outcome is already recorded in the event
  drawer either way.

## Acceptance
- [ ] The rail button toggles the Connections pane; connections list with add/edit/remove.
- [ ] A failed connection reads amber with the engine's reason; a re-scan (↻) clears it once the
      connection resolves.

## Freya / references
- Design: `Connections.dc.html`, `ActivityRail.dc.html`. DEV_TASKS U2/U3/W7. Depends on P3-01 shell.
