# P4-09 · Settings search

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** W3 · **Depends on:** P4-03

## Goal
A search box in the settings nav that jumps to a setting.

## Current state
Not built.

## Wiring into the P4-03 shell

**The search box is deliberately absent from the nav, not inert** (P4-03): a field that returns
nothing is worse than none. Add it above the tree in `apps/settings/views/nav.rs`, and resolve a
hit through the same nav tree the rail and the breadcrumb read — `apps/settings/model.rs`'s
`CATEGORIES` / `category(&Route)`, whose test pins one category per route. A second copy of that
mapping in a search index is exactly what the module exists to prevent; give each indexed setting a
`Route` plus its own anchor id, not a category label of its own.

## Build (DEV_TASKS W3)
- Search box → a flat results list (label + category) over a settings index; picking a result **routes
  to its page and flashes the field** (scroll-to + a one-shot highlight). A "No settings match" empty state.

## Acceptance
- [ ] Typing filters settings; selecting one navigates to its page and flashes it; empty state shows.

## Freya / references
- Design: `Settings.dc.html` search. DEV_TASKS W3.
