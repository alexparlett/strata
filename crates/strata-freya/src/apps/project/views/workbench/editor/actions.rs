//! Editor-toolbar actions (P2-16): buffer ops (Format / Clear) and the save dispatch.
//! Free functions over the window's stores + engine, so the toolbar buttons and the
//! future keymap's ⌘S share one implementation (the binding itself is a later slice).
//!
//! Save writes the *project*, not the tab (state-arch §4): it dispatches on the tab's
//! [`Origin`] — a view-bound tab re-issues `CREATE OR REPLACE VIEW` on *its* view (the
//! `DEV_TASKS` "⌘S on a view saves a saved-query" bug), a saved-query-bound tab
//! overwrites its query by id, and a scratch tab Save-As-es into a new saved query
//! under the tab's name. Save-as-view (the Eye button) is the explicit view path,
//! minting the first free `saved_view_N` name. The buffer is never classified *here* —
//! Run's statement router is the engine's (ED-02), and Save saves the text as-is.

use freya::prelude::spawn;
use freya::radio::{Radio, RadioStation};
use strata_core::util::fmt_int;
use strata_model::{Origin, SavedQuery, TabId, ViewDef};
use uuid::Uuid;

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{QueryMode, QuerySpec, RunId, DEFAULT_PAGE_SIZE};
use crate::apps::project::state::{
    catalog_settled, log_event, persisted_defs, Catalog, Chan, LogCtx, LogLevel, ProjChan,
    ProjectState, QueryTab, ReportCtx, SessionState,
};

/// A Run / Explain / Analyze press (P2-15 + ⌘↵): snapshot the tab's editor text *now*,
/// mint a fresh nonce, and set it as the tab's run request — on `Chan::Request(id)`, so
/// only the tab's results pane and toolbar wake. A blank buffer never runs — backing up
/// the toolbar's visual gate; this shared funnel covers ⌘↵ and the Explain buttons too.
/// Validation errors never block a run (P2-23): diagnostics advise, the engine decides
/// — a doomed statement fails at plan time with the same error in the results pane, and a
/// statement the policy refuses is refused by the engine's own router with the same words the
/// squiggle showed (ED-02).
pub fn press_query(mut session: Radio<SessionState, Chan>, id: TabId, mode: QueryMode) {
    let sql = session
        .read()
        .tabs
        .get(&id)
        .map(QueryTab::text)
        .unwrap_or_default();
    if sql.trim().is_empty() {
        return;
    }
    session.write_channel(Chan::Request(id)).set_request(
        id,
        QuerySpec {
            tab: id,
            run: RunId::new(),
            sql,
            mode,
            page_size: DEFAULT_PAGE_SIZE,
        },
    );
}

/// [`press_query`] with the "already running" gate in front of it — the **Run** command, wherever
/// it is asked for: ⌘↵ over the workbench, and the command palette's Run query row.
///
/// The gate exists because a press that is already executing must not double-run; it lives here
/// rather than beside a caller because the two callers cannot see the same things. The workbench
/// had it inline against its own `running` mirror, which is the *active tab's* request as the
/// results pane knows it — a value the palette (mounted at the window root, addressing the store)
/// has no access to at all. So the question is put to the engine, which is the one thing that
/// knows what is executing across every tab.
///
/// One window it does not cover, deliberately: between setting the request here and the engine
/// dispatching it, `is_running` is still false. A second press landing in that gap supersedes the
/// first, which the engine already settles as `superseded by a newer run` — a stop, not a fault
///, so the outcome is the run the user last asked for and nothing to report.
///
/// Not the Run *button*: it wears Cancel while a run is in flight, so there is nothing there to
/// gate, and it presses [`press_query`] directly.
pub fn run_query(engine: &EngineCtx, session: Radio<SessionState, Chan>, id: TabId) {
    if engine.is_running(id.into()) {
        return;
    }
    press_query(session, id, QueryMode::Run);
}

