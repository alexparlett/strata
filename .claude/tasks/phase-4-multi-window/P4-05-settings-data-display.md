# P4-05 · Settings ▸ Data-display

**Phase:** 4 · **Status:** ✅ · **DEV_TASKS:** U12 · **Depends on:** P4-03

## Goal
The Data-display category: the four settings that shape the results grid — row density, zebra
striping, the starting column width, and the `LIMIT` generated queries carry.

## What landed
`views/data_display.rs` replaces the `Pane::not_built(..)` placeholder behind `Route::DataDisplay`;
every control writes `SettingsCtx::draft` and stops there, and the footer's Apply commits.

**Every one of the four already had its consumer** — the grid reads density / zebra / column width
straight off the config store (`DataGrid::render`) and the catalog's View-table action reads
`row_limit` — so this task built the *control*, not the wiring. Nothing downstream changed.

### `views/field.rs` — the setting-row vocabulary
The pane is a `SettingList` of `Setting`s, and P4-06…P4-08 build on the same three types:

- **`SettingList`** — the divider-separated list. It draws the 24px gaps and the hairlines, so no
  pane spells the rhythm out.
- **`Setting`** — title, optional one-line hint, control. Two shapes, chosen by the control:
  `Setting::stacked(..)` puts it under the label block, `Setting::switch(..)` puts a `Switch` at
  the trailing edge with the label block as a *sibling* press target (never an ancestor — `Switch`
  doesn't stop propagation, so a wrapping ancestor toggles twice).
- **`NumberField`** — a digits-only `Input` plus its unit, clamped to the setting's bounds.
- **`edit_draft(ctx, |s| …)`** — the write-guard-out-of-context boilerplate every control wants.

`ThemePane` (P4-04) was refactored onto it, which is the point of putting it here rather than in
this pane: its hand-rolled Sync-with-OS row was the second copy of the label block, and the
Appearance pane now gets the shared one.

### Two decisions worth keeping
**The row shape is uniform: title → hint → control.** The canvas is inconsistent about this within
a single pane (Row density's hint sits *below* its segmented control; the Theme title takes a
wider gap than the numeric ones). Once dividers separate the rows, subtext that sometimes precedes
and sometimes follows its control leaves a reader unable to tell which setting a line belongs to.
Fixed in `Setting`, so every later pane inherits it.

**A draft-backed field publishes on every keystroke, not on submit.** Apply is a `Button` press,
and `Button` calls `a11y_id.request_focus()` and its `on_press` handler in the same breath — so a
value that waited for the field to be left or for Enter would never reach the draft the user is
about to commit. `NumberField` publishes each accepted keystroke (clamped) through `Input`'s
`on_validate`, which is the fork's own idiom for per-change derived state
(`examples/component_input_validation.rs`) and is re-read from the props each render, so no stale
handler is ever captured. **Losing focus is when the *text* is normalized** instead: the field
re-echoes what the setting actually holds, so an emptied or out-of-range field snaps back rather
than sitting there disagreeing with the value it published. That is what the component owns its
own `AccessibilityId` for — `use_focus(id)` beside the `Input`, since `Input` has no blur prop.

**The column-width bounds moved to `strata_core::config`** (`COL_WIDTH_MIN` / `COL_WIDTH_MAX`) and
the grid's `MIN_COL_W` / `MAX_COL_W` now derive from them. The field has to offer exactly the range
the grid honours: an input that accepts a width the grid then silently clamps is an input that lies
about what it sets. One definition, not two to drift apart. (The canvas's `min="40"` was such a
number — the grid has clamped to 56 since V20.)

## Acceptance
- [x] Fields edit the draft; Apply commits them and the grid picks up density / zebra / column
      width, and generated queries the row limit.

## Freya / references
- Design: `Settings.dc.html` Data-display. Grid: `results/datagrid`. DEV_TASKS U12.
