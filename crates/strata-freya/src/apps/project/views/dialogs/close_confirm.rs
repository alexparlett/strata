//! The close-while-running confirm (T2), built to the Strata canvas's
//! close-confirmation comp: a 420px elevated card — warning chip + title + name header,
//! body copy, a "Don't ask again" checkbox writing `confirm_close_running` — over a
//! footer strip with a ghost keep button and the red stop action. All copy is the
//! canvas's, per variant (`closeConfirmTitle`/`Body`/`Keep`/`Btn`). The close
//! *mechanics* (the winit `on_close` bridge, `CloseGuard`, `TabCloser`) live in
//! `crate::apps::project::close`.

use crate::state::{use_config, use_config_station, write_config, AppCtx, ConfigChan};
use freya::components::{get_theme, use_theme};
use freya::prelude::*;
use freya::radio::{use_radio, use_radio_station};

use strata_core::util::folder_name;

use crate::apps::project::close::CloseTarget;
use crate::apps::project::state::{Chan, EngineRestart, ProjChan, ProjectState, SessionState};
use crate::apps::project::views::{CancelButtonThemePartial, CancelButtonThemePreference};
use crate::components::dialog::{Dialog, DialogHeader};
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Control, Prose, Title};
use crate::platform::{self, OpenCtx};

/// Mounted right after `ContextMenuViewer` at the window root: while open, its key
/// handler precedes every feature listener in document order and consumes every press —
/// Esc = keep (canvas `_onKey`), Enter = stop, everything else is the modal barrier.
/// So a ⌘W under the dialog can't close the very tab being confirmed, and Esc never
/// falls through to "cancel the query".
#[derive(PartialEq)]
pub struct CloseConfirm {
    pub confirm: State<Option<CloseTarget>>,
    /// What a confirmed *window* close needs: the shared close path puts the launcher up
    /// when this window is the app's last, so "Stop & exit" lands where the red button
    /// would rather than quitting the app.
    pub app: AppCtx,
}

