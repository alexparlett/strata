# P3-06 · Catalog context menus

**Phase:** 3 · **Status:** ⬜ · **Depends on:** P3-02

## Goal
Right-click actions on catalog rows.

## Current state
Not built. The rows land with P3-02; this adds their menus and the drop-confirm flow.

## Build

Freya `ContextMenu` / `Menu` on each row type (right-click; the Dioxus app also had a `⋮` trigger
on hover — same item list, one source):

- **Table** → View table (`SELECT * FROM …` in a tab) · Profile (P3-09, disabled + "Profiling…"
  while one runs) · Configure (P4-11) · — · Drop table *(danger)*.
- **View** → View view · Profile · Edit query (open the view's SQL in its own tab, reusing an
  existing tab of that name, `Origin::View` so ⌘S redefines it) · — · Drop view *(danger)*.
- **Saved query** → Open in new tab · Rename · Delete *(danger)*. Addressed by `id`, so a rename
  is free — no origin rewriting (unlike a view rename, which must go through `ProjectState`).

**Drop confirms first**, via a dialog carrying the consequence line (P3-05): *"N view(s) read this
{table,view} and will be left invalid."* Count, not names — a busy table can back dozens of views.
Nothing breaks *now*: a view holds its sources by reference and keeps running until the project
reopens and its SQL re-plans, so dependents are flagged invalid (P3-04), not broken.

### These are direct engine calls, not cache invalidations

The old note here said drop / deregister / register are freya-query **mutations** whose
`on_settled` invalidates `FetchCatalog`. That is wrong on both halves — `FetchCatalog` does not
exist and must not (see P3-02: introspecting DataFusion would surface `__snap_*` result snapshots
and hide failed rows). The actual shape:

```
engine.drop_view(name).await      →  project.write_channel(ProjChan::Views).remove_view(&name)
engine.deregister(&name)          →  project.write_channel(ProjChan::Tables).remove_table(&name)
                                  →  project.peek().save_defs()
```

The `ProjectState` methods already exist (`remove_view`, `remove_table`, `remove_saved_query`,
`upsert_*`), parked `#[allow(dead_code)]` for this task. The store *is* the catalog: mutate it and
notify its channel — subscribers re-render, nothing refetches.

## Acceptance
- [ ] Each row type shows the right menu; every action reaches the engine and the Project store, and the defs are persisted.
- [ ] Drop asks first and states how many views it leaves invalid.

## Freya / references
- Freya `ContextMenu` / `Menu`; the existing `tab_bar/menu.rs` is the in-app precedent.
- Reference implementation: `strata-dioxus/src/ui/sidebar.rs` (`catalog_menu_items`, `remove_dialog`, `phrase`).
- Design: `Sidebar.dc.html` context menus.
