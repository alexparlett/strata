# Chart 08 · Copy chart as image — fork clipboard + offscreen capture

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 02 · Independent of 06–07,
09–11. **Touches the fork** — the gitlink push rule applies (AGENTS.md §6).

## Goal
A Copy Image press on the chart that puts a PNG of the current chart on the system
clipboard. Settled in planning (2026-08-07, Alex): the fork's clipboard grows **image
support now** (not a save-to-PNG stopgap) — true Copy Image in v1.

## Current state
- The fork's `freya-clipboard` (`crates/freya/crates/freya-clipboard`) is copypasta-backed,
  **text only**. No `.claude/tasks/` task owns image-clipboard (ED-07 is typed `COPY … TO`
  file export — unrelated), so this task owns the capability.
- Offscreen Skia render is proven in-repo: `freya-testing/src/lib.rs` (~line 580) —
  `raster_n32_premul(size)` → render → `image_snapshot().encode(…, PNG, …)`.
  `PlotSkiaBackend::new(canvas, font_collection, size)` takes any `&Canvas`.
- The only place app code holds a `FontCollection` is inside the `RenderCallback`'s
  `CanvasContext` (`chart/paint.rs`) — so capture must happen **during a paint**.
- `marks::draw` currently draws straight into the live canvas path.

## Build
1. **Fork** (`crates/freya/crates/freya-clipboard`): add image support — arboard (which
   copypasta wraps anyway) or a platform pasteboard path per the fork's platform-half
   convention (AGENTS.md §6: `cfg`-gated `freya-winit` module, documented no-op elsewhere,
   discoverable API). Follow the fork's own `AGENTS.md` (doc comments, `crate::` paths, no
   em dashes); keep the API upstream-shaped (e.g. `Clipboard::set_image(width, height,
   rgba)` beside `set_text`). **Push the fork before the app PR** — an unpushed gitlink has
   broken worktrees before.
2. **App — shared draw body**: refactor `marks::draw` to take
   `(canvas, font_collection, size, frame, hits)` so the live `RenderCallback` and the
   offscreen capture share one body (no second copy of the mark dispatch).
3. **App — capture-during-paint**: a `Rc<RefCell<Option<CaptureRequest>>>` slot beside
   `Hits`. The press sets it and requests a redraw; the render callback, seeing it, renders
   the current frame into an offscreen `raster_n32_premul` surface at a fixed export size
   (~1600×900), fills the background with `dress.background` first (the live canvas is
   transparent over the pane), encodes PNG, hands the bytes to the clipboard, clears the
   flag. The visible frame paints as normal in the same pass.
4. **The press**: a Copy Image affordance on the chart surface — recommend the results
   toolbar beside Download (it acts on the same settled run; `results/toolbar.rs`, folding
   with the others per the toolbar's one fold policy), shown only when the Chart view is
   active and data has settled. Log the outcome via `log_event` from whichever layer
   observes it (a log is recorded by its observer).
5. **Verification**: build; run via the `run-app` skill and paste the clipboard into a real
   target (Preview.app / Slack) on both a settled chart and a notice state (the press is
   absent or inert over a notice — nothing to copy). A capture of a themed chart honors the
   current theme.

## Acceptance
- [ ] Copy Image puts a PNG of the exact current chart (mark, sort, hidden series, theme)
      on the macOS pasteboard; nothing is written to disk.
- [ ] The fork change is pushed to the fork remote before the app change lands; a fresh
      submodule init builds.
- [ ] `marks::draw` has one body serving both paths; no second draw stack.

## References
`docs/CHART_SPEC.md` §9. `docs/reference/WORKFLOW.md` (fork rules, gitlink trap).
`crates/freya/AGENTS.md`. Offscreen pattern: `crates/freya/crates/freya-testing/src/lib.rs`.
