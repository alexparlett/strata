# P3-06 · Catalog context menus

**Phase:** 3 · **Status:** ⬜ · **Depends on:** P3-02

## Goal
Right-click actions on catalog rows.

## Current state
Not built.

## Build
Freya `ContextMenu`/`Menu` on each row type:
- **Table** → Profile (P3-09) · Refresh · Deregister · Drop (drop warns about dependent views, P3-05).
- **View** → Edit SQL (open in a tab, remembering origin for P2-16) · Drop.
- **Saved query** → Open · Rename · Delete.

> Drop / Deregister / Register are freya-query **mutations**; their `on_settled` invalidates
> `FetchCatalog` (and the affected profile/query caches), per state-arch §4.

## Acceptance
- [ ] Each row type shows the right menu; actions dispatch as mutations and invalidate the catalog.

## Freya / references
- Freya `ContextMenu`/`Menu`. Design: `Sidebar.dc.html` context menus.
