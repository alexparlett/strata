//! The connection editor's views: the title bar, the scrolling body, and the footer.

mod footer;
mod form;
mod status;
mod title_bar;

use freya::components::ScrollView;
use freya::prelude::*;
use freya::radio::use_radio;

pub use footer::Footer;
/// The client-option table's key column, for the test that pins the header to it — see
/// `interaction::the_client_options_header_stands_at_the_split_it_declares`.
#[cfg(test)]
pub use form::OPTION_KEY_WIDTH;
pub use title_bar::TitleBar;

use crate::apps::connection::views::form::Fields;
use crate::apps::connection::views::status::StatusBlock;
use crate::apps::connection::{ConnectionCtx, Status};
use crate::apps::project::{ProjChan, ProjectState, Reg};
use crate::components::form::Form;
use crate::components::metrics::SP_5;

/// The window body's inset (canvas `padding: var(--sp-5)`). The gap *between* sections is the
/// form's own `ROW_GAP` — this body is a [`Form`], so it does not get to invent one.
const BODY_PADDING: Gaps = Gaps::new(SP_5, SP_5, SP_5, SP_5);

/// Everything between the title bar and the footer, scrolling as one.
#[derive(PartialEq)]
pub struct ConnectionBody;

impl Component for ConnectionBody {
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

/// Watch the connection row this window is waiting on, and settle the save.
///
/// The registration itself belongs to the project window's one scan driver — this is a
/// **reconciliation over the shared store**, not an await on a second registration path. While
/// the status is `Connecting(url)`, that row's `Reg` is the answer: `Ready` means the object
/// store went in and this window's work is done, `Failed` brings the engine's own reason back to
/// the body. `Loading` is simply "not yet".
///
/// The status gate is what makes this safe to mount unconditionally: an existing connection's row
/// is already settled when the window opens, and without the gate that would close the window at
/// mount.
pub fn use_watch_connection(mut ctx: ConnectionCtx) {
    let project = use_radio::<ProjectState, ProjChan>(ProjChan::Connections);
    let platform = use_hook(Platform::get);

    use_side_effect(move || {
        let Status::Connecting(url) = ctx.status.read().clone() else {
            return;
        };
        let answer = project.read().connections.iter().find_map(|row| {
            (row.def.url() == url).then(|| match &row.reg {
                Reg::Loading => None,
                Reg::Ready(()) => Some(Ok(())),
                Reg::Failed(why) => Some(Err(why.clone())),
            })
        });
        match answer.flatten() {
            None => {}
            Some(Ok(())) => platform.close_current_window(),
            Some(Err(why)) => ctx.status.set(Status::Failed(why)),
        }
    });
}
