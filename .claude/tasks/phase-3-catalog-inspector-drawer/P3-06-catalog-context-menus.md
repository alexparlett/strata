# P3-06 · Catalog context menus

**Phase:** 3 · **Status:** ✅ · **Depends on:** P3-02

## Goal
Right-click actions on catalog rows.

## What was built

Every catalog row carries a menu, on **two triggers** that share one item list
(`views/sidebar/catalog/menu.rs`): right-click the row, or press the trailing **⋮** the canvas puts
on it. `SidebarRow` grew `on_context_menu` for the first — a wrapper with `on_secondary_down`,
because Freya's `SideBarItem` exposes only `on_press` — so the connections pane (W7) gets the same
affordance for free.

- **Table** → View table · Profile table *(parked, P3-09)* · — · **Refresh table** · Configure
  *(parked, P4-11)* · — · Drop table *(danger)*.
- **View** → View view · Profile view *(parked)* · Edit query · — · Drop view *(danger)*.
- **Saved query** → Open in new tab · Rename · — · Delete query *(danger)*. Pressing the row
  opens it too, per the canvas's own row `title`.

A menu is a **snapshot**: the builders run from an event handler, so every read is a `peek`, like
the tab strip's menu. The rows underneath stay live.

### Opening a row is `SessionState::open_or_focus`

New, and it is what stops a row opening twice. A tab already **bound** to the row (`Origin::View` /
`Origin::SavedQuery`) is focused rather than duplicated — including when it has unsaved edits,
which is exactly the tab "edit this view" should land on; two tabs on one origin would mean two ⌘S
targets. A scratch row (View table's `SELECT *`, `LIMIT` from the row-limit setting) has no binding
to match on, so it reuses an untouched tab of the same name and text, and stops reusing it once
that buffer is edited.

### Rename is inline, and free

The menu item only flips the row's own `renaming` flag; the row reacts in its own scope, so the
rename outlives the menu that started it (the tab strip's rename, exactly). Enter commits, Escape
cancels, a press outside commits. `ProjectState::rename_saved_query` relabels **by id** and
re-sorts — no origin rewriting, and no collision rule, because ⌘S already mints saved queries under
whatever the tab is called and ids are what anything actually holds.

**A rename opens with the name selected**, so the first keystroke replaces it. That took a fork
change — `Input` had no way to start anywhere but the caret at position 0, so typing landed *in
front of* the name being renamed:

- `EditableConfig::with_select_all_on_init` + `UseEditable::create` computing the initial
  `TextSelection` from it (UTF-16 code units, the unit selections are in);
- `Input::select_all_on_init`, opt-in and mount-time only — a value that changes underneath the
  input later still syncs as a plain edit, so nothing else moves. Two tests in the fork's
  `tests/input.rs`: the new behaviour, and a guard that the default is unchanged.

The **tab strip's rename** had the same bug and now shares the fix. It seeded its draft
reactively, which hands the input an empty string at mount and syncs the name in afterwards — so
the selection had nothing to land on. Its input moved into a `TabRename` child that mounts only
while renaming, which makes `use_hook` the honest place to seed (the shape `QueryRename` already
had).

### Drop opens P3-05's confirm — there is no second drop path

Each Drop/Delete item sets the `DropTarget` slot the dialog watches, and nothing else. The dialog
already owns the consequence line and the drop itself (store + persist + engine + tab unbinding).
A test pins that the catalog is untouched until the dialog is confirmed.

### Refresh table (added on top of the original spec)

The canvas's table menu has **Refresh table**, and it is a real re-registration of one row, not a
label:

- `state/hooks.rs::refresh_table` — the same pass as the sidebar's ↻ (`refresh_catalog`), narrowed.
  `register_defs` now takes the table/view *work list* rather than reading the whole store, so
  project open, ↻ and a row Refresh are one implementation at three widths. Held by the same
  `CatalogScan` flag, so no two passes overlap; the item is disabled while one runs, and reads
  "Refreshing…" while *this* row is unanswered.
- `ProjectState::views_to_refresh` — the views that pass must re-create: those that **read** the
  table (`deps`, so transitively through a view-of-a-view), plus every view currently **failing**.
  Re-registering a table does not break a view over it — worse, the view goes on scanning the old
  provider with the old schema, because its plan captured that provider by `Arc` (D10/D11, and the
  decision P3-03 already made for the whole-catalog scan). A failing view has no dependency record,
  so retrying it is the only way "I fixed the path, refresh the row" can heal it.
- **The item raises a request; the window root runs the pass.** Caught in the app, not by the
  suite: `spawn` binds a task to `current_scope_id()`, which during an event is the handler's
  element — the `MenuButton` that the very same press then closes. Scope teardown drops its tasks
  before the future is ever polled, so the rows were reset to `Loading` and no answer ever came:
  the table *and* the view over it spun forever. Fixed first with `spawn_forever`, then folded
  into the **scan driver** `main` grew for the same bug on the ↻ (`ScanRequest` + `claim_scan` +
  `ScanGuard`), which is the better shape: the pass belongs to the root scope that owns
  `ProjectState`, and the flag is released by `Drop` so a *cancelled* pass can't latch it.
  `ScanRequest` gained a **`ScanScope`** — `All` for open and ↻, `Table(name)` for a row Refresh —
  so one driver serves every width. The test asserts the request and its scope, which is the part
  this task adds; the driver's own behaviour is covered where it lives.
- `ProjectState::refresh_order` — **fixes a latent bug in P3-03's ↻ as well.** `CREATE OR REPLACE
  VIEW` inlines the plan of any view it reads *at that moment*, so re-creating an outer view before
  its inner one inlines the stale inner plan. The scan was ordering views alphabetically (the def
  order), which is right only by luck. Kahn's over `view_deps`, computed **before** the rows are
  reset — resetting is what discards the ordering information.

### Also

`ProjectState::{reload_table, reload_view}` (one-row resets), `IconName::Pencil` (the canvas's
`edit` glyph — spent on Rename and Edit query, so Play consistently means "put this in a tab").

> **Test-harness note worth keeping.** Freya polls tasks only once *no scope is dirty*
> (`Runner::handle_events_immediately`), and `use_side_effect` is a task. Every row now mounts a ⋮
> `Button`, which costs one extra settle pass — so the catalog tests' two `sync_and_update()` calls
> silently stopped running effects, and the status slot's held verdict (P3-04) "disappeared" with
> no error. They settle through a `settle()` helper now. Under-settling fails quietly; assume it
> when effect-derived state is missing.

## Acceptance
- [x] Each row type shows the right menu; every action reaches the engine and the Project store, and the defs are persisted.
- [x] Drop asks first and states how many views it leaves invalid. *(The dialog is P3-05's; this is
  the menu item that opens it.)*
- [x] Refresh table re-infers that row and re-creates the views it would otherwise leave stale.

## Freya / references
- Freya `ContextMenu` / `Menu`; the existing `tab_bar/menu.rs` is the in-app precedent.
- Reference implementation: `strata-dioxus/src/ui/sidebar.rs` (`catalog_menu_items`, `remove_dialog`, `phrase`).
- Design: `Strata.dc.html` — the catalog row context menu (`buildCtxItems`) + the row's ⋮ button.
