//! The data source editor's views: the title bar, the scrolling body, and the footer.

mod footer;
mod form;
mod status;
mod title_bar;

use freya::components::ScrollView;
use freya::prelude::*;
use strata_engine::RegStatus;

pub use footer::Footer;
pub use title_bar::TitleBar;

use crate::apps::project::use_registrations;
use crate::apps::source::views::form::Fields;
use crate::apps::source::views::status::StatusBlock;
use crate::apps::source::{SourceCtx, Status};
use crate::components::form::Form;
use crate::components::metrics::SP_5;

/// The window body's inset (canvas `padding: var(--sp-5)`). The gap *between* sections is the
/// form's own `ROW_GAP` — this body is a [`Form`], so it does not get to invent one.
const BODY_PADDING: Gaps = Gaps::new(SP_5, SP_5, SP_5, SP_5);

/// Everything between the title bar and the footer, scrolling as one.
#[derive(PartialEq)]
pub struct SourceBody;

impl Component for SourceBody {
    fn render(&self) -> impl IntoElement {
        rect().width(Size::fill()).height(Size::flex(1.)).child(
            ScrollView::new()
                .width(Size::fill())
                .height(Size::fill())
                .child(
                    rect()
                        .width(Size::fill())
                        .padding(BODY_PADDING)
                        .child(Form::new().child(Fields).child(StatusBlock)),
                ),
        )
    }
}

/// Watch the engine's answer for the data source this window is waiting on, and settle the save.
///
/// The registration itself belongs to the project window's one scan driver — this is a
/// **reconciliation over the window's view of the engine's ledger**, not an await on a second
/// registration path. While the status is `Connecting`, the answer stamped past the generation
/// Save asked at is the verdict: `Ready` means the data source went in and this window's work is
/// done, `Failed` brings the engine's own reason back to the body, and no answer yet is simply
/// "not yet".
///
/// Two things make this safe to mount unconditionally — the status gate, and the generation: an
/// existing data source is already `Ready` in the ledger when the window opens, so a status read
/// alone would close the window at mount.
pub fn use_watch_source(mut ctx: SourceCtx) {
    let registrations = use_registrations();
    let platform = use_hook(Platform::get);

    use_side_effect(move || {
        let Status::Connecting { name, asked_at } = ctx.status.read().clone() else {
            return;
        };
        match registrations.read().sources.answered_since(&name, asked_at) {
            None => {}
            Some(RegStatus::Ready) => platform.close_current_window(),
            Some(RegStatus::Failed { reason, .. }) => {
                ctx.status.set(Status::Failed(reason.clone()));
            }
        }
    });
}
