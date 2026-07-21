use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::run_button::{RunButton, RunState};
use freya::components::use_theme;
use freya::prelude::*;

/// The editor query toolbar, built to the comp. The bar itself only needs the editor surface (its
/// background) and the divider colour. The Run control is its own three-state `RunButton`; the rest
/// are outline [`Button`]s wrapping an icon (the rationalised button model — no bespoke IconButton).
/// Actions are stubbed until the query / editor-command layers land.
#[derive(PartialEq)]
pub struct EditorToolbar;

impl Component for EditorToolbar {
    fn render(&self) -> impl IntoElement {
        let theme = use_theme();
        let (bg, border) = {
            let t = theme.read();
            (t.colors.background, t.colors.border)
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
            .spacing(8.)
            .padding((0., 10.))
            .background(bg)
            .child(RunButton::new(RunState::Idle))
            .child(tool(IconName::Explain))
            .child(tool(IconName::Analyze))
            .child(Divider::vertical().length(Size::px(18.)).color(border))
            .child(tool(IconName::Format))
            .child(tool(IconName::Trash))
            .child(Divider::vertical().length(Size::px(18.)).color(border))
            .child(tool(IconName::Eye))
            .child(tool(IconName::Save));

        rect()
            .width(Size::fill())
            .vertical()
            .child(row)
            .child(Divider::horizontal().color(border))
    }
}
