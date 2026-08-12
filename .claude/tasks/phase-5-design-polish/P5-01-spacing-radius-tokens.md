# P5-01 · Spacing & radius token scale across surfaces

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** F3 · **Depends on:** —

## Goal
Every padding / gap / corner-radius across the Freya app snaps to the design's spacing + radius
scale, not ad-hoc literals — and the app's duplicated layout constants get one home.

## Current state (verified 2026-08-12)
**No scale exists.** The theme layer is colour + typography only, and says so —
`crates/strata-freya/src/theme/components.rs:7-8`: "Layout tokens are literal constants." There is
no `sp_*` / `r_*` / `SPACING` / `RADIUS` anywhere in the crate. Layout is expressed three ways:

- **Bare literals at the call site** — 58 of 89 `.corner_radius(...)` calls, 57 of 176
  `.padding(...)` calls, ~190 `.spacing(...)` calls.
- **Per-file private consts** — 82 files declare their own layout `const …: f32`
  (`components/keycap.rs:53-65`, `components/segmented_toggle.rs:45-67`, `components/dialog.rs:52-54`,
  `components/form/row.rs:31`, `components/dot.rs:9`, …).
- **A handful inside the mapping table** — `theme/components.rs:186,366,740-741`.

The values cluster but don't agree. Radius: 1, 1.5, 2, 3, 4, 6 (×16), 7.5, 8 (×20), 10, 14 (×4),
50 — a real 6/8 spine with 1.5 / 3 / 7.5 / 10 as strays. Spacing: a 4/6/8/12 spine (4 ×24, 6 ×28,
8 ×69, 12 ×41) with 16/20 above and 1/2/3 below.

**Duplicated constants routed here by P5-06/P5-05's inventory** (the drift pass found them; this
task rehomes them):

- `TOOL_SIZE = 28.` (`components/tool_button.rs:24`) is the **only** shared/`pub` size constant.
- The 26px title-bar button is a **four-way copy**: `settings/views/title_bar.rs:37`,
  `connection/views/title_bar.rs:40`, `configure/views/title_bar.rs:33`, `export/views/title_bar.rs:33`
  (plus `settings/views/theme.rs:387`, `settings/views/engine/mod.rs:134`, `chat/header.rs:197`).
- Defined twice with the same meaning: `HEADER_CONTROL_SIZE`/`HEADER_CONTROL` = 24
  (`components/toolbar.rs:45`, `sidebar/mod.rs:72`), `ACTIONS_SIZE` = 22
  (`sidebar/catalog/entry.rs:65`, `sidebar/connections/mod.rs:107`), `STATUS_SIZE` = 12
  (`sidebar/catalog/entry.rs:50`, `sidebar/connections/mod.rs:110`).
- Panel header heights: `HEADER_HEIGHT` is a private const in **both** `sidebar/mod.rs:71` (48)
  and `inspector/mod.rs:94` (40) — same name, different values, nothing linking them.

## Build
- A **const module** (e.g. `theme/metrics.rs` or `components/metrics.rs`) carrying the spacing +
  radius scale — *not* theme-JSON fields: spacing and radius don't vary by theme, and the theme
  layer deliberately excludes layout. Source the values from `Design.dc.html` §03 (`--sp-*`,
  `--r-*`) in the design handoff.
- Sweep call sites onto the scale; snap the strays (1.5, 3, 7.5, 10 radius; the off-spine
  spacings) unless the canvas keeps one literal on purpose (e.g. the mac traffic-light inset).
- Rehome the duplicated constants above into the same module (icon-button sizes, header control
  sizes, status dot, title-bar button). **Rehome without renumbering**: where two copies disagree
  on the *value* (the 48/40/36 header heights), unify the home here and leave the value question
  to P5-05's canvas call — one const per height that is genuinely distinct, not a forced merge.
- P5-03 will co-locate its shared animation durations/easings in this module — leave room.

## Acceptance
- [ ] Padding/gap/radius come from the scale app-wide; a scale change reflows consistently.
- [ ] No layout constant is defined twice; `TOOL_SIZE` and friends live in one module.
- [ ] The deliberate literal exceptions are commented as such at the site.

## Freya / references
- Design: `Design.dc.html` §03 token scale (`.claude/design-handoff/`). `theme/components.rs`
  (the "layout tokens are literal constants" statement — update it to point at the new module).
  DEV_TASKS F3.
