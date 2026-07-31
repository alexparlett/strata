# P6-01 · Command palette (⌘K) + depth

**Phase:** 6 · **Status:** ✅ · **DEV_TASKS:** U11 / T3 · **Depends on:** P2-20

## Goal
The ⌘K command palette, with the "depth" niceties.

## What was built

A permanently mounted `CommandPalette` inside `ProjectLoaded`, drawing only its ⌘K listener until
opened; the overlay card is conditional, so the query and the lit row reset per open by
construction. Its two triggers — the chord and the header's search button — write one
`State<bool>` slot the project root provides, the same shape as `DropTarget` / `ProfileTarget`.

- `apps/project/commands.rs` — the **command registry** (below).
- `apps/project/views/palette/` — `mod.rs` (the `command_palette` theme, the ⌘K node, the overlay,
  the keyboard model), `model.rs` (the index — pure, unit-tested), `row.rs` (a 42px row and a
  group heading).
- `components/keycap.rs` — the shared key cap, adopted by Settings ▸ Keymap.

Depth (T3) is all there: fixed group order, ↑↓ + Enter + Esc, per-item type icons, live shortcut
hints off `keymap::hint`, and the columns group.

## The registry — rmcp's shape, an enum's guarantees

`#[command_router]` / `#[command]` (`crates/strata-command-macro`, the workspace's first proc
macro) over one impl block. Per rmcp's `#[tool_router]`: the **id is the method name** and the
**subtext is the doc comment**, so nothing is typed twice and a command's description cannot drift
from the body it describes. `label`, `icon`, `key` and `keywords` are attribute arguments.

What it does **not** copy is rmcp's `HashMap<name, Arc<dyn Fn>>`, which exists because an MCP
client names a tool by an arbitrary string over a wire. A palette already holds the row the user
picked, and a palette command takes no parameters — so the macro generates an **enum** instead,
one variant per method. Two consequences, both the point:

- **Dispatch is total by construction** — every variant came from a method that has a body, so
  "registered but unrunnable" is not expressible. (rmcp's mirror-image footgun: a `#[tool]` outside
  the `#[tool_router]` block is silently *not* registered.)
- **A route holds a function pointer**, nothing captured, so `ROUTES` is a `const` slice — no
  allocation and no per-open build. (`strata-agent` itself has no `tool_router` field, so it
  rebuilds a 10-entry router on every `tools/call`; a palette filtering per keystroke could not
  afford that.)

`#[command]` is not a macro of its own — `command_router` consumes it before anything resolves it,
so there is nothing to import and a stray one is rustc's own "cannot find attribute".

This **overturns** `docs/FREYA_PORT_PLAN.md`'s "palette command registry (trait-object,
valin-style)" note. The location it reserved (`apps/project/commands.rs`) is kept.

## Every body is one call into an existing funnel

A palette row is a second way to ask for something, never a second implementation. Two pieces of
logic that were inline somewhere the palette cannot reach **moved to the funnel** rather than being
copied:

- `actions::run_query` — the "already running" gate, out of `workbench/mod.rs`. It asks
  `Engine::is_running` rather than the results pane's `running` mirror, because that mirror answers
  for the active tab only and the palette addresses the store from the window root.
- `close::close_project` — the close-while-running predicate, out of `project.rs`'s catch-all, now
  beside the `TabCloser` gate it mirrors. `spawn_forever`, because the close unmounts the scope the
  handler belongs to (the palette dismisses itself in the same breath).

Everything else already had one: `actions::save_as_view`, `CatalogActions::configure`,
`SessionState::{open_blank, open_drawer, toggle_pane}`, `OpenCtx::pick`, `open_settings`, and the
catalog's own `view_row` / `open_saved_query`.

**The palette is not a function of the keymap.** `key` renders the row's hint and nothing else;
synthesizing the chord (what `menu.rs` does, correctly, from a muda handler with no stores) would
make a command the user unbound unreachable from the one surface that exists so you needn't know
the chord.

## Decisions worth keeping

- **Nine actions, not the canvas's ten.** **Export results…** is not built: an `ExportLaunch` is
  assembled from the results pane's live sort and the page it has in hand, so the registry can
  neither build one nor tell whether there is anything to export. Wiring it needs either a request
  slot the results pane consumes (the `ConfigureTarget` shape) *plus* a settled-run mirror for the
  gate, or a second `RunQuery` subscription in a child that only exists while the palette is open.
  Not worth one row today; do it when the results pane is next opened up.
- **Top-level columns only.** `ColumnInfo::children` is a tree, and a real `config.json` measured
  241,425 nested fields in 19 columns (AGENTS.md §2) — indexing it would be that unbounded
  materialization in a new place, paid on every open. Views' columns are indexed as well as tables'.
- **The cap is per group**, not overall: a global cap lets a common substring fill the list with
  columns and push the table you were after off the bottom.
- **An empty query hides COLUMNS** (the canvas's `buildCmdk`) — every other group is bounded by the
  project and worth offering cold.
- **A table row and a view row open the data**, which is what pressing that row in the sidebar
  does. The design prototype's JS opened a view's *SQL*; editing the query behind a view is a
  different, more advanced gesture and stays in the row's own menu.
- **The key cap is a shared component now** (`components::keycap`), with two named shapes rather
  than an average: `key` (the Keymap grid's raised cap, heavier bottom edge) and `chip` (the
  palette's flat hint). The `settings` theme lost its `keycap_*` trio to a shared `keycap` token
  group.
- **The launcher's stub arm is gone**, not reworded: it consumed ⌘K and cycle-windows for targets
  that window will never have — no catalog, no command set, no project windows of its own.

## A breaking change for user themes

Moving `keycap_*` out of the `settings` group is **not** backward compatible, and the mechanism
gives no way to make it so: `strata_components!` states "No code defaults: a field the file omits
keeps its placeholder", and a colour's placeholder is magenta. So a user theme written before this
change renders Settings ▸ Keymap's key caps magenta — a surface that was fine before — on top of
the new `command_palette` group being magenta, which is the ordinary cost of adding any group.

Both built-in themes are updated, so this only reaches someone with a file in the user themes dir.
It is recorded rather than fixed because the fix is not local: a group-level fallback would be a
change to `register_component_themes` affecting every component theme, and inventing one for this
group alone would be the special case AGENTS.md §1 warns against. If theme migration ever becomes
a real concern, that fallback (or a load-time warning naming unauthored groups) is the shape to
build, once, for all of them.

## The keyboard model is the trap

Freya's `Input` `stop_propagation`s **and** `prevent_default`s every key but Enter/Escape/Tab, and
`prevent_default` on `KeyDown` cancels the derived `GlobalKeyDown`. So with the field focused a
global listener sees nothing — which is what makes the palette a genuine modal barrier (better than
`Dialog`, whose own docs note it is not focus containment), but means ↑↓, Enter, Escape and ⌘K are
all handled in `on_pre_key_down` before the field processes them. The overlay keeps a
`GlobalKeyDown` barrier as well, on a **different node** from the ⌘K listener — one handler per
event name.

## Acceptance
- [x] ⌘K opens the palette; typing filters; ↑↓/enter navigate + run; items show icons + shortcut hints.

## Freya / references
Freya overlay (`PopupBackground`'s two-rect structure, hand-rolled because it centres its child and
the canvas is top-aligned at 12vh), `ScrollView` + `ScrollController::scroll_to_item` for the lit
row. Canvas: `Strata.dc.html:1596-1639` (markup) and `:4088-4133` (`cmdkAll` / `buildCmdk`).
DEV_TASKS U11/T3.
