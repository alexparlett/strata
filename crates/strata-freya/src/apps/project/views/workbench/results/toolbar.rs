use crate::components::icon::{Icon, IconName};
use freya::components::use_theme;
use freya::prelude::*;

/// The editor query toolbar, built to the comp. The bar itself only needs the editor surface (its
/// background) and the divider colour. The Run control is its own three-state `RunButton`; the rest
/// are outline [`Button`]s wrapping an icon (the rationalised button model — no bespoke IconButton).
/// Actions are stubbed until the query / editor-command layers land.
#[derive(PartialEq)]
pub struct DataGridToolbar;

impl Component for DataGridToolbar {
    fn render(&self) -> impl IntoElement {
        let theme = use_theme();
        let bg = {
            let t = theme.read();
            t.colors.background
        };

        // An outline icon button — `outline_button` variant with a centred icon. (Icon keeps its
        // resting tint on hover; Freya's Button doesn't cascade its hover colour into an SvgViewer.)
        let tool = move |icon: IconName| {
            Button::new()
                .height(Size::px(28.))
                .width(Size::px(28.))
                .child(Icon::new(icon).size(15.))
        };

        let row = rect()
            .width(Size::fill())
            .height(Size::px(38.))
            .horizontal()
            .cross_align(Alignment::Center)
            .main_align(Alignment::End)
            .spacing(8.)
            .padding((0., 10.))
            .background(bg)
            .child(tool(IconName::Search))
            .child(tool(IconName::Reload))
            .child(tool(IconName::Trash))
            .child(tool(IconName::Download));

        rect()
            .width(Size::fill())
            .vertical()
            .child(row)
    }
}
