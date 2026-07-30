//! The project-load **loading** arm: what a window shows while its `.strata/project.json` and
//! `session.json` are being read.
//!
//! It exists because that read is blocking `std::fs` on files the user named, and Freya is one
//! event loop drawing every window — so the read runs off the render thread
//! ([`load_project`](crate::apps::project::state::load_project)) and this is the third thing
//! `ProjectRoot` can be while it is out there. A project on a mount that stopped answering now
//! costs one parked thread and one window that says so, instead of the whole app.
//!
//! **Nothing is drawn for a load nobody could perceive.** A local project opens in a millisecond
//! or two, and a spinner that flashes on every open is worse than no spinner at all — so the arm
//! renders the window's background alone until [`SLOW_LOAD`] has passed, and only then says what
//! it is waiting for and offers the way out.
//!
//! The way out is **Close window**, not Cancel — an `outline` button, because on a surface with
//! nothing else on it a flat control reads as a label: a blocking syscall cannot be interrupted (see
//! [`offload`](crate::task::offload)), so there is no honest button for stopping the read. What
//! the user can do is stop waiting for it, which is the same close the fault arm offers and the
//! same wording. Beyond it, ⌘O re-roots the window somewhere useful and ⌘, still opens Settings
//! — this arm mounts no key barrier, for the reason the fault dialog spells out.
//!
//! Unlike the fault arm this does **not** stand down while the This/New prompt is up: that
//! stand-down is about two `Dialog`s, which share `Layer::Overlay` and so settle by document
//! order — the fault card, mounted later, would paint over the question. Plain window content
//! loses to an overlay outright, so the prompt covers this of its own accord.

use std::path::PathBuf;
use std::time::Duration;

use async_io::Timer;
use freya::components::use_theme;
use freya::prelude::*;
use strata_core::util::folder_name;

use crate::apps::project::close::{use_engineless_close, CloseTarget};
use crate::apps::project::views::WindowDragStrip;
use crate::components::typography::{Control, Title};
use crate::state::AppCtx;

/// How long a load may take before the window admits to being busy.
///
/// Long enough that a healthy project — two small reads off a local disk — comes and goes without
/// ever painting this, short enough that a wedged mount does not leave an empty window with no
/// account of itself. It is a presentation delay and nothing waits on it: the load is already
/// running, and the arm swaps the moment it answers, whether or not this has elapsed.
const SLOW_LOAD: Duration = Duration::from_millis(600);

/// [`ProjectRoot`](crate::apps::project)'s loading arm: the whole subtree while the project is
/// being read off disk. It holds no engine, no store and no [`Subtree`] — there is nothing
/// loaded yet to build one from — so a child window can neither be opened from here nor be
/// handed a handle that would outlive this mount.
///
/// [`Subtree`]: crate::platform::Subtree
#[derive(PartialEq)]
pub struct ProjectLoading {
    /// The folder being opened — the subject line, once there is reason to show one.
    pub root: PathBuf,
    /// The window's close-confirm slot, drained rather than rendered — see
    /// [`use_engineless_close`].
    pub confirm: State<Option<CloseTarget>>,
    /// The window's fill mark, for the drag strip: this arm owes the window its chrome exactly
    /// as the fault arm does.
    pub filled_by_app: State<bool>,
    pub app: AppCtx,
}

impl Component for ProjectLoading {
    fn render(&self) -> impl IntoElement {
        let theme = use_theme();
        let (close, _closing) = use_engineless_close(self.app.clone(), self.confirm);

        // Say nothing about a load that finishes before anyone could read it. Scope-bound, so a
        // load that lands first takes the timer with it when this arm goes.
        let mut slow = use_state(|| false);
        use_hook(move || {
            spawn(async move {
                Timer::after(SLOW_LOAD).await;
                slow.set(true);
            });
        });

        let name = folder_name(&self.root);
        let c = theme.read().colors().clone();

        // No spacing on this one: its only in-flow child is the card below (the drag strip is
        // globally positioned, so torin leaves it out of the flow entirely), and a gap value that
        // can never apply reads as a gap someone chose.
        rect()
            .expanded()
            .vertical()
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .maybe_child(slow.read().then(|| {
                rect()
                    .vertical()
                    .cross_align(Alignment::Center)
                    .spacing(16.)
                    .child(CircularLoader::new().size(28.))
                    .child(Title::new(format!("Opening '{name}'")).color(c.text_primary))
                    .child(
                        Button::new()
                            .outline()
                            .on_press(move |_| close())
                            .child(Control::new("Close window")),
                    )
            }))
            // The window's drag strip, for the reason the fault arm keeps one: this arm replaces
            // the whole subtree, HeaderBar included, but the OS traffic lights still sit in that
            // corner and a window has to stay movable. An overlay at global position, so it
            // hit-tests above the column above.
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
