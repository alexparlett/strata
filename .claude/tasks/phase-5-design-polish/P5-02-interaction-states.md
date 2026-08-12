# P5-02 · Hover / focus / active interaction states

**Phase:** 5 · **Status:** ⬜ · **Depends on:** —

## Goal
Consistent hover / focus / active / disabled treatments across all interactive elements, with
keyboard focus visibly distinct from pointer focus.

## Current state (verified 2026-08-12)
**Hover is two-tier: colours are centralized, mechanics are not.** The mapping table
(`theme/components.rs`) sets `hover_background`/`hover_border_fill`/`hover_color` for the fork
built-ins (button :73-78, outline_button :84, flat_button :90-93, filled_card :97) and Strata's
own themes (run_button :408-414, toolbar :485, sidebar_row :182-193, tab :532-533,
datagrid :723-729, table, list, scrollbar) — anything resolving from the table inherits hover for
free. But **20 files** hand-roll their own `use_state(|| false)` + `on_pointer_enter`/`leave`
bookkeeping and branch colours inline (`components/{run_button,toggle_button,segmented_toggle,sidebar_row}.rs`,
`launcher/views/row.rs`, `tab_bar/tab.rs`, `chat/{mod,transcript}.rs`, `palette/row.rs`,
`drawer/history.rs`, `results/{cell_view,running}.rs`, `results/chart/{paint,preview,strip}.rs`,
`results/datagrid/{cell,header}.rs`, `explain_plan/node.rs`, `export/views/formats.rs`,
`settings/views/theme.rs`) — several with hand-computed alphas (`toggle_button.rs:128`,
`segmented_toggle.rs:238`: `Role::Text.with_a(18)`).

**The app paints zero keyboard focus rings.** No `Focus::Keyboard` comparison exists anywhere in
`crates/strata-freya/src`; the existing `use_focus` calls are for scroll-into-view/caret only.
Twelve fork components already paint a `Focus::Keyboard`-gated ring (`button.rs:352`, switch,
radio_item, chip, segmented_button, sidebar, floating_tab, checkbox, select, menu, slider, card),
so anything built on them inherits it — `tool_button.rs:78` (on `Button`) and
`sidebar_row.rs:154` (on `SideBarItem`) do. But `toggle_button`, `segmented_toggle` and
`run_button` are hand-rolled rects with **no focus wiring at all** (no `a11y_focusable`, no ring).

**Fork gaps (the fix goes in the fork, AGENTS.md §6):**
- `Input` has **no hover colour fields** — `InputColorsTheme` carries only
  `focus_background`/`focus_border_fill` (`freya-components/src/input.rs:50-52`); hover state
  exists (`InputStatus::Hovering`) but only drives the cursor icon.
- `Input`'s focus border lights on **any** focus (`:783` tests `focus().is_focused()`), not
  `Focus::Keyboard` — so pointer focus and keyboard focus are indistinguishable on inputs.
- App side of the same seam: the default `input` mapping entry (`theme/components.rs:105-109`)
  never sets `focus_border_fill` — only `filled_input` (:116) and `flat_input` (:122) do.

## Build
**Fork half** (upstream-shaped: themed tokens, doc comments, an example; push the gitlink after):
- `Input`: hover colour fields on `InputColorsTheme` (applied like `button.rs`'s), and a
  `Focus::Keyboard`-gated ring alongside the existing any-focus border, matching `button.rs:352`'s
  shape.

**App half:**
- Give `toggle_button`, `segmented_toggle` and `run_button` focus wiring: `a11y_focusable` + a
  `Focus::Keyboard` ring from their own theme's `focus_border_fill` field — or, where it fits,
  rebuild on the fork `Button` ("standard components first", AGENTS.md §3) and delete the
  hand-rolled state.
- Set `focus_border_fill` on the default `input` mapping entry.
- Where a hand-rolled hover site branches colours the surface's component theme already names,
  read the theme field; hand-computed alphas become theme fields (a component's own dress never
  becomes a shared role — FREYA_UI).
- Verify against the canvases' state styles (`Design.dc.html`; the component states in
  `FreyaThemeGallery.dc.html`).

## Acceptance
- [ ] Hover/focus/active/disabled look consistent and theme-driven; keyboard focus shows a ring
      pointer focus does not.
- [ ] `toggle_button` / `segmented_toggle` / `run_button` are keyboard-focusable.
- [ ] Fork change pushed to the fork remote (gitlink), fork tests green.

## Freya / references
- Fork: `freya-components/src/{button,input}.rs`, `theming/themes.rs`. App:
  `theme/components.rs`, the 20-file list above. `use_focus` / `Focus::Keyboard` (freya skill).
