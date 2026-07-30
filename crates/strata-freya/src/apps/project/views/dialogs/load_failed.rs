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
use crate::components::dialog::{Dialog, DialogHeader};
use crate::components::icon::IconName;
use crate::components::typography::{Control, Prose, Title};
use crate::platform::{close_this_window, is_quitting, OpenCtx};
use crate::state::{write_config, AppCtx, ConfigChan};

/// [`ProjectRoot`](crate::apps::project)'s fault arm: the whole subtree while the project
/// could not load. Esc and the backdrop deliberately do nothing — there is no state behind
/// the dialog to return to — so the ways out are Try again, the close (button, Enter, red
/// button, ⇧⌘W), and ⌘O / File ▸ Open…, which re-roots this window to another project.
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

        // A broken project must not come back on the next launch once this window is
        // *deliberately* closed — the P4-01 rule every project window keeps. The fault arm
        // never claimed the project (`use_open_project` is the loaded arm's), but a previous
        // healthy session can have left it in the persisted open-set, so leaving removes it
        // (idempotent) — except on a quit, which preserves the open-set for every window
        // alike, so the reopen resurfaces the fault, which is honest. A re-root or Try again
        // also lands here first; a load that then succeeds re-adds the project in the same
        // breath.
        {
            let config = self.app.config;
            let path = self.root.to_string_lossy().into_owned();
            use_drop(move || {
                if is_quitting() {
                    return;
                }
                write_config(config, &[ConfigChan::Open], |cfg| cfg.remove_open(&path));
            });
        }

        // Drain the close-confirm slot (see the field doc). A `Window` close was asked with
        // nothing here to protect, so it just proceeds; a parked re-root is performed — the
        // T2 question it was gated on is about work this window no longer shows. The other
        // two variants have no writer on the fault arm; clearing keeps the slot from
        // carrying them into the next mount.
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
        rect().maybe_child((!prompt_up).then(move || {
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
    }
}
