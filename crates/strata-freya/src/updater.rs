//! The updater's **surfaces** (UP-03) — the half of the mechanism that belongs to no single
//! window.
//!
//! Three layers, three names: `strata_core::update` is the mechanism (check, download, verify,
//! swap), [`crate::state::updates`] is the app-global status and the presses that move it, and
//! this module is what the user sees — [`Affordance`], the one answer to "what does the app offer
//! right now", and [`UpdateConfirm`], the restart question.
//!
//! **Here rather than in an `apps/` folder** because `apps/` is one folder per OS window and
//! nothing here belongs to one: the launcher rail's version line, the app-global menubar and a
//! dialog mounted at both workspace roots are the same offer in three places. That is exactly
//! why [`Affordance`] exists at all — the rail's label, the menubar's press and the dialog's
//! confirm all resolve through one pure function over the status and the install site, so no
//! surface can offer a download the site cannot install or a restart the mechanism has not
//! staged.
//!
//! **Deliberately quiet.** No toast system and no badge on every window: `Idle`, `UpToDate`,
//! `Checking` and `Failed` all render *nothing*, and a failed check is a log line rather than
//! chrome. The rail nags nobody; App ▸ *Check for Updates…* is how you ask.

use freya::prelude::*;
use strata_core::update::{open_page, Site};
use strata_core::util::human_bytes;

use crate::components::dialog::{Dialog, DialogHeader};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::SP_3;
use crate::components::tones::tones;
use crate::components::typography::{Control, Prose, Title};
use crate::state::{check, download, install, install_site, Update, UpdateStatus};
use crate::theme::{use_roles, Role};

/// **What the app offers about updates right now** — the one decision every surface reads.
///
/// A pure function of the status and the install site, so it is unit-testable without a window
/// and there is exactly one copy of the rules the surfaces would otherwise each restate: that a
/// dev build offers nothing, that a release carrying no archive (or a bundle this process
/// cannot replace) degrades to the release page, and that a staged update is a restart rather
/// than another download.
#[derive(Clone, PartialEq, Debug)]
pub enum Affordance {
    /// Not an installation — a `cargo run` build. Nothing to show and nothing to press: the
    /// mechanism is inert here too (`state::updates::use_updates`).
    Inert,
    /// Nothing has been asked yet, the answer was "up to date", a check is in flight, or the
    /// last one failed. The rail shows nothing for all four; a press is the **manual check**,
    /// which stands itself down while one is already running.
    Check,
    /// A download is in flight. Quiet progress text, and a press that does nothing.
    Downloading { got: u64, total: Option<u64> },
    /// A newer release, an archive to install and somewhere to install it. A press downloads.
    Get { version: String },
    /// A newer release the app cannot install itself — the release carries no update archive,
    /// or this bundle sits where it cannot be replaced ([`Site::ReadOnly`]). A press opens the
    /// release page, and the label says so rather than promising an update.
    Page { version: String, page_url: String },
    /// Downloaded and verified. A press asks the restart question ([`UpdateConfirm`]).
    Restart { version: String, page_url: String },
}

impl Affordance {
    /// Resolve the offer. `site` is [`install_site`]'s cached answer in the app; a test passes
    /// whichever site it is making a claim about.
    pub fn of(status: &Update, site: &Site) -> Affordance {
        if matches!(site, Site::Unbundled) {
            return Affordance::Inert;
        }
        match status {
            Update::Idle | Update::Checking | Update::UpToDate | Update::Failed { .. } => {
                Affordance::Check
            }
            Update::Downloading { got, total, .. } => Affordance::Downloading {
                got: *got,
                total: *total,
            },
            Update::Available {
                version,
                page_url,
                asset,
            } => match (asset, site) {
                (Some(_), Site::Writable(_)) => Affordance::Get {
                    version: version.clone(),
                },
                _ => Affordance::Page {
                    version: version.clone(),
                    page_url: page_url.clone(),
                },
            },
            // Regardless of the site: a `Ready` can only have come from a `Get`, which needed a
            // writable one, and a site that changed under a running app is `install`'s own
            // failure to report rather than a state to hide here.
            Update::Ready {
                version, page_url, ..
            } => Affordance::Restart {
                version: version.clone(),
                page_url: page_url.clone(),
            },
        }
    }

