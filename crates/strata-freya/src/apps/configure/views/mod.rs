//! The Configure window's views: the title bar, the scrolling body, and the footer.
//!
//! The body's order is the canvas's, and one thing in it contradicts DEV_TASKS D7: the busy and
//! failure blocks are the **last** things in the body, after Hive, not "below import-options,
//! above Hive". The canvas is newer; it wins.
//!
//! The LOCATION section that opens the canvas is not here at all — see the module doc on why a
//! one-option toggle is not shipped disabled.

mod footer;
mod hive;
mod identity;
mod options;
mod paths;
mod status;
mod title_bar;

use freya::prelude::*;
use freya::radio::use_radio;

pub use footer::Footer;
pub use title_bar::TitleBar;

use crate::apps::configure::views::hive::Hive;
use crate::apps::configure::views::identity::Identity;
use crate::apps::configure::views::options::ImportOptions;
use crate::apps::configure::views::paths::SourcePaths;
use crate::apps::configure::views::status::StatusBlock;
use crate::apps::configure::{ConfigureCtx, Status};
use crate::apps::project::{ProjChan, ProjectState, Reg};

/// The window body's inset (canvas `padding: var(--sp-5)`), and the gap between its sections.
const BODY_PADDING: Gaps = Gaps::new(16., 16., 16., 16.);
const SECTION_SPACING: f32 = 20.;

/// Everything between the title bar and the footer, scrolling as one.
#[derive(PartialEq)]
pub struct ConfigureBody;

impl Component for ConfigureBody {
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
                        .child(Identity)
                        .child(SourcePaths)
                        .child(ImportOptions)
                        .child(Hive)
                        .child(StatusBlock),
                ),
        )
    }
}

/// Watch the catalog row this window is waiting on, and settle the save.
///
/// The registration itself belongs to the project window's one scan driver — this is a
/// **reconciliation over the shared store**, not an await on a second registration path. While
/// the status is `Registering(name)`, the row's `Reg` is the answer: `Ready` means the table is
/// registered and this window's work is done, `Failed` brings the engine's own message back to
/// the footer. `Loading` is simply "not yet".
///
/// The status gate is what makes this safe to mount unconditionally: an existing table's row is
/// already `Ready` when the window opens, and without the gate that would close the window at
/// mount.
pub fn use_watch_registration(mut ctx: ConfigureCtx) {
    // Subscribes to the tables channel — the only thing this window watches over there.
    let project = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
    let platform = use_hook(Platform::get);

    use_side_effect(move || {
        let Status::Registering(name) = ctx.status.read().clone() else {
            return;
        };
        let answer = project.read().tables.iter().find_map(|row| {
            ProjectState::same_name(&row.def.name, &name).then(|| match &row.reg {
                Reg::Loading => None,
                Reg::Ready(_) => Some(Ok(())),
                Reg::Failed(why) => Some(Err(why.clone())),
            })
        });
        match answer.flatten() {
            // Still registering, or the row went while we waited (a drop from the catalog) —
            // either way there is nothing for this window to say.
            None => {}
            Some(Ok(())) => platform.close_current_window(),
            Some(Err(why)) => ctx.status.set(Status::Failed(why)),
        }
    });
}
