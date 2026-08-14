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
//! **Deliberately quiet — where nobody asked.** No toast system and no badge on every window:
//! the rail draws nothing at all for `Idle`, `UpToDate`, `Checking` and `Failed`, and a failed
//! check is a log line rather than chrome. The rail nags nobody; App ▸ *Check for Updates…* is
//! how you ask — and because it is a question, it is answered where it was asked, in
//! [`UpdateConfirm`]'s report card. A check that found nothing to install still has an answer.

use freya::prelude::*;
use strata_core::update::{open_page, Site};
use strata_core::util::human_bytes;

use crate::components::dialog::{Dialog, DialogHeader};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{R_2, SP_3, SP_4};
use crate::components::tones::{tones, Tones};
use crate::components::typography::{scale, Control, Prose, Title};
use crate::state::{check, download, install, install_site, Update, UpdateStatus, CURRENT};
use crate::theme::{use_roles, Role, RoleColors};

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
    ///
    /// It carries the release's notes as well as its version, because the restart card is the
    /// last thing a user sees before the swap and may be the *first* — a download started from
    /// the rail never draws the report card, so this is the only place its changelog can be
    /// read. The rail ignores the field, as it ignores `page_url`.
    Restart {
        version: String,
        page_url: String,
        notes: String,
    },
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
                ..
            } => match (asset, site) {
                (Some(_), Site::Writable(_)) => Affordance::Get {
                    version: version.clone(),
                },
                _ => Affordance::Page {
                    version: version.clone(),
                    page_url: page_url.clone(),
                },
            },
            Update::Ready {
                version,
                page_url,
                notes,
                ..
            } => Affordance::Restart {
                version: version.clone(),
                page_url: page_url.clone(),
                notes: notes.clone(),
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
    let offer = Affordance::of(&status.peek(), install_site());
    match offer {
        Affordance::Inert | Affordance::Downloading { .. } => {}
        Affordance::Check => check(status),
        Affordance::Get { .. } => download(status),
        Affordance::Page { page_url, .. } => open_page(&page_url),
        Affordance::Restart {
            version,
            page_url,
            notes,
        } => {
            let mut ask = ask;
            ask.set(Some(UpdateAsk::Restart {
                version,
                page_url,
                notes,
            }));
        }
    }
}

/// **Ask what the situation is** — App ▸ *Check for Updates…*'s own press, drained by the
/// focused window from [`use_file_menu`](crate::menu::use_file_menu).
///
/// The rail and the menubar want different things from the same status, which is why this is
/// not [`press`]: the rail's action *is* the offer, so pressing it downloads, opens the page or
/// asks the restart question, and every other status draws no action at all. A menu item is a
/// *question*, and a question is owed an answer even when the answer is "nothing to install" —
/// so this raises the dialog first and lets the status land in it. Two arms divert: a staged
/// update is still the restart question, which is `press`'s to raise, and a dev build offers
/// nothing at all.
///
/// **It checks over an offer it already has**, because the item says *check*: an `Available`
/// learned at startup can be a release behind by the time somebody asks, and reporting it
/// without asking again would answer the question with a fact nobody re-established. `check`
/// stands itself down while a job is running, so pressing twice costs nothing, and a download
/// in flight is the one arm that only reports — there the offer in hand is the one being
/// installed.
///
/// **The affordance is bound before the match, never resolved in its scrutinee**: a `peek`
/// guard in a `match` head lives for the whole match, and the `check` below writes that same
/// state — the generational-borrow panic AGENTS.md §2 records for the confirm dialogs. It cost
/// exactly this function once already.
///
/// **Call it from a Freya scope**, for [`press`]'s reason — the check arms start a check.
pub fn raise(status: UpdateStatus, ask: AskSlot) {
    let mut slot = ask;
    let offer = Affordance::of(&status.peek(), install_site());
    match offer {
        Affordance::Inert => {}
        Affordance::Restart { .. } => press(status, ask),
        Affordance::Downloading { .. } => slot.set(Some(UpdateAsk::Report)),
        Affordance::Check | Affordance::Get { .. } | Affordance::Page { .. } => {
            slot.set(Some(UpdateAsk::Report));
            check(status);
        }
    }
}

