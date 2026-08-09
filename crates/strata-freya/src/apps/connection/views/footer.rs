//! The window's footer — why Save is blocked on the left, Cancel and Save on the right.
//!
//! **This is the only thing in the window that writes anything.** Cancel just closes; nothing is
//! committed until Save.
//!
//! Save does four things and registers none of them itself:
//!
//! 1. writes the def onto the shared project store — removing the row it is **moving from**
//!    first, when the edit changed the bucket or the provider and therefore the connection's
//!    identity;
//! 2. persists through the funnel and **gates on the answer** — a connection the project file
//!    never heard about is gone on the next open, which is what P4-15 exists to stop being
//!    silent;
//! 3. drops the object store the old URL registered, the one call the scan driver cannot make:
//!    `engine::store::connect` only ever sees the def it is given, so nothing else would ever
//!    take that store back out (`Engine::disconnect` — the same call Forget makes);
//! 4. asks the project window's one scan driver for a whole-catalog pass, and leaves this window
//!    watching its row ([`super::use_watch_connection`]).
//!
//! **A whole-catalog pass, not a connection-shaped one.** `plan_scan` puts connections in
//! `ScanScope::All` alone, deliberately: the case that needs a re-connect is a region corrected
//! or an `aws sso login` run, which is exactly this window — and every table over the bucket was
//! registered against the store this save replaces, so re-registering the connection alone would
//! leave them answering from a store that is no longer there. Save is the ↻ the user would
//! otherwise press, with the def written first.

use freya::prelude::*;
use freya::radio::{use_radio_station, RadioStation};

use crate::apps::connection::{ConnectionCtx, ConnectionTarget, Status};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::{log_event, use_report, LogLevel, ReportCtx};
use crate::apps::project::{
    persisted_defs, refresh_catalog, Catalog, CatalogRescan, ProjChan, ProjectState,
};
use crate::components::divider::Divider;
use crate::components::form::form_theme;
use crate::components::typography::{Control, Path};
use crate::components::window::window_theme;
use crate::components::ACTION_HEIGHT;

/// The strip's inset (canvas `padding: var(--sp-4) var(--sp-5)`).
const FOOTER_PADDING: Gaps = Gaps::new(12., 16., 12., 16.);

#[derive(PartialEq)]
pub struct Footer;

impl Component for Footer {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let form = form_theme();
        let ctx = use_consume::<ConnectionCtx>();
        let project = use_radio_station::<ProjectState, ProjChan>();
        let rescan = use_consume::<CatalogRescan>();
        let catalog = use_consume::<Catalog>();
        let engine = use_consume::<EngineCtx>();
        let report = use_report();
        let platform = use_hook(Platform::get);

        let connecting = matches!(*ctx.status.read(), Status::Connecting(_));
        // The project window's driver **drops** a request raised while a pass is already in
        // flight, and nothing retries it — so pressing Save then would leave the row `Loading`
        // for good. The sidebar's ↻ answers this by disabling itself for the duration; so does
        // the Configure window's Save; so does this. Subscribes, so the button comes back by
        // itself when the pass settles.
        let scanning = catalog.read().is_scanning();
        // What the *draft* can answer, then the one thing only the store can (a URL another
        // connection already holds), and last the one nobody can — see [`save_note`].
        let note = save_note(
            ctx.draft
                .read()
                .blocker()
                .or_else(|| url_clash(ctx, project)),
            scanning,
        );

        let cancel = {
            Button::new()
                .height(Size::px(ACTION_HEIGHT))
                // Always available: a pass in flight is the project window's, and it answers on
                // the pane's row whether this window is here to watch or not.
                .on_press(move |_: Event<PressEventData>| platform.close_current_window())
                .child(Control::new("Cancel"))
        };

        let save = Button::new()
            .filled()
            .height(Size::px(ACTION_HEIGHT))
            .enabled(!connecting && note.is_none())
            .on_press({
                move |_: Event<PressEventData>| save(ctx, project, rescan, engine.clone(), report)
            })
            .child(Control::new(match connecting {
                true => "Connecting…",
                false => "Save",
            }));

        rect()
            .width(Size::fill())
            .vertical()
            .child(Divider::horizontal().color(win.border_fill))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(12.)
                    .padding(FOOTER_PADDING)
                    .background(win.background)
                    // Why the button is off, rather than an unexplained dead control. A
                    // *connection* failure is not shown here — it is a sentence the engine wrote,
                    // and it has its own block at the end of the body.
                    .child(
                        rect().width(Size::flex(1.)).maybe_child(
                            note.filter(|_| !connecting).map(|why| {
                                Path::new(why).color(form.hint_color).max_lines(2).wrap()
                            }),
                        ),
                    )
                    .child(cancel)
                    .child(save),
            )
    }
}

