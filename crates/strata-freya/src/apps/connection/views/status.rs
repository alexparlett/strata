//! The busy and failure blocks, at the **end** of the body — the Configure window's, over a
//! connection instead of a table.
//!
//! There is no success block: success is the window closing. And there is no pre-flight probe
//! either: `engine::store::connect` resolves the credential chain once before it registers
//! anything, so the answer this waits for *is* the probe. A second check here would be a request
//! to the bucket asking a question the pass has already answered.
//!
//! The failure text is whatever the engine returned. `engine::store` writes those messages for
//! every caller — "This S3 connection needs a region", "The AWS profile 'analytics' resolved no
//! credentials: …" — and this window must not grow a second set.

use freya::components::CircularLoader;
use freya::prelude::*;

use crate::apps::connection::{ConnectionCtx, Status};
use crate::components::icon::{Icon, IconName};
use crate::components::tones::tones;
use crate::components::typography::{Path, Readout, Strong};
use crate::components::window::window_theme;
use crate::theme::{use_roles, Role};

/// The blocks' inset and the gap inside them (canvas `padding: var(--sp-4) var(--sp-5)`).
const BLOCK_PADDING: Gaps = Gaps::new(12., 16., 12., 16.);
const BLOCK_GAP: f32 = 12.;
const GLYPH: f32 = 14.;

#[derive(PartialEq)]
pub struct StatusBlock;

impl Component for StatusBlock {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let error = tones().error;
        let text = use_roles().get(Role::TextMuted);
        let ctx = use_consume::<ConnectionCtx>();
        let status = ctx.status.read().clone();

        match status {
            Status::Idle => rect(),
            Status::Connecting(url) => rect()
                .width(Size::fill())
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(BLOCK_GAP)
                .padding(BLOCK_PADDING)
                .corner_radius(8.)
                .background(win.panel_background)
                .border(Border::new().width(1.).fill(win.border_fill))
                .child(CircularLoader::new().size(GLYPH))
                .child(Path::new(format!("Connecting to '{url}'…")).color(text)),
            // A failure is a *sentence the engine wrote*, so it gets room to wrap rather than a
            // single clipped line — a credential-chain refusal is two clauses long, and this is
            // the one place it can be read beside the field that caused it.
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
                        .child(Strong::new("Couldn't connect").color(error))
                        .child(Readout::new(why).color(error).width(Size::fill()).wrap()),
                ),
        }
    }
}