/// **What a window is asking about updates**, carried on its slot.
///
/// Two questions, because the two presses ask different ones. The rail only ever has the
/// restart to ask about — everything else it offers, it performs. The menubar item asks what
/// the situation *is*, and that one has to be answerable while the check is still running and
/// after it has found nothing.
#[derive(Clone, PartialEq, Debug)]
pub enum UpdateAsk {
    /// **Restart now?** — raised by a press on [`Affordance::Restart`]: what the app will come
    /// back as, and where to read what changed.
    ///
    /// It carries the version — and the notes it will restart into — rather than reading the
    /// status, on the slot pattern's own terms: the dialog states what it was raised about, and
    /// cannot end up asking about one version while showing another's changelog.
    Restart {
        version: String,
        page_url: String,
        notes: String,
    },
    /// **What is the situation?** — raised by [`raise`], and answered by whatever the status
    /// settles on while the card is up: a check in flight, nothing newer, an offer, a
    /// download's progress, or the failure in its own words.
    ///
    /// It carries nothing, deliberately: unlike the restart it is not a question about one
    /// version but a view of the app-global status, and the card follows it.
    Report,
}

/// **One window's update-question slot** — the confirm pattern's `State<Option<T>>`, provided
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

/// **The window's update dialog** — the restart gate and the report card, mounted at the
/// launcher root and the project root (one component, two mounts: the slot is per window, the
/// status behind it is the app's one).
///
/// Which card is up is the slot's [`UpdateAsk`], never the status: the restart is a question
/// about one staged version, and the report is a view of whatever the status is doing.
///
/// The restart asks *this* question and no other. Confirming is the ordinary quit, so a window
/// with a running query still gets its own close confirm afterwards — "lose the running query?"
/// stays that dialog's question, and re-asking it here would be a second, weaker copy of it.
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
        let tones = tones();

        match asked {
            None => rect().into_element(),
            Some(UpdateAsk::Restart {
                version,
                page_url,
                notes,
            }) => restart_card(ask, status, roles, tones, version, page_url, notes),
            Some(UpdateAsk::Report) => report_card(ask, status, roles, tones),
        }
    }
}

