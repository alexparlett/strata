# P4-09 · Settings search

**Phase:** 4 · **Status:** ✅ · **DEV_TASKS:** W3 · **Depends on:** P4-03

## Goal
A search box in the settings nav that jumps to a setting.

## What was built

`apps/settings/search.rs` — the index — plus the box, the results list and the empty state in
`views/nav.rs`, and a **reveal** capability on the shared form row
(`components/form/reveal.rs` + `Row::anchor`).

Three kinds of thing are findable, because the window holds three kinds of thing:

| Hit | What picking it does |
| --- | --- |
| `Hit::Setting(Anchor)` | routes to the page, then the row **scrolls itself into view and flashes** |
| `Hit::Property(&EngineKey)` | routes to Engine ▸ Properties, and selects the property's row **if it is overridden** (`PropRows::reveal`) |
| `Hit::Page(&Page)` | routes, and that is all — nothing on the page is a named setting yet (Keymap) |

`search()` matches **all** of a query's words against a hit's label, its own subtext, its extra
keywords and its page's breadcrumb, capped at 8 with the named settings first. Unit-tested without a
renderer.

## What it settled

**A setting's name lives in one place, and the compiler holds the panes to it.** An anchor is an
`Anchor` variant, not a string, and the table that generates the enum also carries each setting's
label, subtext, route and keywords — so the pane builds its row from the same entry
(`Anchor::row()`) and nothing can be filed under one name in the results list and another over its
own pane. The failure this rules out is silent and only visible by trying it: a mistyped anchor on
either side is a jump that navigates and then singles nothing out. The category is never spelled out
here either — a hit resolves its page through `model.rs`'s `category()`, the same nav tree the rail
draws and the breadcrumb reads.

**The engine's properties are the catalogue, not a chosen few.** They are indexed straight off
`ENGINE_KEYS` with their descriptions as search terms, so all 59 documented `datafusion.*` keys are
findable by what they do ("memory", "parallelism", "spill") — tunables that were otherwise reachable
only by typing into a grid on a page you had to know to visit. (The canvas hand-picked eleven; a
subset would have been a list to keep in step with the catalogue.) A property's hit reads as its
short name over its namespace, because the whole key would truncate to the part every key shares.

**Following a result is navigation; it never writes.** Every property is indexed and every one takes
you to the Engine pane, but a property nobody has overridden gets **no row made for it**. The first
cut did add one, pre-filled with the name — the canvas's "search doubles as add a known property" —
and it was rejected in review. Two reasons it is wrong: a named row with an empty value still
projects into the draft (`to_map` only drops the *unnamed* ones), so merely following a search result
left Apply live for an override nobody asked for; and the grid's whole claim is that it lists the
overrides in force, which a row for a property that has none breaks. The `+` button is how a
property gets a row, and its name field already autocompletes off the same catalogue.

**The engine row is not flashed, because it is selected.** The grid's selection fill is a persistent
accent tint on exactly that row, with the inspector under it describing the key — a 1.5s pulse over
the top of it would be an accent tint fading over an accent tint. What it *did* need was the reveal's
other half: `PropTable`'s body is now a controlled `ScrollView` and a row brings itself into view
when it becomes the selected one (the tab strip's pattern), which also fixes the Add button
appending a row below the fold.

**A revealed row belongs to the form, not to this window.** `Row::anchor` names a row and
`components::form::reveal` carries the ask: `Reveal` is a window-lived slot (it is written before the
page holding the target has mounted) and `RevealScroll` is the page-lived frame the row scrolls
within — both optional, so the export and Configure windows' forms are unaffected. The flash is an
`AnimColor` over the row's own box from the form theme's new `reveal_background` (→ the palette's
`accent_soft`, the canvas's `--accentSoft`).

**Esc empties the box before it closes the window.** The nav's listener sits inside the router's
subtree, which is before the root's in document order, and declines the press while the box is
already empty — the "returns `false` to fall through" shape `keymap::on_command` is built for.

## Divergences from the canvas (deliberate)

- **The flash does not bleed past the row.** The canvas spreads the wash 10px either side with a
  box-shadow; a torin child cannot paint outside the bounds its parent laid out, and inset-then-
  negative-margin would move every row on the surface to buy it. One constant (`FLASH_RADIUS`) and a
  note in `row.rs`.
- **Property labels are derived, not authored.** `datafusion.execution.batch_size` → "Batch size",
  which is what the canvas's hand-written labels amount to, without a second table to keep in step.

## Acceptance
- [x] Typing filters settings; selecting one navigates to its page and flashes it; empty state shows.

## Wiring notes for later tasks

- **P4-08 (Keymap):** the Keymap `Page` entry in `search.rs`'s `PAGES` exists only because the search
  box replaces the rail — a query for "shortcut" answering "no settings match" while a hidden Keymap
  row sat behind it would be the field lying about the window. When the pane lands, index its
  shortcuts as `Anchor`s (or as their own hit kind, if a keybinding is not a form row) and drop the
  page entry.
- **W7 (Connections):** the canvas's index has a `conn` category. A connections pane's settings are
  rows in the same table; a *connection* is not a setting and should not be indexed as one.

## Freya / references
- Design: `Settings.dc.html` search (`SW.SETTINGS_INDEX`, `gotoSetting`, `ps-setting-flash`).
  DEV_TASKS W3.
- `ScrollController::scroll_to_item` (fork) for the reveal; `freya::animation` (`AnimColor`) for the
  flash — the app's first use of either.
