//! The project-load fault (P4-01): what a window shows when its `.strata/project.json` or
//! `session.json` could not be loaded. The window cannot exist without them — there is no
//! store to mount and nothing to render behind the card — so the dialog states the fault
//! and offers the two honest moves: **Try again** (an [`EngineRestart`] bump, which remounts
//! the subtree and re-runs the load — the file may have been fixed, or the failure was
//! transient) and **Close window**, through the shared close path (the launcher takes its
//! place when it was the app's last). The load itself, and what counts as unrecoverable, is
//! `state::hooks::open_project`'s.
//!
//! Unlike its siblings this dialog is **not modal** (`Dialog::modal(false)`): it *is* the
//! window's whole content, with no feature listeners behind it to protect, and the window
//! commands must keep working — the menubar's Open… and Settings items arrive as synthesized
//! key presses, so a modal barrier here would swallow ⌘O and ⌘, and leave the user no way to
//! point the window somewhere useful.

use std::path::PathBuf;

use freya::components::use_theme;
use freya::prelude::*;
use strata_core::util::folder_name;

use crate::apps::project::close::CloseTarget;
use crate::apps::project::state::EngineRestart;
use crate::apps::project::views::WindowDragStrip;
use crate::components::dialog::{Dialog, DialogHeader};
use crate::components::icon::IconName;
use crate::components::typography::{Control, Prose, Title};
use crate::platform::{close_this_window, OpenCtx};
use crate::state::{use_claim_open, AppCtx};

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
    /// The window's close-confirm slot. The fault arm mounts no `CloseConfirm`, but the
    /// slot's writers still fire — `guard.running` can be true here, because a run in
    /// flight when the window re-rooted into this broken project keeps the old engine
    /// alive until it settles — so the slot is drained below rather than left to no-op
    /// the red button and then pop a stale confirm on the next successful mount.
    pub confirm: State<Option<CloseTarget>>,
    /// The window's fill mark, for the drag strip — the fault arm still owes the window
    /// its chrome (the traffic lights sit in the same corner either way).
    pub filled_by_app: State<bool>,
    pub app: AppCtx,
}

impl Component for ProjectLoadFailed {
    fn render(&self) -> impl IntoElement {
        // Taken in the render scope so the handlers can run from a task (the close_confirm
        // pattern).
        let platform = use_hook(Platform::get);
        let theme = use_theme();
        let open = use_consume::<OpenCtx>();
        let restart = use_consume::<EngineRestart>();
        let confirm = self.confirm;
        // Pressed once: the close is several async hops (it may stand the launcher up
        // before this window goes), and a dialog that cannot dismiss itself can be pressed
        // again in that gap — a second press must not close a second window.
        let closing = use_state(|| false);
        let close = {
            let app = self.app.clone();
            let platform = platform.clone();
            move || {
                let mut closing = closing;
                if *closing.peek() {
                    return;
                }
                closing.set(true);
                // `spawn_forever`, not `spawn`: the close unmounts the very scope this
                // handler belongs to — the whole window goes — and scope teardown drops
                // that scope's tasks before they are ever polled. See the same note in
                // `close_confirm`.
                spawn_forever(close_this_window(platform.clone(), app.clone()));
            }
        };

        // The fault arm is still a window on this project, so it claims the open-set like
        // any project window — a quit reopens it (resurfacing the fault, which is honest)
        // and a deliberate close drops it from reopen-on-startup — while withholding the
        // recents promotion that is `use_open_project`'s other half: a project that doesn't
        // open must not head that list. The add half is load-bearing, not symmetry: a
        // failed Try again remounts this arm, and a remove-on-drop alone would evict the
        // entry with nothing re-adding it — the quit after that failed retry would silently
        // forget the window.
        use_claim_open(self.app.config, &self.root);

        // …and tells the open path so: while this is set, naming this window's own project
        // (⌘O, Open Recent, the switcher) is a retry rather than the usual no-op — the one
        // reading of "open the project I am already looking at" that makes sense when what
        // the user is looking at is its load error. Mount/drop rather than a render read:
        // the flag belongs to the window (`OpenCtx`), which outlives this arm.
        {
            let mut faulted = open.faulted;
            use_hook(move || faulted.set(true));
            use_drop(move || {
                let mut faulted = faulted;
                faulted.set(false);
            });
        }

        // Drain the close-confirm slot (see the field doc), acting rather than re-asking.
        // Not a silent abort: `guard.running` can only be true here for runs orphaned by
        // the confirmed stop that put this arm up — the re-root or restart that replaced
        // the subtree asked the T2 question and the user answered it (or their pref asked
        // never to be asked, which also gates every writer of this slot). The engine's
        // deferred `Drop` merely hasn't finished honouring that answer yet, so a second
        // confirm would re-ask about work already condemned. AGENTS.md §2 records this.
        // The other two variants have no writer on the fault arm; clearing keeps the slot
        // from carrying them into the next mount.
        {
            let close = close.clone();
            use_side_effect(move || {
                // Read into a value first — the close_confirm borrow rule: a match on the
                // guard's temporary would hold the read borrow across the `set`.
                let target = confirm.read().clone();
                let Some(target) = target else {
                    return;
                };
                let mut confirm = confirm;
                confirm.set(None);
                match target {
                    CloseTarget::Window => close(),
                    CloseTarget::Reroot(root) => open.reroot_confirmed(root),
                    CloseTarget::Tab(_) | CloseTarget::Restart => {}
                }
            });
        }

        let try_again = move || {
            if *closing.peek() {
                return;
            }
            // The generation bump remounts `ProjectRoot`, which re-runs the load in a fresh
            // scope — the same mechanism an engine restart uses, with no engine to rebuild.
            restart.restart();
        };

        // The folder name identifies the project; the full path is in the error body.
        let name = folder_name(&self.root);
        let c = theme.read().colors().clone();

        let header = DialogHeader::new(
            IconName::Warning,
            c.error,
            rect()
                .vertical()
                .child(Title::new("Cannot open project").color(c.text_primary))
                .child(
                    Prose::new(name)
                        .color(c.text_placeholder)
                        .text_overflow(TextOverflow::Ellipsis),
                ),
        );

        // While the This/New prompt is up (an Open Recent from this window, pref = Ask),
        // this dialog stands down: the prompt is mounted *before* the project subtree, so
        // its card would otherwise paint underneath this one — visible barrier, invisible
        // question — while its earlier key barrier answered Enter. One question on screen
        // at a time; the prompt clearing brings the fault back.
        let prompt_up = open.prompt.read().is_some();

        // Enter and the button each take their own clone of the close (it captures the
        // `AppCtx` the launcher hand-off needs, so it isn't `Copy`).
        let close_enter = close.clone();
        let error = self.error.clone();
        rect()
            .maybe_child((!prompt_up).then(move || {
                Dialog::new()
                    .modal(false)
                    // Enter is the keyboard close; every other chord stays the window's
                    // (⌘O, ⌘,, ⇧⌘W — see the module doc).
                    .on_confirm(move |_| close_enter())
                    .header(header)
                    .body(
                        rect()
                            .width(Size::fill())
                            .child(Prose::new(error).color(c.text_secondary).wrap()),
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
            // The window's drag strip: the fault arm replaces the whole subtree, HeaderBar
            // included, but the OS traffic lights still sit in this corner and the window
            // must stay movable (a fault window restored onto a detached monitor has no
            // other way back). Mounted AFTER the dialog on purpose — an overlay node at the
            // same depth, later in document order, paints and hit-tests above the dialog's
            // backdrop, which would otherwise swallow the drag press.
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
