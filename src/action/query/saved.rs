//! Saved queries + save-as-view + load-select-*. Split out of the action::query module.

use dioxus::prelude::*;

use crate::engine::Command;
use crate::state::{AppState, SavedQuery};

/// Save the active SELECT as a named catalog view (auto-named `saved_view_N`).
pub fn save_as_view(state: Signal<AppState>) {
    let sql = crate::session::active_sql();
    let n = state.read().project.views.len() + 1;
    let name = format!("saved_view_{n}");
    // The tab is now bound to (and in sync with) this view.
    crate::session::set_origin(
        crate::session::active_id(),
        crate::state::Origin::View(name.clone()),
    );
    let tx = state.read().cmd_tx.clone();
    if let Some(tx) = tx {
        let _ = tx.send(Command::CreateView { name, sql });
    }
}

/// Load `SELECT * FROM t LIMIT <row_limit>` into the active tab (does not run).
/// The LIMIT comes from the "Default row limit" setting (0 = no limit).
pub fn select_star(_state: Signal<AppState>, table: &str) {
    let limit = crate::settings::row_limit();
    let sql = if limit > 0 {
        format!("SELECT *\nFROM {table}\nLIMIT {limit};")
    } else {
        format!("SELECT *\nFROM {table};")
    };
    crate::session::open_named(table, sql, crate::state::Origin::Scratch);
}

/// Save the active tab's SQL to the project under the tab's name (upsert by name,
/// case-insensitive). Bound to ⌘S.
pub fn save(mut state: Signal<AppState>) {
    let Some(w) = crate::session::active() else {
        return;
    };
    let name = w.name.trim().to_string();
    if name.is_empty() {
        return;
    }
    let sql = w.sql.clone();
    let meta = crate::runs::RUNS
        .resolve()
        .get(w.id)
        .and_then(|e| {
            e.peek()
             .result
             .as_ref()
             .map(|r| format!("{} rows", r.total))
        })
        .unwrap_or_else(|| "—".to_string());
    let mut s = state.write();
    let updated = if let Some(q) = s
        .project
        .saved_queries
        .iter_mut()
        .find(|q| q.name.eq_ignore_ascii_case(&name))
    {
        q.sql = sql;
        q.meta = meta;
        true
    } else {
        s.project.saved_queries.push(SavedQuery {
            name: name.clone(),
            sql,
            meta,
        });
        false
    };
    let verb = if updated { "Updated" } else { "Saved" };
    crate::event_ok!("{verb} query '{name}' to project");
    drop(s);
    // The tab is now bound to (and in sync with) this saved query.
    crate::session::set_origin(w.id, crate::state::Origin::SavedQuery(name.clone()));
}

/// Open a saved query: reuse a tab already named after it, else open a new tab.
pub fn open_saved(state: Signal<AppState>, name: &str) {
    let sql = state
        .read()
        .project
        .saved_queries
        .iter()
        .find(|q| q.name == name)
        .map(|q| q.sql.clone());
    let Some(sql) = sql else {
        return;
    };
    crate::session::open_named(
        name,
        sql,
        crate::state::Origin::SavedQuery(name.to_string()),
    );
}

/// Delete a saved query from the project (immediate — no confirm dialog).
pub fn delete_saved(mut state: Signal<AppState>, name: &str) {
    let mut s = state.write();
    s.project.saved_queries.retain(|q| q.name != name);
    crate::event_info!("Deleted query '{name}'");
}
