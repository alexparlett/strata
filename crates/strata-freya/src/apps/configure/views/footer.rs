//! The window's footer — why Save is blocked on the left, Cancel and Save on the right.
//!
//! **This is the only thing in the window that writes anything.** Cancel just closes; nothing
//! is committed until Save.
//!
//! Save does four things and registers none of them itself:
//!
//! 1. writes the def onto the shared catalog store (a **rename** removes the old row first, or
//!    the catalog would keep a row and a registration nobody can reach);
//! 2. persists through the funnel and **gates on the answer** — a registration the project file
//!    never heard about reverts on the next open, which is what P4-15 exists to stop being
//!    silent;
//! 3. asks the project window's one scan driver for a pass over that table, which is the same
//!    pass project open and the sidebar's ↻ use, so there is one implementation of "make the
//!    engine match the defs" and the per-def log entries come from it as they always have;
//! 4. leaves the window watching its row (`views::use_watch_registration`).

use freya::prelude::*;
use freya::radio::{use_radio_station, RadioStation};

use crate::apps::configure::{ConfigureCtx, ConfigureTarget, Status};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::{log_event, LogCtx, LogLevel};
use crate::apps::project::{
    persisted, refresh_catalog, refresh_table, Catalog, CatalogRescan, ProjChan, ProjectState,
};
use crate::components::divider::Divider;
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
        let form = crate::components::form::form_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let project = use_radio_station::<ProjectState, ProjChan>();
        let rescan = use_consume::<CatalogRescan>();
        let catalog = use_consume::<Catalog>();
        let engine = use_consume::<EngineCtx>();
        let log = use_consume::<LogCtx>();
        let platform = use_hook(Platform::get);

        let registering = matches!(*ctx.status.read(), Status::Registering(_));
        // The project window's driver **drops** a request raised while a pass is already in
        // flight, and nothing retries it — so pressing Save then would leave the row `Loading`
        // for good. The sidebar's ↻ answers this by disabling itself for the duration; so does
        // this. Subscribes, so the button comes back by itself when the pass settles.
        let scanning = catalog.read().is_scanning();
        // What the *draft* can answer, the one thing only the catalog can (a name another def
        // already owns), and last the one nobody can — see [`save_note`].
        let note = save_note(
            ctx.draft
                .read()
                .blocker()
                .or_else(|| name_clash(ctx, project)),
            scanning,
        );

        let cancel = {
            let platform = platform.clone();
            Button::new()
                .height(Size::px(ACTION_HEIGHT))
                // Always available: a registration in flight is the project window's, and it
                // answers on the catalog row whether this window is here to watch or not.
                .on_press(move |_: Event<PressEventData>| platform.close_current_window())
                .child(Control::new("Cancel"))
        };

        let save = Button::new()
            .filled()
            .height(Size::px(ACTION_HEIGHT))
            .enabled(!registering && note.is_none())
            .on_press({
                let engine = engine.clone();
                move |_: Event<PressEventData>| save(ctx, project, rescan, engine.clone(), log)
            })
            .child(Control::new(match registering {
                true => "Validating…",
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
                    // registration failure is *not* shown here — it is a paragraph the engine
                    // wrote, and it has its own block at the end of the body.
                    .child(
                        rect().width(Size::flex(1.)).maybe_child(
                            note.filter(|_| !registering).map(|why| {
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
/// so the button and its explanation cannot disagree. They did: this used to be two expressions,
/// and the scanning half was wired into `enabled` but not into the text, leaving a dead button
/// with nothing beside it.
///
/// `blocker` — what the draft and the catalog can answer — comes **first**, ahead of the
/// re-scan. That is the opposite of what this file first claimed, and the reason is that a note
/// should always name the next thing the user can *do*: a blank name is fixable now, and the
/// scan will very likely settle while they are fixing it. Leading with the scan would say "wait"
/// to someone who still has a field to fill in, and then say "name it" once they had.
fn save_note(blocker: Option<String>, scanning: bool) -> Option<String> {
    blocker.or_else(|| {
        scanning
            .then(|| "The catalog is being re-scanned. Save is available when it settles.".into())
    })
}

#[cfg(test)]
mod tests {
    use super::save_note;

    #[test]
    fn an_actionable_blocker_outranks_the_re_scan() {
        let blocker = || Some("A table needs a name.".to_string());
        assert_eq!(save_note(blocker(), true), blocker());
        assert_eq!(save_note(blocker(), false), blocker());
    }

    #[test]
    fn a_re_scan_is_explained_once_it_is_the_only_thing_left() {
        // The regression this guards: Save was disabled while scanning and the footer said
        // nothing, because the two were computed separately.
        let note = save_note(None, true).expect("a scanning footer says why");
        assert!(note.contains("re-scanned"), "{note}");
    }

    #[test]
    fn nothing_to_say_when_save_is_available() {
        assert_eq!(save_note(None, false), None);
    }
}

/// The one blocker the draft cannot see: a name that belongs to something else.
///
/// Tables and views share one SQL namespace, so a new name has to be free in both — and on an
/// edit, the def's own name does not clash with itself.
fn name_clash(ctx: ConfigureCtx, project: RadioStation<ProjectState, ProjChan>) -> Option<String> {
    let draft = ctx.draft.read();
    let name = draft.name.trim();
    let target = ctx.target.read();
    if target
        .editing()
        .is_some_and(|own| ProjectState::same_name(own, name))
    {
        return None;
    }
    let kind = project.peek().name_in_use(name)?;
    Some(format!(
        "'{name}' is already the name of a {}.",
        match kind {
            strata_model::CatalogKind::Table => "table",
            strata_model::CatalogKind::View => "view",
            strata_model::CatalogKind::Query => "saved query",
        }
    ))
}

/// Write the def, persist it, and ask for the registration pass. See the module doc.
fn save(
    mut ctx: ConfigureCtx,
    mut project: RadioStation<ProjectState, ProjChan>,
    rescan: CatalogRescan,
    engine: EngineCtx,
    log: LogCtx,
) {
    let root = project.peek().root.clone();
    let def = ctx.draft.peek().def(&root);
    let renamed_from = ctx
        .target
        .peek()
        .editing()
        .filter(|old| !ProjectState::same_name(old, &def.name))
        .map(str::to_string);
    let name = def.name.clone();

    // The write and the persist are one step, and the persist is checked. `upsert_table` puts
    // the row back in `Reg::Loading`, which is already the state this window renders as busy.
    let landed = {
        let mut p = project.write_channel(ProjChan::Tables);
        if let Some(old) = &renamed_from {
            p.remove_table(old);
        }
        p.upsert_table(def);
        persisted(&p, log)
    };
    // The store write above has already happened, so the row exists either way and **must** be
    // registered either way: returning here would leave it in `Reg::Loading` with nothing left
    // to answer it — a permanent spinner in the catalog. So the pass is asked for below whatever
    // the persist said; what the failure changes is only what this window claims.
    //
    // `persisted` has already logged the cause, in the project window where the user will look
    // for it. Saying so here too would be the same failure twice; what this window owes them is
    // not to claim the save happened, and not to close as though it had.
    if !landed {
        ctx.status.set(Status::Failed(
            "The table is registered, but the project file could not be written, so it will \
             be gone when this project is reopened."
                .into(),
        ));
    } else {
        ctx.status.set(Status::Registering(name.clone()));
    }
    // **The window is now configuring what it just wrote.** Without this a second Save — after a
    // registration failure, say — measures `renamed_from` against the name the window *opened*
    // on, so the row the first Save created is never removed and the catalog keeps a phantom
    // table under the intermediate name.
    {
        let mut target = ctx.target;
        target.set(ConfigureTarget::Edit(name.clone()));
    }

    // A rename leaves the engine still holding the old name, which the scan pass cannot know
    // about — it registers the defs, and this one no longer has a def. Dropping it is the one
    // engine call this window makes. Views written against the old name break, which is the
    // user's edit: their rows fail their own re-create and say so.
    if let Some(old) = &renamed_from {
        engine.deregister(old);
        log_event(
            log,
            LogLevel::Info,
            format!("Renamed table '{old}' to '{name}'"),
        );
    }

    match renamed_from {
        // **A rename is a whole-catalog pass, not a one-table one.** `views_to_refresh` can only
        // find views whose deps name the table it is given, and a view that read the *old* name
        // names neither — so scoping to the new name leaves those views `Ready`, still answering
        // from the provider this rename just deregistered.
        Some(_) => refresh_catalog(rescan),
        None => refresh_table(rescan, name),
    }
}
