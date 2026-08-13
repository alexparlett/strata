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
use freya::radio::{use_radio, use_radio_station, Radio, RadioStation};
use strata_core::engine::{RunOutcome, RunTag, WsId};
use uuid::Uuid;

use crate::apps::configure::{ConfigureCtx, ConfigureTarget, Status};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::{
    log_event, settle, use_report, use_settle, LogLevel, ReportCtx, Settle,
};
use crate::apps::project::{
    persisted_defs, refresh_catalog, refresh_table, Catalog, CatalogRescan, ProjChan, ProjectState,
};
use crate::components::divider::Divider;
use crate::components::metrics::ACTION_HEIGHT;
use crate::components::metrics::{SP_4, SP_5};
use crate::components::typography::{Control, Path};
use crate::components::window::window_theme;

/// The strip's inset (canvas `padding: var(--sp-4) var(--sp-5)`).
const FOOTER_PADDING: Gaps = Gaps::new(SP_4, SP_5, SP_4, SP_5);

/// The page size a internal table's `CREATE TABLE` is dispatched with. It belongs to `Engine::run`'s
/// **query** arm; a create classifies as a statement, so the router never reads it.
const INTERNAL_PAGE_SIZE: usize = 1;

#[derive(PartialEq)]
pub struct Footer;

impl Component for Footer {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let form = crate::components::form::form_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let project = use_radio_station::<ProjectState, ProjChan>();
        // The one channel this footer has to *watch*: a connection forgotten in the pane next
        // door has to disable Save here, and nothing else in this window would notice.
        let connections = use_radio::<ProjectState, ProjChan>(ProjChan::Connections);
        let rescan = use_consume::<CatalogRescan>();
        let catalog = use_consume::<Catalog>();
        let engine = use_consume::<EngineCtx>();
        let report = use_report();
        // The project window's one statement fold, reachable here because this window was handed
        // every store it writes through (`ConfigureLaunch`). A internal table is **created** by a
        // statement, not registered from a def, so its Save folds the report exactly as a typed
        // `CREATE TABLE` in the editor does — never a second `apply`, persist path or epoch bump.
        let to = use_settle();
        let platform = use_hook(Platform::get);

        // Two busy states, and they differ in exactly one thing: whether the work is **this**
        // window's. A registration belongs to the project window's scan driver and lands on the
        // catalog row regardless, so Cancel stays live through it; a create is running here, and
        // the fold that makes it durable is on the other side of this window's own task
        // (`Status::Creating`).
        let status = ctx.status.read().clone();
        let registering = matches!(status, Status::Registering(_));
        let creating = status.holds_window();
        let busy = status.busy();
        // The project window's driver **drops** a request raised while a pass is already in
        // flight, and nothing retries it — so pressing Save then would leave the row `Loading`
        // for good. The sidebar's ↻ answers this by disabling itself for the duration; so does
        // this. Subscribes, so the button comes back by itself when the pass settles.
        let scanning = catalog.read().is_scanning();
        // What the *draft* can answer, the two things only the catalog can (a name another def
        // already owns, a connection this project no longer has), and last the one nobody can —
        // see [`save_note`].
        let note = save_note(
            ctx.draft
                .read()
                .blocker()
                .or_else(|| column_fault(ctx))
                .or_else(|| name_clash(ctx, project))
                .or_else(|| missing_connection(ctx, connections)),
            scanning,
        );

        let cancel = {
            Button::new()
                .height(Size::px(ACTION_HEIGHT))
                // Available through a *registration*: that work is the project window's, and it
                // answers on the catalog row whether this window is here to watch or not. Not
                // through a **create**, which is this window's own — closing now drops the task
                // before the fold, and `ddl::tables::create` publishes its spool by rename
                // before its last await, so the data would be left with no def pointing at it.
                // Esc is gated on the same state (`apps::configure`'s `Command::Cancel`).
                .enabled(!creating)
                .on_press(move |_: Event<PressEventData>| platform.close_current_window())
                .child(Control::new("Cancel"))
        };