    /// The **accent action's** label, or nothing — what a surface with room for one press
    /// draws. The three offers worth a press each say what pressing does; the rest draw no
    /// action at all, which is what keeps the rail from nagging.
    ///
    /// The labels are short, and name no version, because the surface reading them is 200px
    /// wide: *which* version is on offer is the line above ([`Affordance::note`]).
    pub fn action(&self) -> Option<&'static str> {
        match self {
            Affordance::Inert | Affordance::Check | Affordance::Downloading { .. } => None,
            Affordance::Get { .. } => Some("Update now"),
            // Not "Update …": this press installs nothing, and a label that promised it would
            // be answered by a browser window.
            Affordance::Page { .. } => Some("Open the release page"),
            Affordance::Restart { .. } => Some("Restart to update"),
        }
    }

    /// The **quiet line** above the action, or nothing: which version is on offer, or how a
    /// download is getting on. A server that declared no length is stated as what is known
    /// rather than as a fraction of a guess.
    ///
    /// A staged update says so — "downloaded", not "available" — because the two differ in
    /// what the press below costs: one is a network job, the other is a restart.
    pub fn note(&self) -> Option<String> {
        match self {
            Affordance::Inert | Affordance::Check => None,
            Affordance::Downloading {
                got,
                total: Some(total),
            } => Some(format!(
                "Downloading {} of {}",
                human_bytes(*got),
                human_bytes(*total)
            )),
            Affordance::Downloading { got, total: None } => {
                Some(format!("Downloading {}", human_bytes(*got)))
            }
            Affordance::Get { version } | Affordance::Page { version, .. } => {
                Some(format!("Strata {version} is available"))
            }
            Affordance::Restart { version, .. } => Some(format!("Strata {version} is downloaded")),
        }
    }
}

/// **Ask for the update** — the one press behind every surface.
///
/// The launcher rail draws it as an action, the menubar item runs it by another name, and
/// neither implements anything: what a press means is [`Affordance`]'s answer, and each arm is one
/// call into a funnel `state::updates` already owns. That is what makes *Check for Updates…*
/// pressed over a staged update open the restart question rather than start a second check.
///
/// `ask` is the window's own confirm slot — the status is app-global, the question is not.
///
/// **Call it from a Freya scope**, never from the renderer thread: two of its arms reach
/// `state::updates`, which starts its worker with `spawn_forever`, and that panics outside
/// Freya's current context. The menubar item is the one surface with no scope of its own, which
/// is what [`UpdateRequest`] is for.
pub fn press(status: UpdateStatus, ask: AskSlot) {
    // Resolved into a value first: a `match` over `status.peek()` keeps the read borrow alive
    // for the whole match, and the `set` inside `check` or `download` then panics (the trap
    // `close_confirm` documents).
    let offer = Affordance::of(&status.peek(), install_site());
    match offer {
        // A press with nothing to ask for. `Downloading` is here rather than absent because
        // the menubar item is pressable whatever the status: re-pressing it mid-download must
        // not start a second job or replace the one in flight with a check.
        Affordance::Inert | Affordance::Downloading { .. } => {}
        Affordance::Check => check(status),
        Affordance::Get { .. } => download(status),
        Affordance::Page { page_url, .. } => open_page(&page_url),
        Affordance::Restart { version, page_url } => {
            let mut ask = ask;
            ask.set(Some(UpdateAsk { version, page_url }));
        }
    }
}

/// The restart question's subject, carried on the slot: what the app will come back as, and
/// where to read what changed.
///
/// It carries the version rather than reading the status, on the slot pattern's own terms —
/// the dialog states what it was raised about, and cannot end up asking about one version
/// while naming another.
#[derive(Clone, PartialEq, Debug)]
pub struct UpdateAsk {
    pub version: String,
    pub page_url: String,
}

/// **One window's restart-question slot** — the confirm pattern's `State<Option<T>>`, provided
/// at each workspace window's root and watched by the [`UpdateConfirm`] mounted there.
///
/// Per window rather than app-global, unlike the status behind it: two project windows would
/// otherwise both raise the dialog for one press, and a question belongs to whoever is looking
/// at it.
pub type AskSlot = State<Option<UpdateAsk>>;

