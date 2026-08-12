# P5-01 · Spacing & radius token scale across surfaces

**Phase:** 5 · **Status:** ✅ · **DEV_TASKS:** F3 · **Depends on:** —

## Goal
Every padding / gap / corner-radius across the Freya app snaps to the design's spacing + radius
scale, not ad-hoc literals — and the app's duplicated layout constants get one home.

## What was built

**`crates/strata-freya/src/components/metrics.rs`** — the scale as constants, sourced from
`Design.dc.html` §03 and named for the design's own tokens:

- `SP_1`…`SP_9` = 2 / 4 / 8 / 12 / 16 / 24 / 32 / 40 / 48
- `R_XS`, `R_1`…`R_4` = 4 / 6 / 8 / 10 / 14

**Not** theme fields, and the reasoning is in the module doc: a step does not vary by theme, and a
theme author who could retune one could reflow every surface from a JSON file. `theme/components.rs`
now reads the scale like any other consumer, and its "layout tokens are literal constants" line
points here instead.

**The sweep** — 113 files. Every `.padding` / `.spacing` / `.margin` / `.corner_radius` /
`Gaps::*` / `CornerRadius::*` call and every layout-named private const now reads a step. Off-spine
values were snapped: spacing 6→8 (47 sites, the big one), 3→4, 10→8 or 12 per canvas, 13→12,
14→12, 20→16; radius 3→4 and 1.5→4. A surface that names its use of a step still does
(`const CELL_INSET: f32 = SP_4;`) — that is the application, not a second scale.

**The exceptions**, each stated at its site:

- **Pills and circles** — `metrics::pill(extent)`, a `const fn` returning half the extent, so the
  site says *circle* rather than `7.5`. The rail's problem badge, the plan node's self-time bar and
  group stripe, the Settings preview's traffic light.
- **Hairlines** — `metrics::HAIRLINE`. A 1px rule is a stroke that occupies a row, not the smallest
  gap; the canvases keep `gap: 1px` literal too.
- **Alignment nudges** — the 1px optical lift on an icon beside wrapped prose, the events row's
  dot/timestamp offsets, the plan tree's rail centring (now `RAIL_W / 2.` arithmetic).
- **One whole surface**: the Settings theme preview's miniature (`views/theme.rs`) — its 4px and
  5px runs are a drawing of a window at a tenth scale, not layout. Its *gaps* are on the scale.

**Rehomed into the same module** (no renumbering):

`TOOL_SIZE` 28 · `HEADER_CONTROL` 24 (was `HEADER_CONTROL_SIZE` + a sidebar copy) · `ROW_ACTION` 22
(was two `ACTIONS_SIZE`) · `STATUS_DOT` 12 (two `STATUS_SIZE`) · `STATUS_GLYPH` 14 · `ACTION_HEIGHT`
34 (moved from `components/mod.rs`; Settings' footer `BUTTON_HEIGHT` folded in) · `COMPACT_BUTTON` 26
(the four-way title-bar copy plus the chat header trigger, Settings' Revert and the keymap reset) ·
`TITLE_BAR_HEIGHT` 50 (four copies) · `TRAFFIC_LIGHT_GUTTER` 82 (five) · `SIDEBAR_HEADER_HEIGHT` 48 /
`RIGHT_PANE_HEADER_HEIGHT` 40 / `DRAWER_HEADER_HEIGHT` 36 · `PANE_BODY_MIN_W` · `CONTEXT_MENU_WIDTH`
210 · `MENU_ICON` 15 · `MENU_ROW_CHROME` 32 · `TABLE_HEAD_HEIGHT` 32 / `TABLE_ROW_HEIGHT` 34 ·
`EMPTY_TABLE_HEIGHT` 88 · `ERROR_STRIPE` 2 · `SETTINGS_FIELD_WIDTH` 130 · `PROGRESS_HOLD`.

Two notes on the rehoming, since both correct the inventory this task was filed with:

- The **three panel header heights** (48 / 40 / 36) each carried a doc claiming to match the other
  two. They are now three named constants with honest docs; the value question is P5-05's, and it
  is one edit here when it makes it. The inspector's and the chat pane's *are* merged
  (`RIGHT_PANE_HEADER_HEIGHT`) — one slot, `Layout::right`, so one row.
- `settings/views/theme.rs:387`'s 26 is **not** a fourth title-bar button; it is the preview
  miniature's rail width, and stays local as `RAIL`.

Two hand-rolled lookalikes fell out of the sweep and were replaced with the components that
already existed: AI ▸ Configure's status dot is now `Dot`, and the AI provider row's badge lands on
`Badge`'s own geometry (`SP_1`/`SP_2` inset, `R_XS`).

**Docs**: AGENTS.md §3 one-liner + the full entry in `docs/reference/FREYA_UI.md`;
`docs/reference/MODULE_MAP.md` lists the module.

## Acceptance
- [x] Padding/gap/radius come from the scale app-wide; a scale change reflows consistently.
- [x] No layout constant is defined twice; `TOOL_SIZE` and friends live in one module.
- [x] The deliberate literal exceptions are commented as such at the site.

## For P5-03
The module has a **Timing** section holding `PROGRESS_HOLD`. Shared animation durations and easings
go there, beside it — the module doc already says so.

## Freya / references
- Design: `Design.dc.html` §03 token scale (`.claude/design-handoff/`), and `FEATURES.md`'s
  "Design foundations (tokens)" for the same values in one line. DEV_TASKS F3.
