//! The project-load fault (P4-01): what a window shows when its `.strata/project.json` or
//! `session.json` could not be loaded. The window cannot exist without them — there is no
//! store to mount and nothing to render behind the card — so the dialog states the fault
//! and offers the two honest moves: **Try again** (an [`EngineRestart`] bump, which remounts
//! the subtree and re-runs the load — the file may have been fixed, or the failure was
//! transient) and **Close window**, through the shared close path (the launcher takes its
//! place when it was the app's last). The load itself, and what counts as unrecoverable, is
//! `state::hooks::open_project`'s; running it off the render thread is
//! [`load_project`](crate::apps::project::state::load_project)'s, which is why Try again can no
//! longer re-enter a blocking read — the arm it remounts into is
//! [`ProjectLoading`](crate::apps::project::views::ProjectLoading).
//!
//! Unlike its siblings this dialog is **not modal** (`Dialog::modal(false)`): it *is* the
//! window's whole content, with no feature listeners behind it to protect, and the window
//! commands must keep working — the menubar's Open… and Settings items arrive as synthesized
//! key presses, so a modal barrier here would swallow ⌘O and ⌘, and leave the user no way to
//! point the window somewhere useful.

use std::path::PathBuf;

use freya::prelude::*;
use strata_core::util::folder_name;

use crate::apps::project::close::{use_engineless_close, CloseTarget};
use crate::apps::project::state::EngineRestart;
use crate::apps::project::views::WindowDragStrip;
use crate::components::dialog::{Dialog, DialogHeader};
use crate::components::icon::IconName;
use crate::components::tones::tones;
use crate::components::typography::{Control, Prose, Title};
use crate::platform::OpenCtx;
use crate::state::AppCtx;
use crate::theme::{use_roles, Role};

/// [`ProjectRoot`](crate::apps::project)'s fault arm: the whole subtree while the project
/// could not load. Esc and the backdrop deliberately do nothing — there is no state behind
/// the dialog to return to — so the ways out are Try again, the close (button, Enter, red
/// button, ⇧⌘W), and ⌘O / File ▸ Open…, which re-roots this window to another project — or,
/// pointed at this window's own project, retries it ([`OpenCtx::faulted`]).
#[derive(PartialEq)]
pub struct ProjectLoadFailed {
    /// The folder that failed to open — the dialog's subject line.
    pub root: PathBuf,
    /// The load error, verbatim: `open_project`'s strings already name the file with its
    /// full path.
    pub error: String,
    /// The window's close-confirm slot, drained rather than rendered — the reasoning, and the
    /// other arm that needs the same, are [`use_engineless_close`]'s.
    pub confirm: State<Option<CloseTarget>>,
    /// The window's fill mark, for the drag strip — the fault arm still owes the window
    /// its chrome (the traffic lights sit in the same corner either way).
    pub filled_by_app: State<bool>,
    pub app: AppCtx,
}

impl Component for ProjectLoadFailed {
    fn render(&self) -> impl IntoElement {
        let roles = use_roles();
        let danger = tones().error;
        let open = use_consume::<OpenCtx>();
        let restart = use_consume::<EngineRestart>();
        let (close, closing) = use_engineless_close(self.app.clone(), self.confirm);

        {
            let mut faulted = open.faulted;
            use_hook(move || faulted.set(true));
            use_drop(move || {
                let mut faulted = faulted;
                faulted.set(false);
            });
        }

        let try_again = move || {
            if *closing.peek() {
                return;
            }
            restart.restart();
        };

        let name = folder_name(&self.root);

        let header = DialogHeader::new(
            IconName::Warning,
            danger,
            rect()
                .vertical()
                .child(Title::new("Cannot open project").color(roles.get(Role::Text)))
                .child(
                    Prose::new(name)
                        .color(roles.get(Role::TextPlaceholder))
                        .text_overflow(TextOverflow::Ellipsis),
                ),
        );

        let prompt_up = open.prompt.read().is_some();

        let close_enter = close.clone();
        let error = self.error.clone();
        rect()
            .maybe_child((!prompt_up).then(move || {
                Dialog::new()
                    .modal(false)
                    .on_confirm(move |()| close_enter())
                    .header(header)
                    .body(
                        rect()
                            .width(Size::fill())
                            .child(Prose::new(error).color(roles.get(Role::TextMuted)).wrap()),
                    )
                    .action(
                        Button::new()
                            .flat()
                            .on_press(move |_| try_again())
                            .child(Control::new("Try again")),
                    )
                    .action(
                        Button::new()
                            .filled()
                            .on_press(move |_| close())
                            .child(Control::new("Close window")),
                    )
            }))
            .child(
                rect()
                    .layer(Layer::Overlay)
                    .position(Position::new_global())
                    .child(WindowDragStrip {
                        filled_by_app: self.filled_by_app,
                    }),
            )
    }
}
