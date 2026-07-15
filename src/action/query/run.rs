//! Query execution: run / EXPLAIN / cancel. Split out of the action::query module.

use dioxus::prelude::*;

use crate::ddl::{self, Decision};
use crate::engine::Command;
use crate::state::AppState;

/// Run the active tab's SQL (DDL-classified: run / capture-view / drop-view / block).
pub fn run(mut state: Signal<AppState>) {
    let sql = crate::session::active_sql();
    let trimmed = sql.trim().to_string();
    if trimmed.is_empty() {
        return;
    }
    // `EXPLAIN [ANALYZE]` takes a dedicated path: the engine runs it and returns
    // a parsed plan tree (S12) rather than a paged result snapshot.
    if crate::plan::is_explain(&trimmed) {
        explain(state, trimmed);
        return;
    }
    match ddl::classify(&trimmed) {
        Decision::Block { reason } => {
            let id = crate::session::active_id();
            if id != 0 {
                crate::runs::edit_existing(id, |run| {
                    run.running = false;
                    run.result = None;
                });
            }
            tracing::warn!("blocked statement: {reason}");
        }
        Decision::CaptureView { name, sql } => {
            let tx = state.read().cmd_tx.clone();
            if let Some(tx) = tx {
                let _ = tx.send(Command::CreateView { name, sql });
            }
        }
        Decision::DropView { name } => {
            let tx = state.read().cmd_tx.clone();
            if let Some(tx) = tx {
                let _ = tx.send(Command::DropView { name });
            }
        }
        Decision::Query => {
            let req = {
                let mut s = state.write();
                let r = s.next_req;
                s.next_req += 1;
                r
            };
            let ws_id = crate::session::active_id();
            let page_size = crate::runs::RUNS
                .resolve()
                .get(ws_id)
                .map(|e| e.peek().page_size)
                .unwrap_or(100);
            crate::runs::edit(ws_id, |run| {
                run.running = true;
                run.query_error = None;
                run.plan = None;
                run.pending_req = Some(req);
                run.page = 1;
            });
            let tx = state.read().cmd_tx.clone();
            if let Some(tx) = tx {
                let _ = tx.send(Command::Query {
                    req_id: req,
                    ws_id,
                    sql: trimmed,
                    page_size,
                });
            }
        }
    }
}

/// Send an already-built `EXPLAIN …` statement to the engine's explain path for the
/// active tab — the shared core of `run`'s EXPLAIN branch and `run_explain` (E4).
fn explain(mut state: Signal<AppState>, sql: String) {
    let req = {
        let mut s = state.write();
        let r = s.next_req;
        s.next_req += 1;
        r
    };
    let ws_id = crate::session::active_id();
    crate::runs::edit(ws_id, |run| {
        run.running = true;
        run.query_error = None;
        run.plan = None;
        run.pending_req = Some(req);
        run.page = 1;
    });
    let tx = state.read().cmd_tx.clone();
    if let Some(tx) = tx {
        let _ = tx.send(Command::Explain {
            req_id: req,
            ws_id,
            sql,
        });
    }
}

/// Run an `EXPLAIN [ANALYZE]` of the active tab's SQL **without mutating the editor
/// buffer** (E4): wrap the current SQL with the prefix (stripping any existing one) and
/// route it through the engine's explain path. Like Save-as-view, the change lives in
/// the engine, not the editor — the user's query in the editor stays untouched.
pub fn run_explain(state: Signal<AppState>, analyze: bool) {
    let sql = crate::session::active_sql();
    if sql.trim().is_empty() {
        return;
    }
    explain(state, crate::plan::as_explain(&sql, analyze));
}

/// Cancel the active tab's in-flight query / explain (S14). No-op if nothing is
/// running. Scoped to the current `pending_req`, so a stale Esc/click can't abort a
/// just-started newer run.
pub fn cancel(state: Signal<AppState>) {
    let ws_id = crate::session::active_id();
    let req = crate::runs::RUNS.resolve().get(ws_id).and_then(|e| {
        let r = e.peek();
        r.running.then_some(r.pending_req).flatten()
    });
    let Some(req_id) = req else {
        return;
    };
    if let Some(tx) = state.read().cmd_tx.clone() {
        let _ = tx.send(Command::Cancel { ws_id, req_id });
    }
}