/// **Restart now?** — the one gate in front of the install.
///
/// It draws the changelog too, because this card can be the first sight of it: a download
/// started from the launcher rail never raises the report card.
fn restart_card(
    ask: AskSlot,
    status: UpdateStatus,
    roles: RoleColors,
    tones: Tones,
    version: String,
    page_url: String,
    notes: String,
) -> Element {
    let mut ask = ask;
    let restart = move |()| {
        let mut ask = ask;
        ask.set(None);
        install(status);
    };

    let header = DialogHeader::new(
        IconName::Download,
        tones.info,
        titles(roles, "Restart to update", format!("Strata {version}")),
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
                .maybe_child((!notes.is_empty()).then(|| Changelog { notes }))
                .child(release_notes(roles, page_url)),
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

/// **What the check found** — the answer to App ▸ *Check for Updates…*, whatever it turns out
/// to be.
///
/// The card reads the live status, so a check raised here settles *in* it rather than behind
/// it, and a download started from it reports its own progress. The words are [`Report`]'s and
/// the action is [`Affordance`]'s: pressing it is [`press`], the same funnel the rail's action
/// uses, so this card can offer nothing the rail would not.
fn report_card(ask: AskSlot, status: UpdateStatus, roles: RoleColors, tones: Tones) -> Element {
    let mut ask = ask;
    let update = status.read().clone();
    let offer = Affordance::of(&update, install_site());
    let report = Report::of(&update, &offer, tones);

    let offered = offer.action();
    let confirm = move |()| match offered.is_some() {
        true => press(status, ask),
        false => ask.set(None),
    };

    let header = DialogHeader::new(
        report.icon,
        report.tone,
        titles(roles, report.title, report.subject),
    );

    let dialog = Dialog::new()
        .on_dismiss(move |()| ask.set(None))
        .on_confirm(confirm)
        .header(header)
        .body(
            rect()
                .width(Size::fill())
                .vertical()
                .spacing(SP_3)
                .child(
                    Prose::new(report.body)
                        .color(roles.get(Role::TextMuted))
                        .wrap(),
                )
                .maybe_child(report.notes.map(|notes| Changelog { notes }))
                .maybe_child(report.page.map(|page_url| release_notes(roles, page_url))),
        )
        .action(
            Button::new()
                .flat()
                .on_press(move |_| ask.set(None))
                .child(Control::new(match offered.is_some() {
                    true => "Not now",
                    false => "Close",
                })),
        );

    match offered {
        None => dialog,
        Some(label) => dialog.action(
            Button::new()
                .filled()
                .on_press(move |_| press(status, ask))
                .child(Control::new(label)),
        ),
    }
    .into_element()
}

/// **What the report card says about the status behind it** — the glyph, the tone, the title,
/// the subject line under it and the one sentence of body.
///
/// Pure, so the wording is testable without a window, and one match over the status rather than
/// a branch per widget: the card cannot end up with a tick beside "the update failed". The
/// *offer* half is [`Affordance`]'s — the subject is its own `note`, so the card and the rail
/// describe one update in one vocabulary, and the degraded arm's body is the only place this
/// looks at which offer it is.
struct Report {
    icon: IconName,
    tone: Color,
    title: &'static str,
    subject: String,
    body: String,
    /// The release page, where the status names one. Absent while nothing newer is known,
    /// which is also the only time there would be nothing to read there.
    page: Option<String>,
    /// **What changed**, in the release's own Markdown — the three offer states carry it, and
    /// an empty body is `None` rather than an empty panel.
    notes: Option<String>,
}

impl Report {
    fn of(status: &Update, offer: &Affordance, tones: Tones) -> Report {
        let subject = offer.note().unwrap_or_else(|| format!("Strata {CURRENT}"));
        let (page, notes) = match status {
            Update::Available {
                page_url, notes, ..
            }
            | Update::Downloading {
                page_url, notes, ..
            }
            | Update::Ready {
                page_url, notes, ..
            } => (
                Some(page_url.clone()),
                (!notes.is_empty()).then(|| notes.clone()),
            ),
            _ => (None, None),
        };
        let (icon, tone, title, body) = match status {
            Update::Idle | Update::Checking => (
                IconName::Reload,
                tones.info,
                "Checking for updates",
                "Asking GitHub whether there is a newer release.".to_string(),
            ),
            Update::UpToDate => (
                IconName::Check,
                tones.ok,
                "Strata is up to date",
                format!("Strata {CURRENT} is the latest release."),
            ),
            Update::Available { .. } => (
                IconName::Download,
                tones.info,
                "An update is available",
                match offer {
                    Affordance::Page { .. } => "This copy cannot be updated in place. Open the \
                                                release page to install it by hand."
                        .to_string(),
                    _ => "Downloading it leaves the running version in place until you restart."
                        .to_string(),
                },
            ),
            Update::Downloading { .. } => (
                IconName::Download,
                tones.info,
                "Downloading the update",
                "Strata keeps running. The update is installed by a restart.".to_string(),
            ),
            Update::Ready { .. } => (
                IconName::Download,
                tones.info,
                "The update is ready",
                "Restart to install it.".to_string(),
            ),
            Update::Failed { why } => (
                IconName::Warning,
                tones.warning,
                "The update failed",
                why.clone(),
            ),
        };
        Report {
            icon,
            tone,
            title,
            subject,
            body,
            page,
            notes,
        }
    }
}

/// A dialog header's title run: the question over its subject.
fn titles(roles: RoleColors, title: &str, subject: String) -> impl IntoElement {
    rect()
        .vertical()
        .child(Title::new(title.to_string()).color(roles.get(Role::Text)))
        .child(
            Prose::new(subject)
                .color(roles.get(Role::TextPlaceholder))
                .text_overflow(TextOverflow::Ellipsis),
        )
}

/// **What changed, rendered** — the release's own Markdown through the app's
/// [`MarkdownViewer`], in a fixed, scrollable well.
///
/// The viewer is the chat pane's, so a heading is a heading and a bullet is a bullet here too,
/// and there is one Markdown dress in the app rather than a second one grown on this card. What
/// differs is the **scale**: a release note is supporting text in a 420px dialog, not the
/// pane's reading column, so the sizes are overridden per instance off the app's own type scale
/// through the fork's `MarkdownViewer::theme` — how every other themed component takes a local
/// dress. Nothing is hardcoded and the chat pane is untouched.
///
/// A component rather than a builder function because it reads the scale, and a hook must not
/// run behind an `if`: this is drawn only for a release that wrote notes.
///
/// The well is a **fixed height** because the card must not grow with the release: a long
/// changelog scrolls inside its own box instead of pushing the action strip off the window.
#[derive(PartialEq)]
struct Changelog {
    notes: String,
}

impl Component for Changelog {
    fn render(&self) -> impl IntoElement {
        let roles = use_roles();
        let scale = scale();
        let small = scale.caption.size;

        rect()
            .width(Size::fill())
            .height(Size::px(NOTES_HEIGHT))
            .corner_radius(R_2)
            .background(roles.get(Role::SurfaceBackground))
            .border(Border::new().width(1.).fill(roles.get(Role::Border)))
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::fill())
                    .child(
                        rect().width(Size::fill()).padding((SP_3, SP_4)).child(
                            MarkdownViewer::new(self.notes.clone())
                                .width(Size::fill())
                                .theme(
                                    MarkdownViewerThemePartial::new()
                                        .color(roles.get(Role::TextMuted))
                                        .color_link(roles.get(Role::TextAccent))
                                        .paragraph_size(small)
                                        .heading_h1(scale.body.size)
                                        .heading_h2(scale.body.size)
                                        .heading_h3(small)
                                        .heading_h4(small)
                                        .heading_h5(small)
                                        .heading_h6(small)
                                        .code_font_size(scale.meta.size)
                                        .table_font_size(scale.meta.size),
                                ),
                        ),
                    ),
            )
    }
}