impl Component for CloseConfirm {
    fn render(&self) -> impl IntoElement {
        let confirm = self.confirm;
        let platform = use_hook(Platform::get);
        // Cloned, not copied: `CloseTarget::Reroot` carries the folder to open.
        let target = confirm.read().clone();
        // The window's open path, for the re-root variant's answer — the same handle every
        // other open surface uses, so a confirmed re-root goes through one mechanism.
        let open = use_consume::<OpenCtx>();
        // The window's engine generation, for the restart variant's answer — bumping it is the
        // rebuild, exactly as setting the root is the re-root.
        let restart = use_consume::<EngineRestart>();
        let radio = use_radio::<SessionState, Chan>(Chan::Tabs);
        let project = use_radio_station::<ProjectState, ProjChan>();
        let config = use_config_station();
        let settings = use_config(ConfigChan::Settings);
        let theme = use_theme();
        // The action wears the shared `cancel_button` dress (the running body's Cancel)
        // — the themes' authored stop-the-query tone, not a hardcoded red.
        let cancel = get_theme!(
            &None::<CancelButtonThemePartial>,
            CancelButtonThemePreference,
            "cancel_button"
        );

        // Stop & close / Stop & exit — shared by the button and the Enter key, so it's
        // `Clone` (the theme handle isn't `Copy`) and each handler takes its own.
        let close_anyway = {
            let app = self.app.clone();
            move || {
                let mut radio = radio;
                let mut confirm = confirm;
                // Read the target into a value FIRST. `match *confirm.peek() { … }` keeps the
                // scrutinee's temporary — and so the generational-box borrow — alive for the
                // whole match, and the `set(None)` inside then panics ("already borrowed").
                // Verified with a probe: the match form panics, this one doesn't.
                let target = confirm.peek().clone();
                match target {
                    Some(CloseTarget::Tab(id)) => {
                        // The root's tab-diff funnel cancels/retires the tab's engine state.
                        radio.write().close_one(id);
                        confirm.set(None);
                    }
                    // The shared close path: bypasses the on_close veto (this *is* the
                    // confirmed close) and hands over to the launcher if we're the last.
                    // Dismiss first — the close is several async hops (it may stand a
                    // launcher up before this window goes), and a dialog left armed across
                    // them can be pressed again and open a *second* launcher.
                    Some(CloseTarget::Window) => {
                        confirm.set(None);
                        // `spawn_forever`, not `spawn`: dismissing unmounts the dialog subtree
                        // this handler belongs to, and scope teardown drops that scope's tasks
                        // before they are ever polled — the window would simply never close.
                        // See the same note in `drop_confirm::drop_row`.
                        spawn_forever(platform::close_this_window(platform.clone(), app.clone()));
                    }
                    // Dismiss first for the same reason, and more sharply: the re-root
                    // unmounts this very subtree, so a slot left armed would arrive at the
                    // new project still asking about the old one's queries.
                    Some(CloseTarget::Reroot(root)) => {
                        confirm.set(None);
                        open.reroot_confirmed(root);
                    }
                    // And the same again: the bump unmounts this subtree, so the slot is
                    // cleared before it goes.
                    Some(CloseTarget::Restart) => {
                        confirm.set(None);
                        restart.restart();
                    }
                    None => {}
                }
            }
        };
        let close_anyway_key = close_anyway.clone();
        // Every dismissal path (the keep button, Esc, the backdrop). Dismissing a *window*
        // confirm is also the answer to the quit that raised it: without clearing the flag
        // it would latch, and every later close would behave as though the app were exiting.
        let keep_open = move || {
            let mut confirm = confirm;
            // Same borrow rule as `close_anyway`: read it out before writing. Only the
            // *window* variant answers a quit — a declined re-root or tab close never began
            // one, and clearing the flag for them would abandon a quit still in flight.
            let quitting = matches!(&*confirm.peek(), Some(CloseTarget::Window));
            if quitting {
                platform::end_quit();
            }
            confirm.set(None);
        };

        let Some(target) = target else {
            return rect().into_element();
        };

        // The canvas copy, per variant (`ccIsProject`) — plus the re-root, which is the
        // window variant's question about a project rather than the app.
        let (title, body, keep, action, action_icon) = match target {
            CloseTarget::Window => (
                "Confirm exit",
                "Queries are running. Are you sure you want to stop them and exit?",
                "Cancel",
                "Stop & exit",
                IconName::LogOut,
            ),
            CloseTarget::Tab(_) => (
                "Confirm close",
                "A query is running. Are you sure you want to stop it and close this tab?",
                "Keep tab open",
                "Stop & close",
                IconName::Stop,
            ),
            CloseTarget::Reroot(_) => (
                "Confirm open",
                "Queries are running. Are you sure you want to stop them and open another project?",
                "Cancel",
                "Stop & open",
                IconName::Stop,
            ),
            CloseTarget::Restart => (
                "Confirm restart",
                "These properties change the engine runtime, which is fixed when the engine \
                 starts. Restarting stops any running query and registers your tables and views \
                 again.",
                "Not now",
                "Restart engine",
                IconName::Reload,
            ),
        };
        // The subject line: what is being closed — except for the re-root, where naming the
        // project being *opened* is what identifies the action the user just took.
        let name = match target {
            CloseTarget::Tab(id) => radio
                .read()
                .tabs
                .get(&id)
                .map(|t| t.name.clone())
                .unwrap_or_default(),
            CloseTarget::Window => project.read().name.clone(),
            CloseTarget::Reroot(root) => folder_name(&root),
            // The engine belongs to the project, so naming it is what says *whose* engine.
            CloseTarget::Restart => project.read().name.clone(),
        };

        let c = theme.read().colors().clone();

        // Checked = don't ask = the `confirm_close_running` setting off. Toggling writes
        // the app-global config (the close guard mirrors it immediately) and persists in
        // the same funnel — the comp's checkbox edits the setting directly, not a local
        // draft.
        let dont_ask = !settings.read().settings.confirm_close_running;
        let toggle_dont_ask = move |_: Event<PressEventData>| {
            write_config(config, &[ConfigChan::Settings], |cfg| {
                cfg.settings.confirm_close_running = !cfg.settings.confirm_close_running;
            });
        };

        // The title run beside the chip: what is being closed, over its name.
        let header = DialogHeader::new(
            IconName::Warning,
            c.warning,
            rect()
                .vertical()
                .child(Title::new(title).color(c.text_primary))
                .child(
                    Prose::new(name)
                        .color(c.text_placeholder)
                        .text_overflow(TextOverflow::Ellipsis),
                ),
        );

        let checkbox_row = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.)
            .padding((4., 8.))
            .corner_radius(8.)
            .on_press(toggle_dont_ask)
            .child(Checkbox::new().selected(dont_ask).size(16.))
            .child(Prose::new("Don't ask again").color(c.text_placeholder));

        // The card, the strip and the modal keys (Esc keeps, Enter stops) are `Dialog`'s; this
        // supplies the comp's own header, body and its two actions.
        Dialog::new()
            // Esc and the backdrop are dismissals, so they go through `keep_open` — clearing
            // the quit flag, not just the dialog.
            .on_dismiss(move |_| keep_open())
            // Enter takes the second clone: `close_anyway` captures the `AppCtx` the launcher
            // hand-off needs, so it isn't `Copy` and can't be moved into two handlers.
            .on_confirm(move |_| close_anyway_key())
            .header(header)
            .body(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(12.)
                    .child(Prose::new(body).color(c.text_secondary).wrap())
                    .child(checkbox_row),
            )
            .action(
                Button::new()
                    .flat()
                    .on_press(move |_| keep_open())
                    .child(Control::new(keep)),
            )
            .action(
                Button::new()
                    .filled()
                    .theme_colors(
                        ButtonColorsThemePartial::default()
                            .background(cancel.background)
                            .hover_background(cancel.hover_background)
                            .border_fill(cancel.border_fill)
                            .hover_border_fill(cancel.border_fill)
                            .color(cancel.color)
                            .hover_color(cancel.color),
                    )
                    .on_press(move |_| close_anyway())
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .child(Icon::new(action_icon).size(13.))
                            .child(Control::new(action)),
                    ),
            )
            .into_element()
    }
}
