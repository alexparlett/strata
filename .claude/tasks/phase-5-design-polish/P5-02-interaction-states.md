# P5-02 · Hover / focus / active interaction states

**Phase:** 5 · **Status:** ✅ · **Depends on:** —

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
- [x] Hover/focus/active/disabled look consistent and theme-driven; keyboard focus shows a ring
      pointer focus does not.
- [x] `toggle_button` / `segmented_toggle` / `run_button` are keyboard-focusable.
- [ ] Fork change pushed to the fork remote (gitlink), fork tests green. *(fork tests green;
      the push is Alex's — the fork's own `AGENTS.md` forbids an agent pushing it.)*

## Built (2026-08-12)

**Fork** (`crates/freya`, two files):
- `InputColors` grew `hover_background`, `hover_border_fill` and `focus_ring_fill`, defaulted in
  `theming/themes.rs` for all three variants. `hover_background` is not redundant with
  `hover_border_fill`: `filled_input` and `flat_input` default to a transparent outline, so a
  fill is the only hover a variant without one can wear.
- `Input`'s render gained the hover arm on both the fill and the outline, and a
  `Focus::Keyboard`-gated **outer** ring layered over the existing any-focus outline
  (`checkbox.rs`'s shape, `.border(Option<Border>)` pushes onto the `borders` vec). The any-focus
  outline stays: a pointer press focuses the box too, so the ring is the only thing that can say
  *keyboard*.
- No new fork example: `examples/component_input.rs` already renders all three variants enabled
  and disabled, and hovering / tabbing it is the demonstration. No fork test either — the twelve
  existing `Focus::Keyboard` components have none, and `freya-testing` never enters
  `NavigationMode::Keyboard` (only `freya-winit`'s Forward/Backward focus does), so the ring is
  unreachable from a test without first growing the harness a way in.

**App:**
- `toggle_button`, `ToggleSegment` and `run_button` each take `use_a11y()` + `use_focus`,
  `a11y_focusable`, `a11y_role(Button)`, an **Inner** `Focus::Keyboard` ring off a new
  `focus_border_fill` (`item_focus_border_fill` on the segment), and `request_focus()` in their
  press. Inner rather than Outer because a toolbar segment sits in a pill that clips. They keep
  their hand-rolled hover state — `on_press` already covers the OS activation keys, so nothing
  else was needed, and rebuilding on `Button` would have cost each one its bespoke dress.
- Hand-computed alphas replaced by theme fields, four sites: `toggle_button`
  (`hover_background`/`hover_color`), `segmented_toggle` (`item_hover_background`), export's
  format card (`card_hover_border_fill`) and the chart's mark tile (`tile_hover_border_fill`).
  The two washes map to **`Role::ElevatedElementHover`**, not `GhostElementHover`: these controls
  sit on the rail, a toolbar, a form pill and a raised strip, and only the elevated role is
  authored translucent (its value is within a hair of the `Role::Text.with_a(18)` it replaces, in
  both built-ins). The two card edges map to `Role::BorderStrong`, whose doc names hovered cards
  and which Settings' theme card already uses — the canvas's own card hover is
  `brightness(1.12)`, i.e. no accent, so the half-alpha accent edge was the app's invention.
- `input` / `filled_input` / `flat_input` all gained `focus_border_fill` (`BorderFocused`),
  `hover_border_fill` and `focus_ring_fill` (`AccentMuted` — the canvas's literal
  `accent 22%` ring). `hover_background` is mapped **equal to `background`** on all three,
  because the canvas's field keeps `--c-panel` through every state and answers a hover on the
  outline alone; the flat field declines the hover outright, since every flat input in Strata
  (palette search, tab/saved-query rename, the composer) sits inside a box that carries it.
- The chat composer's per-instance undress had to grow the three new fields too, or the bar's
  "one outline around everything" would have gained a second one inside it.

**Deliberately left:** the `tab` theme's unpainted `hover_background` (the canvas's `.ps-tab` has
no `:hover` at all — painting it would diverge, so it stays a documented deferral), and the
launcher row's `RowAction` role reads, which are **P5-10's** (a role read, not a computed alpha).

## Freya / references
- Fork: `freya-components/src/{button,input}.rs`, `theming/themes.rs`. App:
  `theme/components.rs`, the 20-file list above. `use_focus` / `Focus::Keyboard` (freya skill).