        let save = Button::new()
            .filled()
            .height(Size::px(ACTION_HEIGHT))
            .enabled(!busy && note.is_none())
            .on_press({
                move |_: Event<PressEventData>| {
                    save(ctx, project, rescan, engine.clone(), report, to);
                }
            })
            .child(Control::new(match (creating, registering) {
                (true, _) => "Creating…",
                (_, true) => "Validating…",
                _ => "Save",
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
                    .spacing(SP_4)
                    .padding(FOOTER_PADDING)
                    .background(win.background)
                    // Why the button is off, rather than an unexplained dead control. A
                    // registration failure is *not* shown here — it is a paragraph the engine
                    // wrote, and it has its own block at the end of the body.
                    .child(
                        rect().width(Size::flex(1.)).maybe_child(
                            note.filter(|_| !busy).map(|why| {
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

/// The blocker a **internal** table's columns carry (IT-01) — the first faulty row's message in
/// full, and last the wait for a verdict that has not landed.
///
/// The draft cannot answer this on its own: what a type *means* is the planner's, cached in
/// `probes`, so the row-level rule lives on the draft and the window supplies the answers.
///
/// The wait comes last because it is the only one of these that clears itself; everything above
/// it is something the user has to do.
fn column_fault(ctx: ConfigureCtx) -> Option<String> {
    let draft = ctx.draft.read();
    if !draft.internal() {
        return None;
    }
    let probes = ctx.probes.read();
    if let Some((_, message)) = draft.column_faults(&probes).into_iter().next() {
        return Some(message);
    }
    (!draft.unprobed(&probes).is_empty()).then(|| "Checking column types.".into())
}

/// The one blocker the draft cannot see: a name that belongs to something else.
///
/// Tables and views share one SQL namespace, so a new name has to be free in both — and on an
/// edit, the def's own name does not clash with itself. The **sentence** is the store's
/// ([`ProjectState::name_taken`]), shared with the empty-table panel, which asks the catalog the
/// same question.
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
    project.peek().name_taken(name)
}

/// The other blocker the draft cannot see: the connection it reads through is **gone** (W7 · 04).
///
/// A def keeps naming its bucket after that connection is forgotten — the def is the table's, and
/// nothing rewrites it behind the user's back — so this window can open on one. It says so and
/// blocks Save rather than letting the reference be re-saved: the picker offers only connections
/// the project has, so the fix is to choose one, and that is exactly the treatment a format with
/// no reader gets (`ConfigureDraft::blocker`).
///
/// Only while LOCATION is on Remote: a connection kept across a flip back to Local
/// is a remembered choice, not the table's location.
///
/// `connections` is a **subscribed** handle, unlike the station [`name_clash`] peeks: the
/// catalog cannot lose a name under this window (only this window writes one), but the
/// Connections pane next door can forget a bucket while the form sits untouched — and a Save
/// that stayed enabled would then write a def naming a connection that is gone.
fn missing_connection(
    ctx: ConfigureCtx,
    connections: Radio<ProjectState, ProjChan>,
) -> Option<String> {
    let url = ctx.draft.read().store()?.to_string();
    let known = connections
        .read()
        .connections
        .iter()
        .any(|c| c.def.url() == url);
    (!known).then(|| {
        format!("'{url}' is not a connection in this project. Choose one, or add it back.")
    })
}

/// Write the def, persist it, and ask for the registration pass — or, on a **internal** table, run
/// the statement that creates it. See the module doc.
fn save(
    mut ctx: ConfigureCtx,
    mut project: RadioStation<ProjectState, ProjChan>,
    rescan: CatalogRescan,
    engine: EngineCtx,
    report: ReportCtx,
    to: Settle,
) {
    if ctx.draft.peek().internal() {
        create_internal_table(ctx, engine, to);
        return;
    }
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
        persisted_defs(&p, report)
    };
    // The store write above has already happened, so the row exists either way and **must** be
    // registered either way: returning here would leave it in `Reg::Loading` with nothing left
    // to answer it — a permanent spinner in the catalog. So the pass is asked for below whatever
    // the persist said; what the failure changes is only what this window claims.
    //
    // `persisted_defs` has already logged the cause, in the project window where the user will look
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
            report.log,
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
/// **Create the internal table** (IT-01): compose the statement the COLUMNS list describes, run it
/// through the router every typed statement goes through, and fold the report.
///
/// One visible statement on a **minted** `WsId` — a tab's would abort whatever that tab is
/// running — and its report handed to the window's one fold, which is what puts the row in the
/// store, the def in `project.json`, the epoch bump behind every tab's diagnostics and the entry
/// in the log. Registering a def here instead would be a second way to make a table, and the
/// spool that gives it its data has no def to be written from.
///
/// **The window is held open until the fold lands** (`Status::Creating`), and that is the whole
/// reason this state exists beside `Registering`. The task is spawned here, and everything that
/// makes the create durable — the def, the catalog row, the epoch, the log — runs *after* its
/// await. `ddl::tables::create` publishes its spool by **rename** before its own last await
/// (`register_external`), so a window closed mid-create drops this task at a point where the data
/// directory is already under its real name: no def would ever point at it, and `tidy_strata_dir`
/// sweeps only `.tmp-…`, so it would be permanent litter. Cancel and Esc are both refused while
/// this state holds; the moment the fold has landed it becomes `Registering`, and
/// `use_watch_registration` closes the window on the row the fold just made `Ready`.
///
/// The engine's own abort is not the gap: dropping `Engine::run`'s future runs `DispatchGuard`'s
/// drop, which aborts the detached task. But an abort is delivered at the next **await**, and
/// `create` has none left after `register_external` — so a create interrupted late finishes on
/// the engine's runtime with nobody left to receive its report. Holding the window is what
/// removes the window in which that can happen.
fn create_internal_table(mut ctx: ConfigureCtx, engine: EngineCtx, to: Settle) {
    let Some(sql) = ctx.draft.peek().create_statement() else {
        return;
    };
    let name = ctx.draft.peek().name.trim().to_string();
    ctx.status.set(Status::Creating(name.clone()));
    spawn(async move {
        let ws = WsId(Uuid::new_v4().as_u128());
        let tag = RunTag(Uuid::new_v4().as_u128());
        match engine.run(ws, tag, sql, INTERNAL_PAGE_SIZE).await {
            // **The fold's answer decides whether this window may close.** The table is created
            // and registered either way — the statement already ran — but a def that never
            // reached `project.json` is one the next open loses, and closing on `Ready` would
            // show the user a clean success for it. Same sentence and same refusal to close as
            // the external path above; `persisted_defs` has already logged the cause in the
            // project window, so this window only owes them not to claim the save happened.
            Ok(RunOutcome::Statement(report)) => {
                if settle(to, &engine, &report) {
                    // The fold has landed, so the work is no longer this window's to protect —
                    // and the row it named is already `Ready`, which is what closes the window.
                    ctx.status.set(Status::Registering(name));
                } else {
                    ctx.status.set(Status::Failed(
                        "The table was created, but the project file could not be written, so \
                         it will be gone when this project is reopened."
                            .into(),
                    ));
                }
            }
            // Unreachable while the router classifies `CREATE TABLE` as a statement, and said
            // rather than swallowed — but the minted workspace is retired either way, since no
            // tab owns it and nothing else would ever release a snapshot a query arm took.
            Ok(RunOutcome::Rows(..)) => {
                engine.cleanup_ws(ws);
                ctx.status
                    .set(Status::Failed("The statement ran as a query.".into()));
            }
            Err(why) => ctx.status.set(Status::Failed(why)),
        }
    });
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
