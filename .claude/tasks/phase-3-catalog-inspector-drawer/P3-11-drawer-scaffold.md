# P3-11 · Drawer scaffold (tabbed bottom panel)

**Phase:** 3 · **Status:** ✅ · **DEV_TASKS:** U10 · **Depends on:** P3-01 · **Unblocks:** P3-12, P3-13, P3-14

## Goal
The bottom-drawer shell the three tabs render into: tab switcher, shared header, resizable height,
and the common list frame (sticky group headers, green-check empty states, indented rows).

## What it turned out to be
**P3-01 had already built most of this**, and the rest cannot be built honestly without its first
consumer. What shipped under P3-11 is the drawer header's **expand / restore toggle** — the one
piece with no dependency on tab content. The three original items resolved as:

1. **Tab switcher — already built, and not in the drawer header.** The design's switcher is the
   **activity rail's bottom group** (`onOpenProblems` / `onOpenEvents` / `onOpenHistory`, with the
   accent bar and the Problems error badge), which `views/rail.rs` builds. `Strata.dc.html`
   computes `drawerHistTabStyle` / `drawerProbTabStyle` / `drawerEvtTabStyle` and `onDrawerTab`
   (lines 3399, 6645–6657) and then **never renders them** — leftovers from an earlier iteration;
   `screenshots/rail-drawer.png` shows the shipped header as just `History  3`. **Do not add a pill
   row**: it would be a second writer for `Layout::drawer`.
2. **Shared header + Clear — deferred to the tabs.** The show/hide rule is one line, but the button
   would be inert in every sense: Events has **no store at all** (state-arch §8's `LogCtx` was
   never built), and History would need a `clear_history` + `history.jsonl` truncate that
   `strata-core::project` doesn't have. Same for the design's **count label** (`drawerCountLabel`)
   — nothing to count yet. **P3-12** adds the count and the Problems-hides-Clear rule; **P3-13**
   the log store and the first working Clear; **P3-14** the History one.
3. **List frame — deferred to P3-12.** No consumer today, which is the unreferenced pre-work
   AGENTS.md §5 rejects. It is also less shared than this file assumed: Problems is a sticky group
   header + icon·message·line rows at `--sp-7`; Events is flat bottom-bordered dot·message·ts rows;
   History is a card with a meta line over a 2-line SQL preview. Genuinely common: a scroll
   container and a centred empty state. **P3-12 builds them with Problems as the first consumer**;
   P3-13/14 reuse.

## Built
The header's **expand / restore** toggle (design `onToggleLogHeight`): a double-chevron button left
of the collapse ×, raising the drawer to `expanded_drawer_h()` (560, the canvas's height) and
putting back the height it had.

- `Layout::drawer_restore_h: Option<f32>` (`strata-model`) is **both** the remembered height and
  the expanded flag, so the icon can't disagree with the height.
  `SessionState::toggle_drawer_height` swaps them and returns the height it settled on.
- The write is on **`Chan::Layout`**, not `LayoutSize`: it re-seeds the panel's `initial_size` for
  its next mount (a collapse→reopen, or a restart) and persists.
- Moving the **mounted** panel is a separate half. `ResizablePanel` reads `initial_size` once, in a
  `use_hook` (fork `resizable_container.rs:412`), so re-seeding cannot resize a panel that is on
  screen. The shell now holds an explicit `ResizableContext` **controller** for the vertical
  container — the fork's own `component_resizable_panel_controller` example — and
  `shell::set_drawer_panel_height` drives the drawer panel (that container's only pixel-sized
  panel) through it. Supplying a controller means `direction` / `handle_size` come from the
  context, **not** the builder.
- Dragging while expanded leaves it expanded, deliberately: the drag records a new `drawer_h` and
  restore still returns to the pre-expand height ("back to before I expanded", not "back to the
  last drag"). Clearing the flag on drag isn't available — `set_drawer_h` writes on
  `Chan::LayoutSize`, which wakes nobody by design, so the icon would go stale.
- Expanded height is the canvas's flat 560, not a share of the window. A short window therefore
  squeezes the panes row (the drawer's own `min_size(140.)` still holds). Revisit if it bites.

## Acceptance
- [x] Drawer opens/collapses/resizes; the rail switches tabs and the title follows.
- [x] Expand raises the drawer and restore puts back the height it had; both survive a
      collapse→reopen and a restart.
- [x] Clear shows on Events/History, hidden on Problems → the **rule** landed with **P3-12**
      (`drawer/mod.rs`); the button is parked (`enabled(false)`) until **P3-13** builds the log
      store and **P3-14** the History truncate, each of which enables it for its own tab.
- [x] Count label → **P3-12**. It is a `DrawerCount` (`State<usize>`) the shell owns and the
      *mounted body* resolves — the `running` mirror's pattern, because the header cannot
      re-derive a body's list without a second number that can disagree with it. Problems writes
      its **error** count and resets the slot on unmount; P3-13/14 write theirs the same way.
- [x] List frame → **P3-12** built it as `drawer/frame.rs`: `DrawerBody` (the scroll container) and
      `DrawerEmpty` (the centred glyph + one line of copy). Exactly the two pieces this file
      predicted were shared; the row shapes stayed with their tabs.

## Freya / references
- Freya `ResizableContainer` + its `ResizableContext` controller (height), `VirtualScrollView`
  (lists). state-arch §8. Design: `DrawerProblems.dc.html`, `DrawerEvents.dc.html`,
  `DrawerHistory.dc.html` are all crops of `Strata.dc.html`'s `data-rg="drawer"` — read that
  (lines 1267–1348).