/// **The menubar has asked for an update**, waiting for a window to carry it out.
///
/// App ▸ *Check for Updates…* is the one surface with no Freya scope: `handle_menu_event` runs
/// on the renderer thread, straight out of winit's `user_event`, where Freya's current context
/// is unset — so [`press`] called there would panic the moment it reached `spawn_forever`. (The
/// menubar's other data-carrying item, Open Recent, avoids the same edge by hand-rolling its
/// open rather than calling `OpenCtx::apply`.)
///
/// So the press records the intent and the **focused** window performs it, from the
/// `use_side_effect` in [`use_file_menu`](crate::menu::use_file_menu) — AGENTS.md §3's rule for
/// a press with no scope to run in, and it rides the call every window root already makes.
/// A `bool` rather than a parked handle: the window doing the work already has its own
/// [`AskSlot`] from its [`MenuScope`](crate::menu::MenuScope), so there is nothing to point at.
pub type UpdateRequest = State<bool>;

/// Create the slot. Call **once**, in `main`, like its neighbours.
pub fn create_global_update_request() -> UpdateRequest {
    State::create_global(false)
}

/// **Restart now?** — the one gate in front of the install, mounted at the launcher root and
/// the project root (one component, two mounts: the slot is per window, the status behind it
/// is the app's one).
///
/// It asks *this* question and no other. Confirming is the ordinary quit, so a window with a
/// running query still gets its own close confirm afterwards — "lose the running query?" stays
/// that dialog's question, and re-asking it here would be a second, weaker copy of it.
/// Dismissing leaves the status on `Ready`: the staged bundle is untouched and the press can
/// simply be made again.
#[derive(PartialEq)]
pub struct UpdateConfirm {
    pub ask: AskSlot,
    pub status: UpdateStatus,
}

impl Component for UpdateConfirm {
    fn render(&self) -> impl IntoElement {
        let mut ask = self.ask;
        let status = self.status;
        let asked = ask.read().clone();
        let roles = use_roles();
        let info = tones().info;

        // Shared by the button and the Enter key, so it holds only `Copy` handles and shadows
        // `ask` inside — a closure capturing the outer `mut` binding would be `FnMut`, and the
        // two handlers cannot both take it.
        let restart = move |()| {
            let mut ask = ask;
            // Dismissed **before** the install, which is a quit: a close confirm may cancel it
            // (`end_quit` then forgets the intent), and a slot left armed would be a question
            // already answered, sitting over the window that answered it.
            ask.set(None);
            install(status);
        };

        let Some(asked) = asked else {
            return rect().into_element();
        };

        let page_url = asked.page_url.clone();

        // The action over its subject — the other confirms' shape: what pressing does, over
        // what it will leave you running.
        let header = DialogHeader::new(
            IconName::Download,
            info,
            rect()
                .vertical()
                .child(Title::new("Restart to update").color(roles.get(Role::Text)))
                .child(
                    Prose::new(format!("Strata {}", asked.version))
                        .color(roles.get(Role::TextPlaceholder))
                        .text_overflow(TextOverflow::Ellipsis),
                ),
        );

        Dialog::new()
            .on_dismiss(move |()| ask.set(None))
            .on_confirm(restart)
            .header(header)
            .body(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(SP_3)
                    .child(
                        Prose::new(
                            "Strata closes and starts again on the new version. Any window that \
                             would ask before quitting still asks.",
                        )
                        .color(roles.get(Role::TextMuted))
                        .wrap(),
                    )
                    // The link-out for what changed. A ghost action rather than prose the user
                    // cannot press: the release page is the only place the notes exist, and it
                    // is also the fallback this app offers when it cannot install for itself.
                    .child(
                        rect().horizontal().child(
                            Button::new()
                                .flat()
                                .compact()
                                .theme_colors(
                                    ButtonColorsThemePartial::default()
                                        .color(roles.get(Role::Accent))
                                        .hover_color(roles.get(Role::Accent)),
                                )
                                .on_press(move |_| open_page(&page_url))
                                .child(Control::new("Release notes")),
                        ),
                    ),
            )
            .action(
                Button::new()
                    .flat()
                    .on_press(move |_| ask.set(None))
                    .child(Control::new("Not now")),
            )
            .action(
                Button::new().filled().on_press(move |_| restart(())).child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(SP_3)
                        .child(Icon::new(IconName::Reload).size(13.))
                        .child(Control::new("Restart now")),
                ),
            )
            .into_element()
    }
}

