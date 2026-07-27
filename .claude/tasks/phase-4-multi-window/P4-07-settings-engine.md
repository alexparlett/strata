# P4-07 · Settings ▸ Engine (properties editor)

**Phase:** 4 · **Status:** ✅ · **DEV_TASKS:** W2 · **Depends on:** P4-03

## Goal
The Engine category: a free-form DataFusion **properties** editor, applied to every open project
window's engine (with restart-gated runtime keys).

## What was built

### The pane — `apps/settings/views/engine/`
- `mod.rs` — the frame: blurb, the four-tool toolbar (add / remove / duplicate / paste), the grid,
  the inspector, and **Revert changes** on the breadcrumb line.
- `model.rs` — `PropRows`, the editing model, unit-tested without a renderer.
- `table.rs` — the grid, on Freya's builtin `Table`.
- `inspector.rs` — the selected key's description, default, and RESTART / CUSTOM badges.

`Pane` grew the two opt-outs the canvas needs and nothing else takes: `maybe_trailing` (an action
on the breadcrumb line) and `filled` (the pane's full height instead of a scroll frame, for
content that manages its own overflow). Widened rather than bypassed, so the other four categories
keep one frame between them.

### Freya's builtin `Table` — investigated, adopted, four fork fixes

The investigation is the reason this task looks the way it does, so it is recorded here. `Table`
gave the bordered rounded box, the shared `column_widths` context, and a per-row bottom rule. It
could not give the three things this surface **is**, and each was a fork gap rather than a design
limit — so they were fixed in `crates/freya` rather than worked around (AGENTS.md §6):

| Gap | Fix |
| --- | --- |
| `TableRow.theme` was a `pub` field with **no builder**, and `key` is private, so a row could not carry its own fill — no selection tint, no zebra, no way to opt out of the hover response a selectable table doesn't want. Every other themed component (`Chip`, `Select`, `SideBarItem`, `Table` itself) had `.theme()`. | `TableRow::theme(TableThemePartial)` |
| Only `TableCell` had `on_press`; the canvas selects a row by pressing anywhere in it. | `TableRow::on_press(..)` |
| `TableCell` hardcoded `main_align(Alignment::End)` — right for the numeric columns a table usually holds, wrong for two text ones. | `TableCell::main_align(..)`, default unchanged |
| `Table::height` accepts any `Size`, but its root rect had no flex content, so a table with a stated height could not hand any of it to a scrolling body — no pinned header. | `.content(Content::Flex)` on `Table`'s rect (inert for the default `Size::Inner`) |

What is composed **inside** the parts stayed in the app: the column rule, the invalid-row stripe,
and the error message (a full-width sibling between rows rather than a cell — the fault belongs to
the property, not to either column). The header is a `TableRow` too, which is what earns it the
strip fill, the rule beneath it and the shared widths for nothing.

Rejected: hand-building the grid like `results/datagrid`. That is justified by virtualization and
column resize, and this surface needs neither — `Settings.engine` holds non-default overrides only.

### Rows are the editing model; the map is what commits
`Settings::engine` is a `BTreeMap`, which cannot hold the row you have not named yet or the
duplicate you are halfway through fixing. So `PropRows` is an ordered list of identified rows and
`to_map()` projects it back into `SettingsCtx::draft` on every edit — the window's single commit
path is untouched (Apply still merges field-by-field, `dirty()` still compares draft to seed).

The list lives on `SettingsCtx`, not in the pane: navigating to another category and back must not
throw away a half-finished edit, and the footer has to be able to ask what is blocking Apply
without the pane being mounted to answer (`SettingsCtx::blocker()`, which disables Apply and says
why — a button disabled for an invisible reason reads as a broken button).

**Row identity is a counter, not the name** — the name is the thing you are retyping. The counter
carries across a revert, so a stale selection can only fail to resolve, never resolve to a
*different* property.

**The boxes are the source; the list is downstream.** Each cell owns its `State<String>`, seeded at
mount, and an effect pushes changes into the list. Nothing writes the list back into a box: each
keystroke wakes the grid, and re-seeding on that wake would drag the cursor back. So the
autocomplete fills the **box**, not the list — the same one direction of travel `NumberField`
holds. Rows are keyed on their id so paste / revert / remove remount the boxes on new values.

### Validation
`strata_core::engine::config::value_error` per value (bool / int / bytes / duration / timezone /
enum / reserved), plus the two faults that are properties of the *list* rather than of a key: a
value with no name, and a duplicated name (both rows marked — either is the one to fix). Any error
blocks Apply.

