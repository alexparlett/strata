//! **Copy Image** (Chart 08): the chart, exactly as it is drawn, on the system clipboard.
//!
//! The capture is an ordinary offscreen Skia render — a raster surface, the frame drawn into it
//! through [`marks::draw`], then its pixels handed to `Clipboard::set_image` (the image side the
//! fork's `freya-clipboard` grew for this). Nothing is written to disk: a save-to-PNG stopgap was
//! rejected in planning, so this is the real capability.
//!
//! **One draw body.** The capture calls the same [`marks::draw`] the live `RenderCallback` calls,
//! with the same [`Frame`] the visible plot is painting — so an image cannot show a different
//! mark, a different sort or a different theme from the chart it was copied off. That is why
//! `draw` takes a canvas and a font collection rather than a `CanvasContext`: there is no context
//! to build out here, only a surface.
//!
//! **No paint pass involved.** The font collection is a root context
//! ([`try_consume_root_context`](freya::prelude::try_consume_root_context)), the same one
//! `freya-code-editor` measures against, so a press can render on its own rather than flagging a
//! request that the next paint has to notice.

use std::rc::Rc;

use freya::clipboard::{Clipboard, ClipboardError, ClipboardImage};
use freya::engine::prelude::{
    raster_n32_premul, AlphaType, ColorType, FontCollection, ImageInfo, Surface,
};
use freya::prelude::{try_consume_root_context, Size2D};

use super::marks;
use super::paint::Frame;
use crate::apps::project::state::{log_event, LogCtx, LogLevel};

/// The copied image's size in pixels. Fixed rather than the pane's, so what lands in a document
/// is the same picture whatever the window happened to be doing: a pane dragged narrow would
/// otherwise copy a chart with half its labels thinned away.
const EXPORT_WIDTH: i32 = 1600;
const EXPORT_HEIGHT: i32 = 900;

/// How many pixels the capture puts in a logical unit.
///
/// The plot's furniture is sized in logical units (`marks`), so rendering 1600x900 of them
/// straight out would draw a chart at twice the size with the same 10pt tick labels lost in it.
/// Drawing 800x450 logical units at 2x is the retina pass the screen already does, and it is the
/// reason a capture reads like the chart rather than a stretched one.
const EXPORT_SCALE: f32 = 2.;

/// Four bytes a pixel, which is what [`ClipboardImage`] and the read below both mean by RGBA.
const CHANNELS: usize = 4;

/// What a press of Copy Image would put on the clipboard.
///
/// Carries the frame by handle — the same `Rc` the canvas paints from — so building one costs a
/// refcount rather than a copy of the read. Only built where a chart actually settled, which is
/// what makes the toolbar item's presence the honest test of "is there anything to copy".
///
/// The derived `PartialEq` is a **content** comparison, not a pointer one: `Frame` is not `Eq`,
/// so `Rc` has no identity shortcut to take. That is deliberate rather than overlooked. The
/// frame is a fresh `Rc` on every render of the body, so a pointer comparison would answer
/// "different" every time and re-render the whole results toolbar with it; the content
/// comparison is the same walk `ChartCanvas` already makes to decide whether to repaint, over a
/// read the engine caps at `ROWS_CAP`.
#[derive(Clone, PartialEq)]
pub struct ChartCapture {
    frame: Rc<Frame>,
    log: LogCtx,
}

impl ChartCapture {
    pub fn new(frame: Rc<Frame>, log: LogCtx) -> Self {
        Self { frame, log }
    }

    /// Render the chart and put it on the clipboard, recording the outcome.
    ///
    /// The log entry is written here because this is the layer that watched the write happen
    /// (AGENTS.md §2) — the press knows it was pressed, and nothing else learns whether the
    /// pasteboard took the image.
    pub fn copy(&self) {
        match render(&self.frame).map(Clipboard::set_image) {
            Some(Ok(())) => log_event(self.log, LogLevel::Ok, "Copied the chart as an image"),
            Some(Err(err)) => log_event(
                self.log,
                LogLevel::Error,
                format!("The chart could not be copied: {}", why(err)),
            ),
            None => log_event(
                self.log,
                LogLevel::Error,
                "The chart could not be copied: it did not render",
            ),
        }
    }
}