/// The offer's rules, which is the whole of what a surface reads, plus the dialog's dismissals.
///
/// [`Affordance::of`] is a pure function, and that is what lets the rail, the menubar item and
/// the dialog agree by construction — [`press`] is a thin match over its answer, deliberately,
/// so there is one thing to test rather than three. `press` itself is not
/// exercised here: it asks [`install_site`], and a test binary is never a bundle, so every arm
/// would resolve to `Inert`. Handing the site in as an argument to make it testable is the
/// production-signature-for-a-test shape AGENTS.md §1 refuses.
///
/// Nor is **confirming** the dialog: the press is an install, and an install is a `quit` plus a
/// process-global intent. What is covered is every path that leaves the status alone.
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use freya_testing::TestingRunner;
    use strata_core::theme::load;
    use strata_core::update::Asset;

    use super::*;
    use crate::state::create_global_updates;
    use crate::theme::strata_theme;

    fn writable() -> Site {
        Site::Writable(PathBuf::from("/Applications/Strata.app"))
    }

    fn read_only() -> Site {
        Site::ReadOnly(PathBuf::from("/Applications/Strata.app"))
    }

    fn asset() -> Asset {
        Asset {
            name: "Strata-0.4.0-update.zip".into(),
            url: "https://example.invalid/Strata.zip".into(),
            size: 1 << 20,
        }
    }

    fn available(asset: Option<Asset>) -> Update {
        Update::Available {
            version: "0.4.0".into(),
            page_url: "https://example.invalid/releases/v0.4.0".into(),
            asset,
        }
    }

    /// **A dev build offers nothing at all**, whatever the status says — the mechanism is inert
    /// outside a bundle, and a surface that still drew an action would be offering to replace
    /// something that is not an installation.
    #[test]
    fn an_unbundled_app_offers_nothing() {
        for status in [Update::Idle, available(Some(asset()))] {
            let offer = Affordance::of(&status, &Site::Unbundled);
            assert_eq!(offer, Affordance::Inert);
            assert_eq!(offer.action(), None);
            assert_eq!(offer.note(), None);
        }
    }

    /// The four quiet statuses draw nothing and press as a check. A failed check especially:
    /// it is a log line, not chrome, and the way to retry it is the manual press.
    #[test]
    fn the_quiet_statuses_draw_nothing_and_press_as_a_check() {
        for status in [
            Update::Idle,
            Update::Checking,
            Update::UpToDate,
            Update::Failed {
                why: "no network".into(),
            },
        ] {
            let offer = Affordance::of(&status, &writable());
            assert_eq!(offer, Affordance::Check, "{status:?}");
            assert_eq!(offer.action(), None, "{status:?}");
            assert_eq!(offer.note(), None, "{status:?}");
        }
    }

    /// An offer with an archive and somewhere to put it is a download, and its label names the
    /// version being offered rather than the one running.
    #[test]
    fn an_installable_offer_is_a_download() {
        let offer = Affordance::of(&available(Some(asset())), &writable());
        assert_eq!(
            offer,
            Affordance::Get {
                version: "0.4.0".into()
            }
        );
        assert_eq!(offer.action(), Some("Update now"));
        assert_eq!(offer.note().as_deref(), Some("Strata 0.4.0 is available"));
    }

    /// **The two degraded offers are one arm**, and neither promises an update: a release with
    /// no archive and a bundle that cannot be replaced both leave the release page as the only
    /// thing this app can honestly offer.
    #[test]
    fn a_release_with_no_archive_and_a_read_only_bundle_both_degrade_to_the_page() {
        let page = Affordance::Page {
            version: "0.4.0".into(),
            page_url: "https://example.invalid/releases/v0.4.0".into(),
        };
        assert_eq!(Affordance::of(&available(None), &writable()), page);
        assert_eq!(
            Affordance::of(&available(Some(asset())), &read_only()),
            page
        );
        assert_eq!(page.action(), Some("Open the release page"));
    }

    /// A staged update is a restart, not a second download — the press that installs the one in
    /// hand is the only thing left to offer.
    #[test]
    fn a_staged_update_is_a_restart() {
        let status = Update::Ready {
            version: "0.4.0".into(),
            page_url: "https://example.invalid/releases/v0.4.0".into(),
            staged: PathBuf::from("/tmp/strata-update-x/Strata.app"),
        };
        let offer = Affordance::of(&status, &writable());
        assert_eq!(offer.action(), Some("Restart to update"));
        // "downloaded", not "available": the press below is a restart, not a network job.
        assert_eq!(offer.note().as_deref(), Some("Strata 0.4.0 is downloaded"));
        assert!(matches!(offer, Affordance::Restart { .. }));
    }

    /// A download draws progress and **no** action, and states what is known when the server
    /// declared no length rather than a fraction of a guess.
    #[test]
    fn a_download_draws_progress_and_no_action() {
        let downloading = |total| {
            Affordance::of(
                &Update::Downloading {
                    version: "0.4.0".into(),
                    page_url: "https://example.invalid".into(),
                    got: 1 << 20,
                    total,
                },
                &writable(),
            )
        };

        let known = downloading(Some(4 << 20));
        assert_eq!(known.action(), None);
        assert_eq!(
            known.note().as_deref(),
            Some("Downloading 1.0 MB of 4.0 MB")
        );

        let unknown = downloading(None);
        assert_eq!(unknown.action(), None);
        assert_eq!(unknown.note().as_deref(), Some("Downloading 1.0 MB"));
    }
    /// The dialog driven the way the user drives it: `TestingRunner` over the confirm alone,
    /// with its slot and the app-global status provided at the root the way both window roots
    /// provide them.
    fn runner() -> (TestingRunner, (AskSlot, UpdateStatus)) {
        fn app() -> impl IntoElement {
            use_init_theme(|| strata_theme(&load("midnight")));
            let ask = use_consume::<AskSlot>();
            let status = use_consume::<UpdateStatus>();
            rect().expanded().child(UpdateConfirm { ask, status })
        }
        TestingRunner::new(
            app,
            (900., 700.).into(),
            |r| {
                let ask = r.provide_root_context(|| State::create(None::<UpdateAsk>));
                // Idle, and it stays that way: every path this test takes is a dismissal, so a
                // status that moved would mean the dialog had done something to the mechanism.
                let status = r.provide_root_context(create_global_updates);
                (ask, status)
            },
            1.,
        )
    }

    fn open(runner: &mut TestingRunner, ask: &mut AskSlot) {
        runner.sync_and_update();
        ask.set(Some(UpdateAsk {
            version: "0.4.0".into(),
            page_url: "https://example.invalid/releases/v0.4.0".into(),
        }));
        runner.sync_and_update();
        runner.sync_and_update();
    }

    fn texts(runner: &TestingRunner) -> Vec<String> {
        runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
    }

    /// Press the **lowest** run reading `text` — the action strip, since the header can carry
    /// the same words as the button that answers it.
    fn click_action(runner: &mut TestingRunner, text: &str) {
        let area = runner
            .find_many(|node, element| {
                Label::try_downcast(element)
                    .filter(|l| l.text == text)
                    .map(|_| node.layout().area)
            })
            .into_iter()
            .max_by(|a, b| a.min_y().total_cmp(&b.min_y()))
            .unwrap_or_else(|| panic!("no text run {text:?} in the tree"));
        let point = (
            (area.min_x() + area.width() / 2.) as f64,
            (area.min_y() + area.height() / 2.) as f64,
        );
        runner.move_cursor(point);
        runner.click_cursor(point);
        runner.sync_and_update();
        runner.sync_and_update();
    }

    /// **The card states what it will restart into, and what it does not promise.** The version
    /// is the subject line, and the body says the close confirms still get their say rather
    /// than re-asking their question here.
    #[test]
    fn the_confirm_names_the_version_and_leaves_the_close_question_alone() {
        let (mut runner, (mut ask, _)) = runner();
        open(&mut runner, &mut ask);

        let texts = texts(&runner);
        assert_eq!(texts[0], "Restart to update");
        assert_eq!(texts[1], "Strata 0.4.0");
        let body = texts
            .iter()
            .find(|t| t.contains("new version"))
            .expect("the body copy");
        assert!(body.contains("would ask before quitting still asks"));
        assert!(texts.iter().any(|t| t == "Release notes"));
        assert!(texts.iter().any(|t| t == "Restart now"));
    }

    /// **Dismissing keeps everything**: the staged bundle is untouched and the status is still
    /// whatever it was, so the press can simply be made again. Both dismissal paths, because
    /// Esc runs a different closure from the button and the two could be swapped with the
    /// suite still green.
    #[test]
    fn both_dismissals_close_the_dialog_and_move_no_status() {
        let (mut runner, (mut ask, status)) = runner();

        open(&mut runner, &mut ask);
        click_action(&mut runner, "Not now");
        assert!(ask.peek().is_none(), "the button left the dialog up");
        assert_eq!(*status.peek(), Update::Idle);

        open(&mut runner, &mut ask);
        runner.press_key(Key::Named(NamedKey::Escape));
        runner.sync_and_update();
        assert!(ask.peek().is_none(), "Esc left the dialog up");
        assert_eq!(*status.peek(), Update::Idle);
    }
}