### Applying — `Engine::set_config` + `state/engine_config.rs`
The task file used to name `Command::SetEngineConfig` / `Event::EngineRestartRequired`; **both were
deleted from `strata-core` with P2-01** and must not come back. What replaced them:

- **`Engine::set_config(overrides) -> bool`** writes the `ConfigOptions` half straight onto the
  live `SessionState`, and returns whether a restart is still owed. A **removed** key is not
  skipped — it is set back to its `ENGINE_KEYS` default, which completes the mapping: the keys
  `ConfigOptions` accepts are exactly the ones the catalogue names a default for, so every key that
  ever applied can also be un-applied.
- **`Engine::restart_owed()`** measures the live overrides against `built_runtime`, the
  `datafusion.runtime.*` set the `RuntimeEnv` was *built* with — not against the previous map. A
  user who declines the restart keeps the new values, and comparing the two maps would then report
  "nothing changed" and never offer the restart again.
- **`EngineCtx::new(overrides)`** — before this task the engine was built with
  `Default::default()`, i.e. **the setting was not wired at all**. The overrides are a launch
  value, because the `RuntimeEnv` half is fixed the moment the context is built.
- **`use_engine_config`** (mounted by `ProjectRoot`) subscribes `ConfigChan::Settings` and calls
  `set_config`. Settings has no engine of its own to talk to, so Apply just writes the config and
  each open project window picks the change up.

### The restart is the remount, through the one confirm
A changed `datafusion.runtime.*` needs a new `SessionContext`. `ProjectRoot`'s `render_key` already
drops-and-rebuilds a project for a re-root, so the restart is a bump of that key
(`EngineRestart`, a window-layer `State<u64>` that survives the remount it causes) rather than a
second path re-pointing a live store — the project registers its tables and views through the very
hooks that run at launch, which *is* the canvas's "registers your tables and views again".

Because it drops the engine, it aborts what is in flight, so it goes through the **T2 confirm** as
`CloseTarget::Restart` on the same predicate as every other destructive path (`guard.running` +
`confirm_close_running`) — not a confirm of its own. Declining leaves the restart owed.

### Theme
Three new `settings` fields — `table_head_background`, `table_selection_background`,
`table_zebra_background`: the row states a table cannot have an opinion about, because *which* row
is selected or striped is the caller's answer. Everything else the grid paints is Freya's builtin
`table` theme or the sheet's semantic slots (`error` / `warning`).

Two authoring bugs fixed on the way, both invisible until this task became `table`'s first
consumer: both themes authored `cell_hover_background`, which is not a field of `TableTheme` (the
schema forbids unknown keys, so it was simply never read) and neither authored
`hover_row_background` or `corner_radius`. And the zebra wash is now one palette slot
(`surface_zebra`) referenced by both the datagrid and this grid, instead of a repeated `specific` —
which also fixed daylight, whose copy was the *dark* theme's `rgba(255,255,255,.025)`.

Two new icons: `Minus` (remove — a minus, not a bin: the row is one of a list you are editing) and
`Clipboard` (paste).

## Deliberate divergences from the canvas
- **No "Unsaved engine changes" dot in the footer.** The canvas's Settings applies engine
  properties separately from the rest; ours has one Apply for the whole window, so the dot would be
  a second copy of `dirty()` — which the Apply button already shows. The footer carries the
  **blocker** line instead, which is information the button cannot convey.
- **The restart modal is the project window's T2 confirm, not a Settings-window dialog.** The
  canvas is drawn as though there were one engine; there is one per project window, and only that
  window knows what is running in it.

## Acceptance
- [x] Add / edit / remove / duplicate / paste properties, with catalogue autocomplete
- [x] Per-value + per-list validation, inline under the row, blocking Apply
- [x] Apply reaches every open project window's engine live
- [x] A changed `datafusion.runtime.*` prompts a restart, through the T2 confirm

## Tests
- `views/engine/model.rs` — 10 unit tests over `PropRows` (projection, the three error kinds,
  removal/duplication selection, paste parsing, revert id-freshness, suggestion filtering + cap).
- `strata-core::engine` — `set_config` moves a live option and restores a removed one to its
  default; a runtime key stays owed until the engine is rebuilt; the owned catalog names are fenced.

## Freya / references
- Design: `Settings.dc.html` Engine. Core `engine_config` (`ENGINE_KEYS`, `value_error`,
  `is_restart_key`, `is_owned_key`). DEV_TASKS W2.
