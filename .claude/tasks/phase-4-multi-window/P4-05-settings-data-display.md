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

### `components/form` — the shared form vocabulary
P4-05 landed on top of P4-10, which had just shipped the export window's `FieldRow` / `ValueField`
/ `NumberField` and the segmented toggle's form layout. Rather than keep a settings-only copy of
half of that, the row vocabulary moved into **one module** — `components/form`, under one `form`
component theme (renamed from `field_row`, plus `title_color` and `divider_fill`):

- **`FormList`** — the rows in order, and the only place the rhythm between them is spelled out.
  `.divided()` is the Settings panes' hairline-and-24px; the default gap is the window forms'.
  Both windows use it.
- **`FieldRow`** (+ `FieldNote`) — the **window form**'s row: uppercase eyebrow, ⓘ tooltip.
- **`Setting`** — the **settings pane**'s row: sentence-case title, inline subtext.
  `Setting::switch(..)` puts a `Switch` at the trailing edge with the label block as a *sibling*
  press target (never an ancestor — `Switch` doesn't stop propagation, so a wrapper toggles twice).
- **`ValueField` / `NumberField`** — the boxes, unchanged apart from what this task needed:
  `NumberField::unit("px")` (the label the canvas sets beside a measured number) and the blur
  normalize below.

**Two rows, deliberately.** `FieldRow` and `Setting` look like one row seen twice. The design
swept every inline explainer in the app into a hover tip, and then its **Settings consistency
pass** swept that window's four back out — "settling on subtext everywhere, since every non-toggle
setting already used it" — and made its panes uniform divider-separated rows, "matching Data
display". `field_row.rs`'s doc claimed the Settings panes too; corrected here, because the next
pane would have followed it into the wrong shape.

**Divergences are named, not averaged** (`form/mod.rs`'s "known divergences"): the row gap
(20 vs 24-rule-24), the label-to-control gap (8 vs 12), and where a `Switch` sits (stacked under
its label in a window form, trailing in a settings row). Each is one canvas differing from
another and one constant to change when the design settles it.

The Settings draft also gained `SettingsCtx::edit(|s| …)`, mirroring `ExportCtx::edit` — the two
windows now write their drafts the same way.

`ThemePane` (P4-04) and the export window's option list were both refactored onto the shared
pieces, which is the point of the module: the Appearance pane's hand-rolled Sync-with-OS row was
a second copy of the label block, and `Options` was hand-rolling the list.

### Decisions worth keeping
**The row shape is uniform: title → hint → control.** The canvas is inconsistent about this within
a single pane (Row density's hint sits *below* its segmented control; the Theme title takes a
wider gap than the numeric ones). Once dividers separate the rows, subtext that sometimes precedes
and sometimes follows its control leaves a reader unable to tell which setting a line belongs to.
Fixed in `Setting`, so every later pane inherits it.

**A draft-backed field reports per keystroke, and normalizes its box when it is left.** Reporting
per keystroke was already `NumberField`'s behaviour and is load-bearing: Apply is a `Button` press,
and `Button` calls `a11y_id.request_focus()` and its `on_press` handler in the same breath, so a
value that waited for blur or for Enter would never reach the draft being committed. What was
missing is the other half — the box was free to sit showing something the caller never received
(`abc`, an empty box, `9999` where the max is 2000). **Added to the shared component** (it is the
export window's bug too, not this pane's): losing focus re-echoes what was last *reported*, which
keeps `NumberField`'s own rule that it never re-reads the parent. Watching for that needed
`ValueField::a11y_id`, since `Input` has no blur prop.

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
