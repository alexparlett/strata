# P5-07 · One `Search` control for the app's eight filter boxes

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** —

## Goal
The search / filter inputs in the app become one `Search` component, so the magnifier, the
placeholder dress and the box size stop being a per-call-site decision.

## Current state (verified 2026-08-12 — the inventory has grown from four boxes to eight)

| # | Where | Dress | Magnifier | Notes |
|---|---|---|---|---|
| 1 | `launcher/views/projects.rs:119` — "Search projects" | body | inside (`.leading`, 14) | `SEARCH_MAX_WIDTH = 420.` |
| 2 | `sidebar/mod.rs:123` — "Filter catalog…" | mono, compact | inside (13) | suppressed below `CATALOG_FILTER_MIN` |
| 3 | `tab_bar/controls.rs:190` — "Find a query tab…" | body, compact | inside (14) | `auto_focus`; its doc comment records the magnifier-inside convention |
| 4 | `results/toolbar.rs:140` — "Find in results" | mono, compact | **sibling `Icon`** | `auto_focus`; Input stripped transparent, panel rect carries the chrome; the close ✕ **cannot** be `trailing` (the Input's focus-press `prevent_default`s pointer-down — comment at :112-118) |
| 5 | `settings/views/nav.rs:205` — "Search settings" | body | inside (13) | the only one with `on_submit` (jumps to first hit); glyph colour deliberately from the `settings` theme |
| 6 | `palette/mod.rs:423` — command palette | flat | **sibling `Icon`** | `auto_focus`; heavy `on_pre_key_down` (Up/Down/Esc/toggle) |
| 7 | `chat/mention.rs:482` — attach-picker "Search" | compact | inside (13) | `auto_focus`; placeholder has no ellipsis |
| 8 | `export/views/partition.rs:215` — "Filter…" | **`ValueField`**.bare() | inside (12) | the only one not on `Input` |

Divergences: magnifier 6 inside / 2 sibling; icon size 12–14; dress body(4)/mono(2)/other(2);
ellipsis on 5 of 8; `auto_focus` on 4; `on_submit` on 1. The two sibling-magnifier sites are the
ones that are *wrong* rather than merely repeated — their glyph neither focuses nor scrolls with
the field. (The old note that the connections pane would be the fifth caller is dead: the
connections sidebar pane grew **no** filter box, deliberately.)

## Build
- `crates/strata-freya/src/components/search.rs` — `Search`, built directly on Freya's `Input`,
  owning the magnifier, placeholder, dress and size. Builders roughly:
  `.mono()` · `.compact()` · `.bare()` · `.auto_focus()` · `.on_submit()`.
- Move the eight call sites onto it. Site-specific constraints the component must carry:
  - **The results find ✕** (site 4): a trailing press inside the Input is swallowed by the
    focus-press `prevent_default` — either `Search` accommodates a sibling action slot, or the
    fork's `Input` gets a real `trailing`-press fix (AGENTS.md §6: prefer the fork fix over a
    component workaround; check `extensions.rs` first).
  - **The palette** (site 6): `on_pre_key_down` must pass through — `Search` forwards the
    handler, it doesn't own key policy.
  - **The export partition filter** (site 8) moves off `ValueField` onto `Search`: it is a
    filter, not a form value, and `ValueField`'s form guarantees (mono-only, `FIELD_HEIGHT`,
    length cap) are the wrong contract for it.
- Settle the body/mono and default/compact splits against the canvases while doing this, not
  preserve them by default; one placeholder convention (ellipsis or not).

**Not `SearchField`, and not in `components/form`.** A filter box is not a form control — it is
transient, always carries a magnifier, and has no label, no hint and no row around it. "Field" is
the form vocabulary's word (`ValueField`, `FieldRow`) and borrowing it here is exactly the drift
`components/form` exists to stop. Plain noun, per AGENTS.md §3 naming.

**Built on `Input`, not on `ValueField`.** `ValueField` is deliberately the form's *value box*:
mono-only, a stated `FIELD_HEIGHT`, a length cap enforced on the state. Making it serve filters
would mean growing it a dress switch, `auto_focus` and a way out of its default height — i.e.
leaking the form module into a control that has nothing to do with forms.

**The results pager is deliberately out of scope** (`results/status_bar.rs`). It looks like a
shared numeric field and is not: it commits on submit only (each report is a `FetchSnapshotPage`,
so per-keystroke would load a page per digit), it *follows* its parent (the chevrons and the
page-size dropdown write `page` and the box syncs back), and its max is derived per render. Two of
those are the opposite of what `components::form::NumberField` guarantees. Leave it alone.

## Acceptance
- [ ] One `Search` component; the eight call sites are one line each.
- [ ] Every magnifier is inside its input — including the find panel's and the palette's.
- [ ] The dress / size variants that remain are the ones the canvases actually differ on.

## Freya / references
- `Input::leading` / `auto_focus` (fork `freya-components/src/input.rs`); the focus-press
  `prevent_default` seam (`results/toolbar.rs:112-118`).
