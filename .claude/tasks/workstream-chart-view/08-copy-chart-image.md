# Chart 08 · Copy chart as image — fork clipboard + offscreen capture

**Workstream:** Chart (Rz2) · **Status:** ✅ · **Depends on:** 02 · Independent of 06–07,
09–11. **Touches the fork** — the gitlink push rule applies (AGENTS.md §6).

## Goal
A Copy Image press on the chart that puts a PNG of the current chart on the system
clipboard. Settled in planning (2026-08-07, Alex): the fork's clipboard grows **image
support now** (not a save-to-PNG stopgap) — true Copy Image in v1.

## As built

**Fork** (`crates/freya/crates/freya-clipboard`) — copypasta **replaced** by arboard, which does
text and images both, and `Clipboard::get_image` / `set_image` beside `get` / `set`.

Two rejected cuts, in order, both Alex's calls on 2026-08-09 and both worth not re-deriving:

1. **arboard beside copypasta** — two backends, one per content type. Rejected: text and images
   are one clipboard, and a second connection is a second claim on the same selection; on the
   platforms that serve a paste out of the copying process, whichever was written last owns it.
2. **Collapsing to arboard by deleting the provider seam** — `freya-clipboard` owning a lazily
   created handle, with the `provide_root_context` calls removed from `freya-winit` and
   `freya-testing`. Rejected outright: that is fork *functionality* (a host that is not the
   desktop has nowhere to plug in), and nothing about wanting one backend asked for it.

**What shipped keeps the shape.** The integration still builds a `Box<dyn ClipboardProvider>` and
provides it into the root context; `Clipboard` still reads it from there and still answers
`NotAvailable` on `None`. What changed inside that shape: the trait is the fork's own instead of
copypasta's and carries `get_image` / `set_image` beside the text pair, and the desktop
implementation is `ClipboardContext` — **the same name the integrations already constructed**, so
`freya-winit` and `freya-testing` differ only in the import path. The one behavioural line that
had to go is winit's Wayland branch (`wayland_clipboard::create_clipboards_from_external` off the
raw display handle): arboard's constructor takes nothing.

**The Linux trade, stated because it is real.** copypasta reached Wayland through
smithay-clipboard over the standard `wl_data_device` on the app's own connection — every
compositor, focus-correct. arboard (feature `wayland-data-control`) uses `wl-clipboard-rs` over
`wlr-data-control` / `ext-data-control`: wlroots, KDE and GNOME 48+, otherwise a logged warning
and a fall back to its X11 backend, which under Wayland is the compositor's XWayland bridge. No
crate speaks the standard protocol *and* carries images, and a Wayland-only text provider is
rejected cut 1 again. Recorded on `ClipboardContext` itself.

The image type is a **struct** (`ClipboardImage { width, height, rgba }`), not the sketched
`set_image(width, height, rgba)`: three positional numbers are two that can be swapped silently.
`set_image` refuses a buffer that disagrees with the stated size rather than handing the platform
something to read past.

**App** — `chart/capture.rs` (`ChartCapture`), plus the shared draw body.

- `marks::draw(canvas, font_collection, size, frame) -> Vec<Hit>`. Two changes, both structural:
  it takes the two things a `CanvasContext` carried rather than the context (there is no context
  to build offscreen), and it **returns** the hit regions instead of writing them through the
  `Hits` handle — so a capture cannot overwrite what the visible plot last recorded for its
  pointer. The live `RenderCallback` assigns the returned vector.
- The `Frame` is an `Rc`, built once in `ChartView`'s drawable branch and handed to **both**
  `ChartCanvas` and `ChartCapture`. That is what makes "the image is the chart" structural rather
  than a thing to keep in step, and it also removes the deep clone per render the canvas used to
  pay for its slot seed.
- The capture is a fixed **1600x900 at 2x** (so `marks` lays out 800x450 logical units): the
  pane's own size would copy whatever labels a narrow drag had thinned away, and drawing the
  export's pixels as logical units would leave a 10pt tick label lost in a chart at twice the
  size. `dress.background` is filled first — the live canvas is transparent over the pane, which
  paints it. Pixels are read back as **unpremultiplied RGBA**, because `raster_n32_premul` is the
  platform's native order (BGRA on Apple) and a raw read puts a blue-for-red chart on the
  pasteboard.
- The press is a `ToolbarAction` on the shared results toolbar beside Download, supplied by the
  chart body (`ResultsToolbar::copy_image`) and therefore **absent** — not disabled — over any
  notice state: there is no chart to copy, and a greyed control says there is one that is merely
  unavailable. It folds tail-first with the rest under the one fold policy.
- `ChartCapture::copy` writes the log entry, because it is the layer that watched the clipboard
  take (or refuse) the image.

## Correction to the plan (do not re-derive)

The plan said "the only place app code holds a `FontCollection` is inside the `RenderCallback`'s
`CanvasContext` — so capture must happen **during a paint**", and specified an
`Rc<RefCell<Option<CaptureRequest>>>` slot, a redraw request, and a capture that the next paint
notices. **That premise is false**: the font collection is a root context
(`consume_root_context::<FontCollection>()`, exactly as `freya-code-editor::metrics` reads it),
so a press renders on its own. None of the slot machinery was built, and none of it should be —
it would also have put the outcome's log write inside a paint pass.

## Acceptance
- [x] Copy Image puts a PNG of the exact current chart (mark, sort, hidden series, theme)
      on the macOS pasteboard; nothing is written to disk.
- [x] The fork change is pushed to the fork remote before the app change lands; a fresh
      submodule init builds.
- [x] `marks::draw` has one body serving both paths; no second draw stack.

## References
`docs/CHART_SPEC.md` §9. `docs/reference/INVARIANTS.md` (the chart entry, Rz2/08).
`docs/reference/WORKFLOW.md` (fork rules, gitlink trap). `crates/freya/AGENTS.md`.
