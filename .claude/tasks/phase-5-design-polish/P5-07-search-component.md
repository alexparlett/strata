# P5-07 · One `Search` control for the app's four filter boxes

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** surfaces exist

## Goal
The four search / filter inputs in the app become one `Search` component, so the magnifier, the
placeholder dress and the box size stop being a per-call-site decision.

## Current state
Four call sites, each hand-rolling `InputTypography::…(Input::new(..).leading(Search).placeholder(..))`:

| Where | Dress | Box | Magnifier |
|---|---|---|---|
| [launcher/views/projects.rs](../../../crates/strata-freya/src/apps/launcher/views/projects.rs) — "Search projects" | `body` | default | `Input::leading()` |
| [sidebar/mod.rs](../../../crates/strata-freya/src/apps/project/views/sidebar/mod.rs) — "Filter catalog…" | `mono` | compact | `Input::leading()` |
| [tab_bar/controls.rs](../../../crates/strata-freya/src/apps/project/views/workbench/tab_bar/controls.rs) — "Find a query tab…" | `body` | compact | `Input::leading()` |
| [results/toolbar.rs](../../../crates/strata-freya/src/apps/project/views/workbench/results/toolbar.rs) — "Find in results" | `mono` | compact | **a sibling `Icon`** |

The last row is the one that is actually wrong rather than merely repeated: its magnifier sits
*outside* the input, so it neither focuses nor scrolls with the field the way the other three do.
The body/mono and default/compact splits look like drift — worth settling against the canvases
while doing this, not preserving by default.

## Build
- `crates/strata-freya/src/components/search.rs` — `Search`, built directly on Freya's `Input`,
  owning the magnifier, placeholder, dress and size. Builders roughly:
  `.mono()` · `.compact()` · `.bare()` · `.auto_focus()`.
- Move the four call sites onto it.

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
- [ ] One `Search` component; the four call sites are one line each.
- [ ] The find panel's magnifier is inside the input, like the other three.
- [ ] The dress / size variants that remain are the ones the canvases actually differ on.

## Freya / references
- `Input::leading` / `auto_focus` (fork `freya-components/src/input.rs`).
- W7's connections pane will be the fifth caller — see `workstream-connections`.
