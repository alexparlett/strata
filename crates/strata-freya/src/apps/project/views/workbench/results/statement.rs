use freya::prelude::*;
use strata_engine::StmtKind;

use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{R_2, SP_4, SP_6};
use crate::components::tones::tones;
use crate::components::typography::{Readout, Title};
use crate::theme::{use_roles, Role};

/// The results pane after an **intercepted statement** ran (ED-02): the empty-state layout in
/// success dress — a rounded icon tile over what ran, then the engine's own sentence.
///
/// No grid and no pager, because there is nothing to page: a statement returns no rows. The
/// tab's previous snapshot is not retired either (`SNAPSHOT_SPEC` §4), so pressing Run on a
/// `SELECT` again brings the grid straight back from the result that was already there.
#[derive(PartialEq)]
pub struct StatementState {
    kind: StmtKind,
    message: String,
}

impl StatementState {
    pub fn new(kind: StmtKind, message: String) -> Self {
        Self { kind, message }
    }
}

impl Component for StatementState {
    fn render(&self) -> impl IntoElement {
        let roles = use_roles();
        let (tile_bg, tile_border, icon_color, title_color, msg_color, background) = (
            roles.get(Role::ElementBackground),
            roles.get(Role::Border),
            tones().ok,
            roles.get(Role::TextMuted),
            roles.get(Role::TextPlaceholder),
            roles.get(Role::SurfaceRaised),
        );

        rect()
            .width(Size::fill())
            .height(Size::flex(1.))
            .vertical()
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .spacing(SP_4)
            .padding((0., SP_6))
            .background(background)
            .child(
                rect()
                    .width(Size::px(46.))
                    .height(Size::px(46.))
                    .corner_radius(R_2)
                    .background(tile_bg)
                    .border(Border::new().width(1.).fill(tile_border))
                    .center()
                    .child(Icon::new(IconName::Check).color(icon_color).size(22.)),
            )
            .child(Title::new(self.kind.label()).color(title_color))
            .child(
                Readout::new(self.message.clone())
                    .color(msg_color)
                    .max_width(Size::px(560.))
                    .wrap(),
            )
    }
}
