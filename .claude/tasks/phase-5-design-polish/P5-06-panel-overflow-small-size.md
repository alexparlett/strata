# P5-06 · Panel overflow & small-size behaviour

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** P3-01 (shell) + the content tasks (P3-02, workbench Phase 2)

## Goal
Define and implement how each resizable region **degrades gracefully when dragged small**: what
scrolls, what folds, and what hides — instead of clipping or being crushed. A cross-cutting
interaction pass over the shell that P3-01 deliberately punted (it shipped **hard** min-sizes to keep
panels usable; this task revisits them in favour of softer mins + graceful degradation).

## Current state (after P3-01)
`ResizableContainer` panels have hard `min_size`s (sidebar 210 / inspector 220 / drawer 140; the
editor pane 92 in the workbench's inner split), and the panel rects `overflow: Clip` — so at the min
the content just stops shrinking, and anything larger than the panel clips with no scroll. Good enough
for empty shells; not the intended behaviour once real content lands.

## Build (per-surface overflow policy)
Design the rules against the `.dc.html` canvases (which already mark `ps-scroll` on the sidebar /
inspector bodies), then implement per surface:

- **Editor + results (vertical):** when the editor pane is dragged very small, the editor **and** the
  results should scroll vertically within their panes; the editor/results **toolbars fold** (collapse
  to an overflow affordance) rather than being crushed. Revisit the 92px editor min.
- **Sidebar / catalog (horizontal):** when narrow, the catalog tree **clips → should scroll**; the
  header (filter search box + rescan) needs a **threshold** below which chrome is dropped — e.g. hide
  the search input (or collapse it to an icon) before the panel becomes unusable. Pick the breakpoints.
- **Inspector (horizontal):** the facts / stats body scrolls; decide any header degradation.
- **Drawer (vertical):** the tab strip + Clear fold / overflow when short; body scrolls.
- **Cross-cutting:** decide soft-vs-hard min-sizes per panel, and where a scroll container belongs vs
  where the content task already provides one (so we don't double-wrap).

## Acceptance
- [ ] Each region degrades to a defined scroll / fold / hide behaviour at small sizes — no crushed
  chrome, no silent clipping — with the breakpoints documented.

## Notes
- Best done **after** the content tasks (P3-02 catalog search bar, the Phase-2 editor/results
  toolbars) so the fold/hide thresholds are tuned against real content, not shells.
- Freya: `overflow(Overflow::Scroll)` / a `ScrollView` / `VirtualScrollView` for scroll containers;
  fold thresholds keyed off the panel's measured size (`on_sized`), mirroring the shell's size probe.
  Per-panel **max**-size is a separate Freya-fork gap noted in P3-01.

## References
- `Strata.dc.html` (region `ps-scroll` markers), `ActivityRail.dc.html`, `Sidebar.dc.html`; the P3-01
  shell (`views/shell.rs`, `views/{sidebar,inspector,drawer}/`).
