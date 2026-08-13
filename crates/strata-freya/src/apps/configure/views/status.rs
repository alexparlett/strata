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
use crate::components::metrics::{R_2, SP_2, SP_4, SP_5, STATUS_GLYPH};
use crate::components::tones::tones;
use crate::components::typography::{Path, Readout, Strong};
use crate::components::window::window_theme;
use crate::theme::{use_roles, Role};

/// The blocks' inset and the gap inside them (canvas `padding: var(--sp-4) var(--sp-5)`).
const BLOCK_PADDING: Gaps = Gaps::new(SP_4, SP_5, SP_4, SP_5);
const BLOCK_GAP: f32 = SP_4;

#[derive(PartialEq)]
pub struct StatusBlock;

impl Component for StatusBlock {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let error = tones().error;
        let text = use_roles().get(Role::TextMuted);
        let ctx = use_consume::<ConfigureCtx>();
        let status = ctx.status.read().clone();

        let busy = match &status {
            Status::Creating(name) => Some(format!("Creating '{name}'…")),
            Status::Registering(name) => Some(format!("Registering '{name}'…")),
            _ => None,
        };

        match status {
            Status::Idle => rect(),
            Status::Creating(_) | Status::Registering(_) => rect()
                .width(Size::fill())
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(BLOCK_GAP)
                .padding(BLOCK_PADDING)
                .corner_radius(R_2)
                .background(win.panel_background)
                .border(Border::new().width(1.).fill(win.border_fill))
                .child(CircularLoader::new().size(STATUS_GLYPH))
                .child(Path::new(busy.unwrap_or_default()).color(text)),
            Status::Failed(why) => rect()
                .width(Size::fill())
                .horizontal()
                .cross_align(Alignment::Start)
                .spacing(BLOCK_GAP)
                .padding(BLOCK_PADDING)
                .corner_radius(R_2)
                .background(win.panel_background)
                .border(Border::new().width(1.).fill(error))
                .child(Icon::new(IconName::Alert).size(STATUS_GLYPH).color(error))
                .child(
                    rect()
                        .width(Size::flex(1.))
                        .vertical()
                        .spacing(SP_2)
                        .child(Strong::new("Couldn't register table").color(error))
                        .child(Readout::new(why).color(error).width(Size::fill()).wrap()),
                ),
        }
    }
}