/// The changelog well's box. Tall enough for the shape of a release note — a heading and a few
/// bullets — and short enough that the card still fits a small window with the body copy, the
/// link and the action strip above and below it.
const NOTES_HEIGHT: f32 = 132.;

/// The link-out to what changed, in both cards' body.
fn release_notes(roles: RoleColors, page_url: String) -> impl IntoElement {
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
    )
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
///
/// [`Report`] is tested the same way and for the same reason — the report card's whole job is
/// the wording, and it is the only place the app says anything at all about a check that found
/// nothing. [`raise`] itself is untestable here on `press`'s grounds.
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
            notes: NOTES.into(),
            asset,
        }
    }

    /// A release body in the shape GitHub hands one over: Markdown, shown as written.
    const NOTES: &str = "## What's new\n\n- Charts got a Shape panel";

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
            notes: NOTES.into(),
            staged: PathBuf::from("/tmp/strata-update-x/Strata.app"),
        };
        let offer = Affordance::of(&status, &writable());
        assert_eq!(offer.action(), Some("Restart to update"));
        assert_eq!(offer.note().as_deref(), Some("Strata 0.4.0 is downloaded"));
        assert!(
            matches!(&offer, Affordance::Restart { notes, .. } if notes == NOTES),
            "the staged release's notes did not reach the restart question"
        );
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
                    notes: NOTES.into(),
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
                let status = r.provide_root_context(create_global_updates);
                (ask, status)
            },
            1.,
        )
    }

    fn open(runner: &mut TestingRunner, ask: &mut AskSlot, question: UpdateAsk) {
        runner.sync_and_update();
        ask.set(Some(question));
        runner.sync_and_update();
        runner.sync_and_update();
    }

    fn restart_ask() -> UpdateAsk {
        UpdateAsk::Restart {
            version: "0.4.0".into(),
            page_url: "https://example.invalid/releases/v0.4.0".into(),
            notes: NOTES.into(),
        }
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
    /// is the subject line, the changelog is under it — this card is the only sight of it when
    /// the download was started from the rail — and the body says the close confirms still get
    /// their say rather than re-asking their question here.
    #[test]
    fn the_confirm_names_the_version_and_leaves_the_close_question_alone() {
        let (mut runner, (mut ask, _)) = runner();
        open(&mut runner, &mut ask, restart_ask());

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
        assert!(
            spans(&runner)
                .iter()
                .any(|t| t == "Charts got a Shape panel"),
            "a restart raised over a rail download showed no changelog"
        );
    }

    /// **Dismissing keeps everything**: the staged bundle is untouched and the status is still
    /// whatever it was, so the press can simply be made again. Both dismissal paths, because
    /// Esc runs a different closure from the button and the two could be swapped with the
    /// suite still green.
    #[test]
    fn both_dismissals_close_the_dialog_and_move_no_status() {
        let (mut runner, (mut ask, status)) = runner();

        open(&mut runner, &mut ask, restart_ask());
        click_action(&mut runner, "Not now");
        assert!(ask.peek().is_none(), "the button left the dialog up");
        assert_eq!(*status.peek(), Update::Idle);

        open(&mut runner, &mut ask, restart_ask());
        runner.press_key(Key::Named(NamedKey::Escape));
        runner.sync_and_update();
        assert!(ask.peek().is_none(), "Esc left the dialog up");
        assert_eq!(*status.peek(), Update::Idle);
    }

    /// **The question the menubar asks is answered where it was asked, and it follows the
    /// status.** The card is up while the check runs and is still up for the answer — which for
    /// an up-to-date app is the only thing the app says about it at all, since the rail draws
    /// nothing for that status by design. Dismissing it moves nothing.
    #[test]
    fn the_report_card_follows_the_status_and_closes_on_a_press() {
        let (mut runner, (mut ask, mut status)) = runner();
        open(&mut runner, &mut ask, UpdateAsk::Report);
        assert_eq!(texts(&runner)[0], "Checking for updates");

        status.set(Update::UpToDate);
        runner.sync_and_update();
        runner.sync_and_update();
        let shown = texts(&runner);
        assert_eq!(shown[0], "Strata is up to date");
        assert!(
            shown.iter().any(|t| t.contains("is the latest release")),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|t| t == "Close"),
            "a report with nothing to install offered more than a dismissal: {shown:?}"
        );

        click_action(&mut runner, "Close");
        assert!(ask.peek().is_none(), "the report card stayed up");
        assert_eq!(*status.peek(), Update::UpToDate);
    }

    /// Four distinct tones, so a test can tell which one the card picked.
    fn probe_tones() -> Tones {
        Tones {
            error: Color::RED,
            warning: Color::YELLOW,
            info: Color::BLUE,
            ok: Color::GREEN,
        }
    }

    /// **A check that found nothing still has an answer**, and it names the version that is
    /// running — the one thing the user is asking about when nothing is on offer.
    #[test]
    fn nothing_to_install_is_reported_as_the_latest_release() {
        let report = Report::of(&Update::UpToDate, &Affordance::Check, probe_tones());
        assert_eq!(report.title, "Strata is up to date");
        assert_eq!(report.subject, format!("Strata {CURRENT}"));
        assert_eq!(
            report.body,
            format!("Strata {CURRENT} is the latest release.")
        );
        assert!(report.icon == IconName::Check);
        assert_eq!(report.tone, probe_tones().ok);
        assert!(report.page.is_none(), "a release page for no release");
        assert!(report.notes.is_none(), "a changelog for no release");
    }

    /// A check in flight says so rather than answering early — `Idle` too, which is what the
    /// card shows in the frame between the press and the status moving.
    #[test]
    fn a_check_in_flight_says_so() {
        for status in [Update::Idle, Update::Checking] {
            let report = Report::of(&status, &Affordance::Check, probe_tones());
            assert_eq!(report.title, "Checking for updates", "{status:?}");
            assert_eq!(report.subject, format!("Strata {CURRENT}"));
        }
    }

    /// **A failure is reported in its own words**, and wears the tone that goes with them: the
    /// status carries the only description of what went wrong, and the card must not gloss it.
    #[test]
    fn a_failure_is_reported_in_its_own_words() {
        let status = Update::Failed {
            why: "the release could not be reached".into(),
        };
        let report = Report::of(&status, &Affordance::Check, probe_tones());
        assert_eq!(report.title, "The update failed");
        assert_eq!(report.body, "the release could not be reached");
        assert!(report.icon == IconName::Warning);
        assert_eq!(report.tone, probe_tones().warning);
    }

    /// **The card describes an offer in the rail's own words** — the subject line *is*
    /// `Affordance::note`, so one update is never described two ways — and the degraded offer's
    /// body sends the user to the page rather than promising an install the app cannot do.
    #[test]
    fn an_offer_is_reported_in_the_affordance_s_words() {
        let status = available(Some(asset()));
        let report = Report::of(
            &status,
            &Affordance::of(&status, &writable()),
            probe_tones(),
        );
        assert_eq!(report.title, "An update is available");
        assert_eq!(report.subject, "Strata 0.4.0 is available");
        assert!(report.body.contains("until you restart"), "{}", report.body);
        assert_eq!(
            report.page.as_deref(),
            Some("https://example.invalid/releases/v0.4.0")
        );

        let status = available(None);
        let degraded = Report::of(
            &status,
            &Affordance::of(&status, &writable()),
            probe_tones(),
        );
        assert!(
            degraded.body.contains("release page"),
            "the degraded offer promised an install: {}",
            degraded.body
        );
    }

    /// **What changed travels with the offer, as written.** Every state that has a release
    /// behind it carries the notes, so the panel does not vanish the moment the download
    /// starts — and the Markdown is handed over untouched, because the card shows it rather
    /// than rendering it. A release with an empty body is no panel rather than an empty one.
    #[test]
    fn every_offer_state_carries_the_changelog_unrendered() {
        let staged = |notes: &str| Update::Ready {
            version: "0.4.0".into(),
            page_url: "https://example.invalid/releases/v0.4.0".into(),
            notes: notes.into(),
            staged: PathBuf::from("/tmp/strata-update-x/Strata.app"),
        };
        let downloading = Update::Downloading {
            version: "0.4.0".into(),
            page_url: "https://example.invalid/releases/v0.4.0".into(),
            notes: NOTES.into(),
            got: 1 << 20,
            total: Some(4 << 20),
        };

        for status in [available(Some(asset())), downloading, staged(NOTES)] {
            let report = Report::of(
                &status,
                &Affordance::of(&status, &writable()),
                probe_tones(),
            );
            assert_eq!(report.notes.as_deref(), Some(NOTES), "{status:?}");
        }

        let bare = staged("");
        let report = Report::of(&bare, &Affordance::of(&bare, &writable()), probe_tones());
        assert!(
            report.notes.is_none(),
            "an empty body became an empty panel"
        );
    }

    /// The panel is up while an update is on offer — **rendered**, so the heading's `##` is a
    /// heading rather than two characters of body text — and it is gone when there is nothing
    /// to install.
    #[test]
    fn the_report_card_renders_the_changelog_only_when_there_is_an_update() {
        let (mut runner, (mut ask, mut status)) = runner();
        open(&mut runner, &mut ask, UpdateAsk::Report);
        status.set(available(Some(asset())));
        runner.sync_and_update();
        runner.sync_and_update();
        assert_eq!(texts(&runner)[0], "An update is available");
        let notes = spans(&runner);
        assert!(
            notes.iter().any(|t| t == "Charts got a Shape panel"),
            "the changelog was dropped: {notes:?}"
        );
        assert!(
            notes.iter().any(|t| t == "What's new"),
            "the heading was left as Markdown source: {notes:?}"
        );

        status.set(Update::UpToDate);
        runner.sync_and_update();
        runner.sync_and_update();
        let notes = spans(&runner);
        assert!(
            !notes.iter().any(|t| t.contains("Shape panel")),
            "the changelog outlived the offer: {notes:?}"
        );
    }

    /// The Markdown viewer paints its text as paragraph spans rather than labels — a heading is
    /// one paragraph, a bullet's content another — so the changelog is read from those.
    fn spans(runner: &TestingRunner) -> Vec<String> {
        runner.find_many(|_, element| {
            Paragraph::try_downcast(element)
                .map(|p| p.spans.iter().map(|span| span.text.as_ref()).collect())
        })
    }
}