/// The one line the footer shows about why Save is off — and **the same value that disables it**,
/// so the button and its explanation cannot disagree.
///
/// `blocker` comes **first**, ahead of the re-scan, for the Configure footer's reason: a note
/// should name the next thing the user can *do*, and a blank region is fixable now while the scan
/// will very likely settle while they are fixing it.
fn save_note(blocker: Option<String>, scanning: bool) -> Option<String> {
    blocker.or_else(|| {
        scanning
            .then(|| "The catalog is being re-scanned. Save is available when it settles.".into())
    })
}

/// The one blocker the draft cannot see: a URL another connection already holds.
///
/// `upsert_connection` replaces on `url()`, so without this an edit that moved a bucket onto an
/// existing connection's would silently take that connection's def out from under it — the same
/// hazard the Configure window's name clash guards, one key along. On an edit the connection's
/// own URL does not clash with itself.
///
/// Matched case-**sensitively**, like every other connection lookup: a URL is not a SQL
/// identifier, and the object-store registry matches it verbatim.
fn url_clash(ctx: ConnectionCtx, project: RadioStation<ProjectState, ProjChan>) -> Option<String> {
    let url = ctx.draft.read().def().url();
    if ctx.target.read().editing() == Some(url.as_str()) {
        return None;
    }
    project
        .peek()
        .connections
        .iter()
        .any(|c| c.def.url() == url)
        .then(|| format!("'{url}' is already a connection in this project."))
}

/// Write the def, persist it, drop what the old URL registered, and ask for the pass. See the
/// module doc.
fn save(
    mut ctx: ConnectionCtx,
    mut project: RadioStation<ProjectState, ProjChan>,
    rescan: CatalogRescan,
    engine: EngineCtx,
    report: ReportCtx,
) {
    let def = ctx.draft.peek().def();
    let url = def.url();
    // The URL this window opened on, when the edit has moved off it — a changed bucket *or* a
    // changed provider, since the URL is both.
    let moved_from = ctx
        .target
        .peek()
        .editing()
        .filter(|old| *old != url)
        .map(str::to_string);

    // The write and the persist are one step, and the persist is checked. `upsert_connection`
    // puts the row back in `Reg::Loading`, which is already the state this window renders as
    // busy.
    let landed = {
        let mut p = project.write_channel(ProjChan::Connections);
        if let Some(old) = &moved_from {
            p.remove_connection(old);
        }
        p.upsert_connection(def);
        persisted_defs(&p, report)
    };
    // The store write above has already happened, so the row exists either way and **must** be
    // registered either way: returning here would leave it in `Reg::Loading` with nothing left to
    // answer it — a permanent spinner in the pane. So the pass is asked for below whatever the
    // persist said; what the failure changes is only what this window claims.
    //
    // `persisted_defs` has already logged the cause, in the project window where the user will
    // look for it. Saying so here too would be the same failure twice.
    if !landed {
        ctx.status.set(Status::Failed(
            "The connection is registered, but the project file could not be written, so it will \
             be gone when this project is reopened."
                .into(),
        ));
    } else {
        ctx.status.set(Status::Connecting(url.clone()));
    }
    // **The window is now editing what it just wrote.** Without this a second Save — after a
    // refused connection, say — measures `moved_from` against the URL the window *opened* on, so
    // the row the first Save created is never removed and the project keeps a phantom connection
    // under the intermediate URL.
    {
        let mut target = ctx.target;
        target.set(ConnectionTarget::Edit(url.clone()));
    }

    // A moved identity leaves the engine still holding a store under the old URL, which the scan
    // pass cannot know about — it registers the defs, and this one no longer has a def. Dropping
    // it is the one engine call this window makes; `register_pass` is additive by contract, so
    // nothing else ever would.
    if let Some(old) = &moved_from {
        engine.disconnect(old);
        log_event(
            report.log,
            LogLevel::Info,
            format!("Moved connection '{old}' to '{url}'"),
        );
    }

    refresh_catalog(rescan);
}

#[cfg(test)]
mod tests {
    use super::save_note;

    #[test]
    fn an_actionable_blocker_outranks_the_re_scan() {
        let blocker = || Some("An S3 connection needs a region.".to_string());
        assert_eq!(save_note(blocker(), true), blocker());
        assert_eq!(save_note(blocker(), false), blocker());
    }

    #[test]
    fn a_re_scan_is_explained_once_it_is_the_only_thing_left() {
        let note = save_note(None, true).expect("a scanning footer says why");
        assert!(note.contains("re-scanned"), "{note}");
    }

    #[test]
    fn nothing_to_say_when_save_is_available() {
        assert_eq!(save_note(None, false), None);
    }
}