/// A clipboard failure in the app's own words. The variant name is the fork's vocabulary, and a
/// log entry is prose (AGENTS.md §3).
fn why(err: ClipboardError) -> &'static str {
    match err {
        ClipboardError::NotAvailable => "the system clipboard is not available",
        ClipboardError::FailedToSet => "the system clipboard would not take the image",
        ClipboardError::FailedToRead => "the system clipboard could not be read",
    }
}

/// Draw `frame` into an offscreen surface and read it back as unpremultiplied RGBA.
///
/// `None` where Skia would not give a surface or would not hand its pixels back — both are the
/// same thing to the caller, which has no image either way.
fn render(frame: &Frame) -> Option<ClipboardImage> {
    let mut surface = raster_n32_premul((EXPORT_WIDTH, EXPORT_HEIGHT))?;
    // `try_`, so a window with no font collection is the "no image" the caller already handles
    // rather than a panic on the render thread. Copying a chart is not worth taking the window
    // down for.
    let mut font_collection = try_consume_root_context::<FontCollection>()?;
    {
        let canvas = surface.canvas();
        // The live canvas is transparent over the pane, which paints the background behind it;
        // an offscreen surface has nothing behind it, so a capture without this is a chart
        // floating on whatever the target application puts under an alpha channel.
        //
        // **Forced opaque.** A theme may state its colours with alpha (`theme.schema.json`
        // allows `#RRGGBBAA` and `rgba(...)`), and a translucent `surface.raised` would clear to
        // a translucent background — putting back exactly the see-through capture this line is
        // here to prevent.
        canvas.clear(frame.dress.background.with_a(u8::MAX));
        canvas.scale((EXPORT_SCALE, EXPORT_SCALE));
        marks::draw(canvas, &mut font_collection, logical_size(), frame);
    }
    read_rgba(&mut surface)
}

/// The size [`marks::draw`] lays the capture out at: the export in logical units.
fn logical_size() -> Size2D {
    Size2D::new(
        EXPORT_WIDTH as f32 / EXPORT_SCALE,
        EXPORT_HEIGHT as f32 / EXPORT_SCALE,
    )
}

/// The surface's pixels as unpremultiplied RGBA8, which is what the clipboard takes.
///
/// Converted on the way out rather than assumed: `raster_n32_premul` is the platform's native
/// order (BGRA on Apple), and premultiplied, so reading the buffer raw would put a
/// blue-for-red chart on the pasteboard.
fn read_rgba(surface: &mut Surface) -> Option<ClipboardImage> {
    let info = ImageInfo::new(
        (EXPORT_WIDTH, EXPORT_HEIGHT),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let row_bytes = EXPORT_WIDTH as usize * CHANNELS;
    let mut rgba = vec![0u8; row_bytes * EXPORT_HEIGHT as usize];
    surface
        .read_pixels(&info, &mut rgba, row_bytes, (0, 0))
        .then(|| ClipboardImage {
            width: EXPORT_WIDTH as usize,
            height: EXPORT_HEIGHT as usize,
            rgba,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The capture is laid out in logical units and painted at [`EXPORT_SCALE`], so the two have
    /// to multiply back to the pixel size the clipboard is told about. Getting this wrong is not
    /// a visual glitch: the buffer's length is derived from the pixel size and the plot from the
    /// logical one, and a mismatch is a clipboard write the platform reads past.
    #[test]
    fn the_logical_layout_scales_up_to_the_exported_pixels() {
        let logical = logical_size();
        assert_eq!(logical.width * EXPORT_SCALE, EXPORT_WIDTH as f32);
        assert_eq!(logical.height * EXPORT_SCALE, EXPORT_HEIGHT as f32);
    }
}
