//! The Export window's views: the title bar, the scrolling body, and the footer.
//!
//! The body's order is the canvas's, and the PREVIEW's place in it is deliberate: it used to
//! sit in a right-hand column beside the options, and moved to a **full-width row at the
//! bottom** when partitioning grew its two-pane picker, which needs the width.

mod footer;
mod formats;
mod partition;
mod title_bar;

use freya::prelude::*;

pub use footer::Footer;
pub use title_bar::TitleBar;

use crate::apps::export::views::formats::Formats;
use crate::apps::export::views::partition::Partition;
use crate::apps::export::{preview, ExportCtx, ExportThemePartial, ExportThemePreference};
use crate::components::divider::Divider;
use crate::components::form::OptionList;
use crate::components::metrics::{R_1, SP_4, SP_5, SP_6};
use crate::components::typography::{Eyebrow, Readout};

/// The window body's inset (canvas `padding: var(--sp-5)`), and the gap between its sections.
const BODY_PADDING: Gaps = Gaps::new(SP_5, SP_5, SP_5, SP_5);
const SECTION_SPACING: f32 = SP_6;

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

/// This window's option list — the format's groups, rendered by the shared vocabulary.
///
/// **Flat: there is no ADVANCED disclosure.** The canvas folded it away on the grounds that a
/// format's advanced controls are just more of that format's options. The Configure window's
/// canvas kept one and it was built that way first; it was then flattened to match this, because
/// the reasoning is not specific to exporting. Neither window has a disclosure, so neither does
/// [`OptionList`].
#[derive(PartialEq)]
struct Options;

impl Component for Options {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<ExportCtx>();
        // Both reads subscribe: a format switch or any edit rebuilds the list, which is the
        // point — the Parquet level group appears and disappears with the codec.
        let (scope, groups) = {
            let draft = ctx.draft.read();
            (draft.format.name(), draft.groups(&ctx.target.read()))
        };
        OptionList::new(scope, groups, move |edit| {
            ctx.edit(|draft| draft.apply(edit));
        })
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
            .spacing(SP_4)
            .child(Divider::horizontal().color(theme.border_fill))
            // The canvas puts an estimated output size on the right of this header. It is
            // computed from invented compression factors, so it isn't here — see `footer`.
            .child(Eyebrow::new("PREVIEW").color(theme.label_color))
            .child(
                rect()
                    .width(Size::fill())
                    .padding((SP_4, SP_4))
                    .corner_radius(R_1)
                    .background(theme.panel_background)
                    .border(Border::new().width(1.).fill(theme.border_fill))
                    .child(Readout::new(text).color(theme.card_color).wrap()),
            )
    }
}
