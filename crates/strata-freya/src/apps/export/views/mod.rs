//! The Export window's views: the title bar, the scrolling body, and the footer.
//!
//! The body's order is the canvas's, and the PREVIEW's place in it is deliberate: it used to
//! sit in a right-hand column beside the options, and moved to a **full-width row at the
//! bottom** when partitioning grew its two-pane picker, which needs the width.

mod footer;
mod formats;
mod options;
mod partition;
mod title_bar;

use freya::prelude::*;

pub use footer::Footer;
pub use title_bar::TitleBar;

use crate::apps::export::views::formats::Formats;
use crate::apps::export::views::options::Options;
use crate::apps::export::views::partition::Partition;
use crate::apps::export::{preview, ExportCtx, ExportThemePartial, ExportThemePreference};
use crate::components::divider::Divider;
use crate::components::typography::{Eyebrow, Readout};

/// The window body's inset (canvas `padding: var(--sp-5)`), and the gap between its sections.
const BODY_PADDING: Gaps = Gaps::new(16., 16., 16., 16.);
const SECTION_SPACING: f32 = 24.;

/// Everything between the title bar and the footer, scrolling as one — the format cards, the
/// option list, the partition picker, then the preview.
#[derive(PartialEq)]
pub struct ExportBody;

impl Component for ExportBody {
    fn render(&self) -> impl IntoElement {
        rect().width(Size::fill()).height(Size::flex(1.)).child(
            ScrollView::new()
                .width(Size::fill())
                .height(Size::fill())
                .child(
                    rect()
                        .width(Size::fill())
                        .vertical()
                        .spacing(SECTION_SPACING)
                        .padding(BODY_PADDING)
                        .child(Formats)
                        .child(Options)
                        .child(Partition)
                        .child(Preview),
                ),
        )
    }
}

/// The PREVIEW pane — what the chosen options will actually produce, over a rule that
/// separates it from the controls above.
#[derive(PartialEq)]
struct Preview;

impl Component for Preview {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let ctx = use_consume::<ExportCtx>();
        // Subscribes to both: every edit re-renders the preview, which is the point.
        let text = preview::build(&ctx.draft.read(), &ctx.target.read());

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(12.)
            .child(Divider::horizontal().color(theme.border_fill))
            // The canvas puts an estimated output size on the right of this header. It is
            // computed from invented compression factors, so it isn't here — see `footer`.
            .child(Eyebrow::new("PREVIEW").color(theme.label_color))
            .child(
                rect()
                    .width(Size::fill())
                    .padding((12., 12.))
                    .corner_radius(6.)
                    .background(theme.panel_background)
                    .border(Border::new().width(1.).fill(theme.border_fill))
                    .child(Readout::new(text).color(theme.card_color).wrap()),
            )
    }
}