/// Load `sql` into the tab's buffer, replacing what it held — the History drawer's click
/// (P3-14), and the first half of its double-click, which then calls [`press_query`].
///
/// A plain `set_text`, so it is **undoable**: the buffer's own history is what makes replacing a
/// tab's text a safe click rather than something that needs a confirm. Nothing else changes —
/// the tab keeps its name, its origin and its save target, because a past run is a string, not
/// an artifact to bind to.
pub fn load_sql(mut session: Radio<SessionState, Chan>, id: TabId, sql: &str) {
    let changed = session
        .read()
        .tabs
        .get(&id)
        .is_some_and(|t| t.text() != sql);
    if !changed {
        return;
    }
    if let Some(t) = session.write_channel(Chan::Tab(id)).tabs.get_mut(&id) {
        t.editor.set_text(sql);
    }
}

/// Open a **new** tab holding `sql`, focused, and hand back its id — the chat pane's promotion
/// (AS-04).
///
/// A new tab rather than the active one is the whole point: the History drawer loads into the
/// tab you are in because you asked for that by being there, but an offer is the assistant's
/// suggestion arriving in a surface you were only reading, and overwriting your buffer with it
/// would destroy the record of how a number was reached.
///
/// Composed from the two funnels that already exist ([`SessionState::open_blank`] then
/// [`load_sql`]) rather than a store method of its own, so a promoted query is an ordinary
/// scratch tab in every respect — named `query N`, undoable, saveable, and bound to no artifact.
pub fn open_sql(mut session: Radio<SessionState, Chan>, sql: &str) -> TabId {
    let id = session.write_channel(Chan::Tabs).open_blank();
    load_sql(session, id, sql);
    id
}

/// Cancel the in-flight request — the toolbar's Run→Cancel flip, the Running body's control, and
/// the Esc that body binds to the same handler (`results::running`'s `on_esc`) all land here:
/// tag-guarded engine-side abort (S14 — a stale press can't kill a newer run) + drop *this tab's*
/// trigger, unmounting its results body back to Empty. Other tabs' requests are untouchable from
/// here by construction.
///
/// **And the cancel is where a cancel gets logged** (P3-13), not the settle: dropping the
/// trigger unmounts the press's keeper in the same pass, so the entry's `Err("cancelled")`
/// lands with nobody subscribed (the keeper's own doc says as much). `Engine::cancel` answers
/// with the elapsed time *iff* it really aborted something, which is both the guard against
/// logging a cancel that hit nothing and the one real fact the event can carry.
pub fn cancel_run(
    engine: &EngineCtx,
    mut session: Radio<SessionState, Chan>,
    log: LogCtx,
    id: TabId,
    run: RunId,
) {
    if let Some(elapsed) = engine.cancel(id.into(), run.into()) {
        log_event(
            log,
            LogLevel::Warning,
            format!("Query cancelled after {} ms", fmt_int(elapsed as u64)),
        );
    }
    session.write_channel(Chan::Request(id)).clear_request(id);
}

/// Pretty-print the tab's SQL in place. History-tracked — undo restores the
/// unformatted text.
pub fn format(mut session: Radio<SessionState, Chan>, id: TabId) {
    let Some(sql) = session.read().tabs.get(&id).map(QueryTab::text) else {
        return;
    };
    let formatted = sqlformat::format(
        &sql,
        &sqlformat::QueryParams::None,
        &sqlformat::FormatOptions {
            indent: sqlformat::Indent::Spaces(4),
            uppercase: Some(true),
            ..Default::default()
        },
    );
    if formatted != sql {
        if let Some(t) = session.write_channel(Chan::Tab(id)).tabs.get_mut(&id) {
            t.editor.set_text(&formatted);
        }
    }
}

/// Clear the tab's buffer. History-tracked — undo restores it.
pub fn clear(mut session: Radio<SessionState, Chan>, id: TabId) {
    if let Some(t) = session.write_channel(Chan::Tab(id)).tabs.get_mut(&id) {
        if t.editor.rope.len_chars() > 0 {
            t.editor.set_text("");
        }
    }
}

/// The Save button: write the buffer to the tab's save target, dispatching on its
/// origin (see the module doc). A blank buffer never saves.
///
/// `log` is the window's event log: a save is a project mutation, so its outcome is recorded
/// there (P3-13).
pub fn save(
    session: Radio<SessionState, Chan>,
    project: RadioStation<ProjectState, ProjChan>,
    engine: EngineCtx,
    catalog: Catalog,
    report: ReportCtx,
    id: TabId,
) {
    let Some((sql, name, origin)) = read_tab(session, id) else {
        return;
    };
    match origin {
        Origin::View(view) => save_view(
            session, project, engine, catalog, report, id, view, sql, false,
        ),
        Origin::SavedQuery(qid) => save_query(session, project, report, id, qid, name, sql),
        Origin::Scratch => save_query(session, project, report, id, Uuid::new_v4(), name, sql),
    }
}

