//! Editor-toolbar actions (P2-16): buffer ops (Format / Clear) and the save dispatch.
//! Free functions over the window's stores + engine, so the toolbar buttons and the
//! future keymap's ⌘S share one implementation (the binding itself is a later slice).
//!
//! Save writes the *project*, not the tab (state-arch §4): it dispatches on the tab's
//! [`Origin`] — a view-bound tab re-issues `CREATE OR REPLACE VIEW` on *its* view (the
//! DEV_TASKS "⌘S on a view saves a saved-query" bug), a saved-query-bound tab
//! overwrites its query by id, and a scratch tab Save-As-es into a new saved query
//! under the tab's name. Save-as-view (the Eye button) is the explicit view path,
//! minting the first free `saved_view_N` name. The buffer is never DDL-classified —
//! DDL is blocked at Run, and Save saves the text as-is.

use freya::prelude::spawn;
use freya::radio::{Radio, RadioStation};
use strata_model::{SavedQuery, ViewDef};
use uuid::Uuid;

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{QueryMode, QuerySpec, RunId, DEFAULT_PAGE_SIZE};
use crate::apps::project::state::{
    Chan, Origin, ProjChan, ProjectState, SessionState, TabId,
};

/// A Run / Explain / Analyze press (P2-15 + ⌘↵): snapshot the tab's editor text *now*,
/// mint a fresh nonce, and set it as the tab's run request — on `Chan::Request(id)`, so
/// only the tab's results pane and toolbar wake. A blank buffer never runs, and neither
/// does one with current validation errors (P2-18) — both back up the toolbar's visual
/// gate, and this shared funnel covers ⌘↵ and the Explain buttons too.
pub fn press_query(mut session: Radio<SessionState, Chan>, id: TabId, mode: QueryMode) {
    let sql = session.read().tabs.get(&id).map(|t| t.text()).unwrap_or_default();
    if sql.trim().is_empty() || session.read().blocking_errors(id) {
        return;
    }
    session.write_channel(Chan::Request(id)).set_request(id, QuerySpec {
        tab: id,
        run: RunId::new(),
        sql,
        mode,
        page_size: DEFAULT_PAGE_SIZE,
    });
}

/// Cancel the in-flight request (the toolbar's Run→Cancel flip, the Running body's
/// control, and Esc all land here): tag-guarded engine-side abort (S14 — a stale press
/// can't kill a newer run) + drop *this tab's* trigger, unmounting its results body
/// back to Empty. Other tabs' requests are untouchable from here by construction.
pub fn cancel_run(
    engine: &EngineCtx,
    mut session: Radio<SessionState, Chan>,
    id: TabId,
    run: RunId,
) {
    engine.cancel(id.into(), run.into());
    session.write_channel(Chan::Request(id)).clear_request(id);
}

/// Pretty-print the tab's SQL in place. History-tracked — undo restores the
/// unformatted text.
pub fn format(mut session: Radio<SessionState, Chan>, id: TabId) {
    let Some(sql) = session.read().tabs.get(&id).map(|t| t.text()) else {
        return;
    };
    // Uppercased keywords; indent matched to the editor's own Tab width (4), so a
    // Format pass and hand-typed indentation agree. (A formatting setting later can
    // drive both from one place.)
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
pub fn save(
    session: Radio<SessionState, Chan>,
    project: RadioStation<ProjectState, ProjChan>,
    engine: EngineCtx,
    id: TabId,
) {
    let Some((sql, name, origin)) = read_tab(session, id) else {
        return;
    };
    match origin {
        Origin::View(view) => save_view(session, project, engine, id, view, sql, false),
        Origin::SavedQuery(qid) => save_query(session, project, id, qid, name, sql),
        Origin::Scratch => save_query(session, project, id, Uuid::new_v4(), name, sql),
    }
}

/// The Eye button: save the buffer as a **new** catalog view, auto-named with the
/// first free `saved_view_N` (tables + views share one SQL namespace) — and rename
/// the tab to it, since the view's name is its identity.
pub fn save_as_view(
    session: Radio<SessionState, Chan>,
    project: RadioStation<ProjectState, ProjChan>,
    engine: EngineCtx,
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
    save_view(session, project, engine, id, name, sql, true);
}

/// The tab's savable state: `(sql, trimmed name, origin)`; `None` when the tab is
/// gone or the buffer is blank (nothing to save).
fn read_tab(
    session: Radio<SessionState, Chan>,
    id: TabId,
) -> Option<(String, String, Origin)> {
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
fn save_view(
    mut session: Radio<SessionState, Chan>,
    mut project: RadioStation<ProjectState, ProjChan>,
    engine: EngineCtx,
    id: TabId,
    name: String,
    sql: String,
    rename: bool,
) {
    {
        let mut p = project.write_channel(ProjChan::Views);
        p.upsert_view(ViewDef { name: name.clone(), sql: sql.clone() });
        if let Err(e) = p.save_defs() {
            tracing::error!("save project defs: {e}");
        }
    }
    session.write_channel(Chan::Tabs).bind_saved(
        id,
        rename.then(|| name.clone()),
        Origin::View(name.clone()),
    );
    spawn(async move {
        match engine.create_view(name.clone(), sql).await {
            Ok(meta) => project.write_channel(ProjChan::Views).view_registered(&name, meta),
            Err(e) => {
                tracing::error!("create view '{name}' failed: {e}");
                project.write_channel(ProjChan::Views).view_failed(&name, e);
            }
        }
    });
}

/// Upsert the saved query `qid` with the tab's name + `sql` (keeping the meta chip of
/// the query being overwritten — a fresh one has no run yet), persist, and bind the tab.
fn save_query(
    mut session: Radio<SessionState, Chan>,
    mut project: RadioStation<ProjectState, ProjChan>,
    id: TabId,
    qid: Uuid,
    name: String,
    sql: String,
) {
    if name.is_empty() {
        return;
    }
    {
        let mut p = project.write_channel(ProjChan::Queries);
        let meta = p
            .saved_queries
            .iter()
            .find(|q| q.id == qid)
            .map(|q| q.meta.clone())
            .unwrap_or_else(|| "—".into());
        p.upsert_saved_query(SavedQuery { id: qid, name, sql, meta });
        if let Err(e) = p.save_defs() {
            tracing::error!("save project defs: {e}");
        }
    }
    session
        .write_channel(Chan::Tabs)
        .bind_saved(id, None, Origin::SavedQuery(qid));
}
