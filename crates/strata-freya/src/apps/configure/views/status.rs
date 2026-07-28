//! The busy and failure blocks, at the **end** of the body.
//!
//! There is no success block: success is the window closing. And there is deliberately no
//! pre-flight readout of file counts or schema consistency (D9) — the Register *is* the check,
//! and a readout that guessed ahead of it would be exactly the invented figures the inspector
//! rejected.
//!
//! The failure text is whatever `register_external` returned. P3-07 maps those messages inside
//! the engine so every caller inherits them; this window must not grow a second set.

use freya::components::CircularLoader;
use freya::prelude::*;

use crate::apps::configure::ConfigureCtx;
use crate::apps::configure::Status;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Path, Readout, Strong};
use crate::components::window::window_theme;

/// The blocks' inset and the gap inside them (canvas `padding: var(--sp-4) var(--sp-5)`).
const BLOCK_PADDING: Gaps = Gaps::new(12., 16., 12., 16.);
const BLOCK_GAP: f32 = 12.;
const GLYPH: f32 = 14.;

#[derive(PartialEq)]
pub struct StatusBlock;

impl Component for StatusBlock {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let error = use_theme().read().colors().error;
        let ctx = use_consume::<ConfigureCtx>();
        let status = ctx.status.read().clone();

        match status {
            Status::Idle => rect(),
            Status::Registering(name) => rect()
                .width(Size::fill())
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(BLOCK_GAP)
                .padding(BLOCK_PADDING)
                .corner_radius(8.)
                .background(win.panel_background)
                .border(Border::new().width(1.).fill(win.border_fill))
                .child(CircularLoader::new().size(GLYPH))
                .child(Path::new(format!("Registering '{name}'…")).color(win.busy_color)),
            // A failure is a *sentence the engine wrote*, so it gets room to wrap rather than a
            // single clipped line — several of P3-07's messages are two clauses long.
            Status::Failed(why) => rect()
                .width(Size::fill())
                .horizontal()
                .cross_align(Alignment::Start)
                .spacing(BLOCK_GAP)
                .padding(BLOCK_PADDING)
                .corner_radius(8.)
                .background(win.panel_background)
                .border(Border::new().width(1.).fill(error))
                .child(Icon::new(IconName::Alert).size(GLYPH).color(error))
                .child(
                    rect()
                        .width(Size::flex(1.))
                        .vertical()
                        .spacing(4.)
                        .child(Strong::new("Couldn't register table").color(error))
                        .child(Readout::new(why).color(error).width(Size::fill()).wrap()),
                ),
        }
    }
}