/// The Eye button: save the buffer as a **new** catalog view, auto-named with the
/// first free `saved_view_N` (tables + views share one SQL namespace) — and rename
/// the tab to it, since the view's name is its identity.
pub fn save_as_view(
    session: Radio<SessionState, Chan>,
    project: RadioStation<ProjectState, ProjChan>,
    engine: EngineCtx,
    catalog: Catalog,
    report: ReportCtx,
    id: TabId,
) {
    let Some((sql, _, _)) = read_tab(session, id) else {
        return;
    };
    let name = {
        let p = project.peek();
        (1..)
            .map(|i| format!("saved_view_{i}"))
            .find(|n| p.name_in_use(n).is_none())
            .unwrap()
    };
    save_view(
        session, project, engine, catalog, report, id, name, sql, true,
    );
}

/// The tab's savable state: `(sql, trimmed name, origin)`; `None` when the tab is
/// gone or the buffer is blank (nothing to save).
fn read_tab(session: Radio<SessionState, Chan>, id: TabId) -> Option<(String, String, Origin)> {
    let s = session.read();
    let t = s.tabs.get(&id)?;
    let sql = t.text();
    if sql.trim().is_empty() {
        return None;
    }
    Some((sql, t.name.trim().to_string(), t.origin.clone()))
}

/// Write `sql` as the view `name`: def first (row → `Loading`, persisted at the
/// mutation point), bind the tab, then `CREATE OR REPLACE VIEW` on the engine with
/// the answer landing on the row exactly like load-time registration (Ready with
/// columns/deps, or Failed with the planner's error).
#[allow(clippy::too_many_arguments)]
fn save_view(
    mut session: Radio<SessionState, Chan>,
    mut project: RadioStation<ProjectState, ProjChan>,
    engine: EngineCtx,
    catalog: Catalog,
    report: ReportCtx,
    id: TabId,
    name: String,
    sql: String,
    rename: bool,
) {
    let persisted = {
        let mut p = project.write_channel(ProjChan::Views);
        p.upsert_view(ViewDef {
            name: name.clone(),
            sql: sql.clone(),
        });
        persisted_defs(&p, report)
    };
    session.write_channel(Chan::Tabs).bind_saved(
        id,
        rename.then(|| name.clone()),
        Origin::View(name.clone()),
    );
    spawn(async move {
        match engine.create_view(name.clone(), sql).await {
            Ok(meta) => {
                if persisted {
                    log_event(report.log, LogLevel::Ok, format!("Saved view '{name}'"));
                }
                project
                    .write_channel(ProjChan::Views)
                    .view_registered(&name, meta);
            }
            Err(e) => {
                tracing::error!("create view '{name}' failed: {e}");
                log_event(
                    report.log,
                    LogLevel::Error,
                    format!("View '{name}' failed to register: {e}"),
                );
                project.write_channel(ProjChan::Views).view_failed(&name, e);
            }
        }
        catalog_settled(catalog);
    });
}

/// Upsert the saved query `qid` with the tab's name + `sql` (keeping the meta chip of
/// the query being overwritten — a fresh one has no run yet), persist, and bind the tab.
fn save_query(
    mut session: Radio<SessionState, Chan>,
    mut project: RadioStation<ProjectState, ProjChan>,
    report: ReportCtx,
    id: TabId,
    qid: Uuid,
    name: String,
    sql: String,
) {
    if name.is_empty() {
        return;
    }
    let saved = {
        let mut p = project.write_channel(ProjChan::Queries);
        let meta = p
            .saved_queries
            .iter()
            .find(|q| q.id == qid)
            .map(|q| q.meta.clone())
            .unwrap_or_else(|| "—".into());
        p.upsert_saved_query(SavedQuery {
            id: qid,
            name: name.clone(),
            sql,
            meta,
        });
        persisted_defs(&p, report)
    };
    if saved {
        log_event(report.log, LogLevel::Ok, format!("Saved query '{name}'"));
    }
    session
        .write_channel(Chan::Tabs)
        .bind_saved(id, None, Origin::SavedQuery(qid));
}
