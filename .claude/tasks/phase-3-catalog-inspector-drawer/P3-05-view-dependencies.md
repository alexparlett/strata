# P3-05 · View dependencies (UI consumer)

**Phase:** 3 · **Status:** ✅ `[core ✓]` · **DEV_TASKS:** D10 · **Depends on:** P3-02/04

## Goal
Use the core-derived view→base-table deps in the UI.

## Current state
Built. Both directions of D10's deps are now consumed:

1. **"Is this row invalid?"** — landed with **P3-04**: `ProjectState::view_problem` walks a view's
   `deps` against the live table rows. Nothing to redo here.
2. **"What would this drop leave invalid?"** — this task: `ProjectState::dependent_views` plus the
   **drop confirm** that states it.

## Build

### `ProjectState::dependent_views(kind, name)`
The reverse lookup, alphabetical (rows are kept sorted). It dispatches on what is being dropped,
and the two lists are **not** interchangeable:

- **Table** → `ViewInfo::deps` — the base tables a view reads, *transitive* because the planner
  inlines nested views at creation. So a view-of-a-view over the dropped table is named too, which
  is precisely the reader a scan of SQL text would miss.
- **View** → `ViewInfo::view_deps` — the views a view reads. DEV_TASKS D10 records "which views
  read view B" as unanswerable, and it is *from `deps` alone*: deps are base tables by
  construction. The Freya store resolves `view_deps` from the engine's raw aliases at registration
  (`view_registered`), so the question is answerable here. `view_deps` was carrying
  `#[allow(dead_code)]` as a feature reservoir; this is its consumer.
- **Query** → nothing. A saved query is a stored string, not a SQL object; nothing can read it.

Names fold case (`same_name`) — deps come back from the planner, the dropped name comes from a def.
A view with **no landed answer** is not counted: there is no dependency information to read off it,
and mid-re-scan that is the whole catalog.

### The shared `Dialog` shell (`components/dialog.rs`)
Extracted while building this, and **both** confirms are on it: every centred confirm is now
**header · body · footer** on one card. `Dialog` owns the scrim, the overlay layer, the card
(420px · r14 · `surface_tertiary` · border · shadow · clip), the `24/24/16/24` body inset, the
hairline, the action strip and the modal key barrier (Esc dismisses · Enter confirms · everything
else consumed). `DialogHeader` owns the tinted chip (38 · r8 · 19px glyph) beside the title run —
only the icon and `tone` vary, and the tone colours glyph and fill together, so no dialog can end
up with a red icon in an amber chip.

Two details worth keeping:

- **The strip sizes its own actions.** `action()` takes the `Button`, not an `Element`, and stamps
  34px on it — 12 + 34 + 12 = the comps' **58px** bar. Freya's `button_layout` hugs its label
  (≈28px), so left to the call sites both dialogs shipped squashed. Asserted, not eyeballed
  (`the_action_strip_is_the_comps_fifty_eight_pixels`), because it was got wrong twice.
- **`on_dismiss` / `on_confirm` are `EventHandler<()>`**, not the usual `Event<T>` props: they are
  *outcomes* with two trigger sources each (Esc **or** backdrop; Enter **or** the caller's button).
  Freya types its own semantic actions the same way (`Popup::on_close_request`, `Menu::on_close`).

### The drop confirm (`views/dialogs/drop_confirm.rs`)
The comp's own parts over that shell: a trash chip (error tone), the action over its subject
(`Drop table` / `orders` in accent mono, ellipsised), the what-this-does line, and — when the drop
leaves something behind — an amber callout carrying the **count line then the names as chips** (a
96px well that scrolls past that). Flat Cancel + a destructive action wearing the shared
`cancel_button` dress.

> **Read the design from the bundle in *this* worktree.** The handoff is gitignored, so a worktree
> can sit on a stale copy while the main checkout has a newer one — which happened here: bundle 38
> against 39. In **39** the designer had already aligned the two confirms, and it overruled every
> judgement call this task had made on its own: card 430 → **420**, chip 40/r10 → **38/r8**, the
> inline `Drop table `orders`?` title → **stacked** verb over name, body copy mono → **UI**, Cancel
> outlined → **flat**. Check the bundle number before styling anything.

**Copy is D11's correction, not the canvas's.** The canvas says "will stop resolving"; the line
reads *"N view(s) read this {table,view} and will be left invalid:"*. Verified against DataFusion
54: a dependent view's plan captured its sources by `Arc` and keeps answering after the drop — it
fails on the next reload. So the dialog claims exactly what the row's triangle will claim (P3-04).

**Count *and* names.** P3-06's note said count-only (a busy table can back dozens of views); the
canvas lists names. Both, as the canvas reconciles them: the count leads the line, the names follow
as chips in a height-capped, scrolling well, so the warning can't grow the card off-screen.

### The drop itself
Confirming performs it, so the dialog is a working end-state rather than a placeholder:
`ProjectState::{remove_table, remove_view, remove_saved_query}` (their `#[allow(dead_code)]` is
gone) on the matching `ProjChan`, `save_defs()` in the same guard, then the engine
(`deregister` / `drop_view`; a saved query was never registered). Def first, engine after — the
`save_view` order. Nothing refetches: the store *is* the catalog.

Dropping a view or a saved query also **unbinds** the tabs that were saving to it
(`SessionState::{unbind_view, unbind_saved_query}`). The buffer survives — that is what the body
copy promises — but a tab left on `Origin::View("orders_daily")` would re-create the dropped view
on the next ⌘S and silently undo the drop.

### Typography
Nothing new. An earlier cut built the title as a `paragraph()` of mixed-style spans and added
`typography::{text_scale, role_span}` for it; the design's stacked header removed the only mixed
run in the app, so both helpers were **deleted** rather than left as unused API. If a genuine
mixed run turns up later, they are in the history.

## Acceptance
- [x] Dropping a table lists its dependent views in the confirm dialog.

## Freya / references
- Core `CatalogView.deps`. DEV_TASKS D10 (the planner-derived deps + DF-54 nuance), D11 (the
  "left invalid, not stop working" correction).
- Canvas: `Strata.dc.html`'s `removeOpen` block (+ the `remove` / `remove-deps` tiles in
  `Dialogs.dc.html`).
- Tests: `dependent_views` unit tests in `state/project.rs`; the rendered dialog in
  `views/dialogs/drop_confirm.rs`.

## Left for its own task
- **The trigger is P3-06's.** The dialog watches a `State<Option<DropTarget>>` provided at the
  window root; a catalog row's context menu setting it is all that remains. There is no second copy
  of the mechanics to write. `DropTarget` carries an `#[allow(dead_code)]` until then — nothing
  constructs a variant yet; drop the attribute with that task.
- **The rest of the dialog family onto `Dialog`.** The profile-cost confirm (P3-10), export /
  settings (P4) and the connection "forget" confirm (W7) are the same card; none of them should
  re-derive the strip or the barrier.
- **Profile invalidation** (P3-09) takes the same `deps` list from the third direction — a table
  registration landing should drop the cached profile of every view that reads it (D10). The
  landing point is marked on `ProjectState::table_registered`.
- **Connections** (W7) adds a fourth `DropTarget` variant ("Forget connection", in the canvas's
  `removeTitle`) whose consequence is about *tables*, not views.
