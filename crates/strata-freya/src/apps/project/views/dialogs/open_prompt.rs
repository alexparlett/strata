//! The **open-target prompt** (B10), built to the Strata canvas's open-target comp: the
//! accent folder chip over "Open Project" and the folder being opened, the question, a
//! "Remember, don't ask again" checkbox, and the three actions — Cancel · New Window · This
//! Window, with This Window as the primary (and so Enter's).
//!
//! It is raised only when the "Opening a project" preference is *Ask* and the open came from
//! a window that already has a project; the routing and both outcomes are [`OpenCtx`]'s, so
//! this file is the surface and nothing else.
//!
//! Host + card, like the Dioxus original: the host stays mounted reading the slot, while the
//! card — and so the `remember` checkbox — is a fresh scope per prompt, which is what resets
//! the box to unchecked on every open rather than carrying the last answer forward. The card
//! is **keyed on the folder** to make that true even when one question replaces another
//! without the slot passing through `None`.

use freya::components::use_theme;
use freya::prelude::*;

use std::path::PathBuf;

use crate::components::dialog::{Dialog, DialogHeader};
use crate::components::icon::IconName;
use crate::components::typography::{Control, Prose, Title};
use crate::platform::OpenCtx;
use crate::state::AppCtx;

/// Mounted at the window root beside the other dialogs, and above them in document order:
/// while it is open its key barrier precedes every feature listener, so Esc dismisses the
/// question rather than cancelling the query underneath it.
#[derive(PartialEq)]
pub struct OpenPrompt {
    pub open: OpenCtx,
    pub app: AppCtx,
}

impl Component for OpenPrompt {
    fn render(&self) -> impl IntoElement {
        let pending = self.open.prompt.read().clone();
        match pending {
            Some(path) => OpenPromptCard {
                path,
                open: self.open,
                app: self.app.clone(),
            }
            .into_element(),
            None => rect().into_element(),
        }
    }
}

#[derive(PartialEq)]
struct OpenPromptCard {
    /// The project folder waiting on an answer.
    path: PathBuf,
    open: OpenCtx,
    app: AppCtx,
}

impl Component for OpenPromptCard {
    fn render(&self) -> impl IntoElement {
        let platform = use_hook(Platform::get);
        let remember = use_state(|| false);
        let theme = use_theme();
        let c = theme.read().colors().clone();
        let open = self.open;

        // One binding per handler: `AppCtx` and `Platform` are `Clone`, not `Copy`, so each
        // action takes its own. Enter and This Window are the same outcome and so are two
        // separate closures over the same values, not one shared one — `on_confirm` carries
        // `()` while a button handler carries its press event.
        let (this_app, this_platform) = (self.app.clone(), platform.clone());
        let (enter_app, enter_platform) = (self.app.clone(), platform.clone());
        let (new_app, new_platform) = (self.app.clone(), platform);

        let header = DialogHeader::new(
            IconName::Folder,
            c.primary,
            rect()
                .vertical()
                .child(Title::new("Open Project").color(c.text_primary))
                .child(
                    Prose::new(self.path.display().to_string())
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
            .on_press(move |_: Event<PressEventData>| {
                let mut remember = remember;
                remember.toggle();
            })
            .child(Checkbox::new().selected(*remember.read()).size(16.))
            .child(Prose::new("Remember, don't ask again").color(c.text_placeholder));

        Dialog::new()
            .on_dismiss(move |_| open.dismiss())
            // Enter takes the primary action, which is the comp's This Window.
            .on_confirm(move |_| {
                open.choose(
                    enter_platform.clone(),
                    enter_app.clone(),
                    false,
                    *remember.peek(),
                )
            })
            .header(header)
            .body(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(12.)
                    .child(
                        Prose::new("Open this project in the current window, or in a new window?")
                            .color(c.text_secondary)
                            .wrap(),
                    )
                    .child(checkbox_row),
            )
            .action(
                Button::new()
                    .flat()
                    .on_press(move |_| open.dismiss())
                    .child(Control::new("Cancel")),
            )
            .action(
                Button::new()
                    .outline()
                    .on_press(move |_| {
                        open.choose(
                            new_platform.clone(),
                            new_app.clone(),
                            true,
                            *remember.peek(),
                        )
                    })
                    .child(Control::new("New Window")),
            )
            .action(
                Button::new()
                    .filled()
                    .on_press(move |_| {
                        open.choose(
                            this_platform.clone(),
                            this_app.clone(),
                            false,
                            *remember.peek(),
                        )
                    })
                    .child(Control::new("This Window")),
            )
            .into_element()
    }

    /// Keyed on the folder, so a prompt **replaced in place** is a different card. The slot
    /// does not always pass through `None` between questions — raising a prompt while one is
    /// already up (a menubar recent behind the open dialog) overwrites it — and without the
    /// key that would re-render the same scope, carrying a ticked `remember` onto a question
    /// the user has not answered yet.
    fn render_key(&self) -> DiffKey {
        DiffKey::from(&self.path)
    }
}

/// The dialog driven for real: mounted under a headless runner, pressed by its own copy, and
/// asserted through the only two things it is allowed to change — the window's project root
/// and its pending question.
///
/// **Two paths are deliberately not driven here**, so that neither is mistaken for covered:
///
/// * **New Window.** The press reaches `platform::open_project` →
///   `Platform::launch_window`, which awaits an ack from a renderer the headless harness has
///   none of, and `expect`s on it — so the spawned task panics the moment it is polled. The
///   right fix is not to loosen that `expect` (a cancelled window launch really is a fault in
///   production, and widening its signature to suit a test is the thing §1 forbids), so the
///   button's routing rests on `platform::open::tests` instead, which proves the same
///   `OpenTarget::NewWindow` decision without a window to open.
/// * **"Remember, don't ask again" persisting the preference.** `write_config` is the sole
///   write path *and* it funnels to the real user config file — a test that ticked the box
///   would overwrite the developer's own settings and recents. Every press below therefore
///   leaves the box unticked, which is also its default. Covering it properly needs a config
///   path a test can redirect; that is not worth a production seam today, so it is a real gap
///   rather than a silent one.
#[cfg(test)]
mod interaction {
    use freya_testing::{TestingNode, TestingRunner};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use strata_core::config::AppConfig;
    use strata_core::theme::load;

    use super::*;
    use crate::apps::project::{CloseGuard, CloseTarget};
    use crate::menu::create_global_menu;
    use crate::platform::{create_global_open, create_global_windows};
    use crate::state::{create_global_theme_preview, ConfigStation};
    use crate::theme::{strata_theme, ThemesCtx};

    const HERE: &str = "/data/sales";
    const THERE: &str = "/data/ml_features";

    fn app() -> impl IntoElement {
        use_init_theme(|| strata_theme(&load("midnight")));
        let open = use_consume::<OpenCtx>();
        let app = use_consume::<AppCtx>();
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .child(OpenPrompt { open, app })
    }

    /// Mount the prompt already asking about [`THERE`], from a window showing [`HERE`].
    fn armed() -> (TestingRunner, OpenCtx) {
        TestingRunner::new(
            app,
            (900., 700.).into(),
            |r| {
                // The window's two slots — the only thing the assertions read, because they
                // are the only thing the dialog is allowed to change.
                let open = r.provide_root_context(|| OpenCtx {
                    root: State::create(PathBuf::from(HERE)),
                    prompt: State::create(Some(PathBuf::from(THERE))),
                    // Idle by default, like a window with nothing executing; the gate test
                    // flips `running` before it presses.
                    guard: State::create(Arc::new(CloseGuard {
                        running: Arc::new(AtomicBool::new(false)),
                        confirm: AtomicBool::new(true),
                        last: AtomicBool::new(false),
                    })),
                    confirm: State::create(None),
                });
                // The app-globals the actions are handed. Fresh per test, so nothing here
                // touches the real app's config store.
                r.provide_root_context(|| AppCtx {
                    themes: ThemesCtx::discover(),
                    config: ConfigStation::create_global(AppConfig::default()),
                    windows: create_global_windows(),
                    preview: create_global_theme_preview(),
                    menu: create_global_menu(),
                    open: create_global_open(),
                });
                open
            },
            1.,
        )
    }

    /// The centre of the first label reading `text` — so a control is pressed by its own copy
    /// rather than by a coordinate that any layout tweak would silently invalidate.
    fn label_center(runner: &TestingRunner, text: &str) -> (f64, f64) {
        let node: TestingNode = runner
            .find(|node, element| {
                Label::try_downcast(element)
                    .filter(|label| label.text.as_ref() == text)
                    .map(|_| node)
            })
            .unwrap_or_else(|| panic!("no label reading {text:?} is on screen"));
        let area = node.layout().area;
        (area.center().x as f64, area.center().y as f64)
    }

    fn is_on_screen(runner: &TestingRunner, text: &str) -> bool {
        runner
            .find(|_, element| {
                Label::try_downcast(element).filter(|label| label.text.as_ref() == text)
            })
            .is_some()
    }

    /// The card is up only while the slot holds a folder, and it names the folder it is
    /// asking about — the host/card split, which is also what resets `remember` per prompt.
    #[test]
    fn the_card_is_up_only_while_a_folder_is_pending() {
        let (mut runner, open) = armed();
        runner.sync_and_update();
        assert!(is_on_screen(&runner, "Open Project"));
        assert!(is_on_screen(&runner, THERE), "the card names the folder");

        let mut prompt = open.prompt;
        prompt.set(None);
        runner.sync_and_update();
        assert!(!is_on_screen(&runner, "Open Project"));
    }

    /// **This Window re-roots the window.** The whole point of the primary action: the root
    /// the project subtree is keyed on becomes the folder that was pending, which is what
    /// unmounts the old project and stands the new one up.
    #[test]
    fn this_window_rewrites_the_window_s_project() {
        let (mut runner, open) = armed();
        runner.sync_and_update();

        runner.click_cursor(label_center(&runner, "This Window"));
        runner.sync_and_update();

        assert_eq!(*open.root.peek(), PathBuf::from(THERE));
        assert!(open.prompt.peek().is_none(), "the question is answered");
    }

    /// **A re-root asks before it destroys work.** Opening in place unmounts the project
    /// subtree, and dropping its engine aborts every query executing in it — the same loss
    /// ⇧⌘W, the red button and ⌘Q all stop and ask about. So with a query in flight This
    /// Window must hand over to the close-while-running confirm rather than re-root behind
    /// it; the window keeps its project until that second question is answered.
    #[test]
    fn this_window_asks_first_while_a_query_is_running() {
        let (mut runner, open) = armed();
        open.guard.peek().running.store(true, Ordering::Relaxed);
        runner.sync_and_update();

        runner.click_cursor(label_center(&runner, "This Window"));
        runner.sync_and_update();

        assert_eq!(
            *open.root.peek(),
            PathBuf::from(HERE),
            "the window must not re-root behind a running query"
        );
        assert!(
            matches!(&*open.confirm.peek(), Some(CloseTarget::Reroot(root)) if root == Path::new(THERE)),
            "the confirm is armed for the folder that was asked about"
        );
        assert!(
            open.prompt.peek().is_none(),
            "the This/New question itself is answered"
        );
    }

    /// Enter takes the primary action, exactly as pressing it does — `Dialog::on_confirm`.
    #[test]
    fn enter_takes_this_window() {
        let (mut runner, open) = armed();
        runner.sync_and_update();

        runner.press_key(Key::Named(NamedKey::Enter));
        runner.sync_and_update();

        assert_eq!(*open.root.peek(), PathBuf::from(THERE));
        assert!(open.prompt.peek().is_none());
    }

    /// **Every dismissal leaves the project alone.** Cancel, Esc and the backdrop all answer
    /// "neither window" — a dismissal that re-rooted, or that left the slot armed for a later
    /// press, would open a project the user declined.
    #[test]
    fn cancel_esc_and_the_backdrop_open_nothing() {
        let (mut runner, open) = armed();
        let mut prompt = open.prompt;

        for dismiss in ["cancel", "escape", "backdrop"] {
            prompt.set(Some(PathBuf::from(THERE)));
            runner.sync_and_update();
            assert!(is_on_screen(&runner, "Open Project"), "{dismiss}: armed");

            match dismiss {
                "cancel" => {
                    let at = label_center(&runner, "Cancel");
                    runner.click_cursor(at);
                }
                "escape" => runner.press_key(Key::Named(NamedKey::Escape)),
                // Well clear of the 420px card centred in a 900x700 window.
                _ => runner.click_cursor((20., 20.)),
            }
            runner.sync_and_update();

            assert!(
                open.prompt.peek().is_none(),
                "{dismiss}: the question is dropped"
            );
            assert_eq!(
                *open.root.peek(),
                PathBuf::from(HERE),
                "{dismiss}: the window keeps its project"
            );
        }
    }
}
