//! Root component: owns the `Signal<AppState>`, bridges the DataFusion engine
//! (spawn + event-draining coroutine), routes keyboard shortcuts, and lays out
//! the shell + modals.
//!
//! UI intents are funneled through [`crate::action::dispatch`]; this file only
//! holds the component, the engine wiring, and the engine→state reducer
//! ([`apply_event`]).

use dioxus::desktop::use_wry_event_handler;
use dioxus::prelude::*;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::action::{dispatch, Action};
use crate::engine::{self, Command, Event};
use crate::action::panel::resize_handle;
use crate::query_error::QueryError;
use crate::state::{
    AppState, CatalogTable, CatalogView, CfgStatus, HistoryItem, LogKind, RegStatus, ResizeTarget,
};
use crate::ui;

/// Root class. On macOS the transparent title bar means the traffic-light
/// buttons overlay the top-left, so the header gets extra left padding there.
#[cfg(target_os = "macos")]
const ROOT_CLASS: &str = "ps-app mac";
#[cfg(not(target_os = "macos"))]
const ROOT_CLASS: &str = "ps-app";

/// One project window: owns its `AppState` + engine. `open_path` is the
/// `.psproj` to load on startup (empty string → a fresh untitled project).
/// Spawned via `crate::window::spawn_project_window`; the first window is created
/// by `main` through `root_entry`.
#[component]
pub fn ProjectRoot(open_path: String) -> Element {
    let mut state = use_signal(AppState::empty);
    use_context_provider(|| state);

    // The command palette's open state is local (egui-style): the ⌘K handler and
    // the header search button both drive this signal, and it closes via its
    // Dialog — no `AppState` flag, no `CloseOverlays`.
    let mut cmdk = use_signal(|| false);

    // Track this window so siblings can be focused / cycled, and so "Close
    // project" knows whether it's the last one.
    let win_id = use_hook(crate::window::register_current_window);

    // Spawn the engine, drain its events, and load the assigned project.
    use_hook(move || {
        let engine::Handle { cmd_tx, evt_rx } = engine::spawn();
        state.write().cmd_tx = Some(cmd_tx);
        spawn(drain_events(state, evt_rx));
        let cfg = crate::config::load();
        let os_dark = crate::theme::os_is_dark();
        {
            let mut s = state.write();
            s.theme_id = cfg.theme;
            s.sync_os = cfg.sync_os;
            s.os_dark = os_dark;
            s.density_compact = cfg.density_compact;
            s.zebra = cfg.zebra;
            s.row_limit = cfg.row_limit;
            s.reopen_on_startup = cfg.reopen_on_startup;
            s.default_project_dir = cfg.default_project_dir;
            s.open_pref = cfg.open_pref;
            s.confirm_close_running = cfg.confirm_close_running;
            s.recent_projects = cfg.recent_projects;
        }
        if !open_path.is_empty() {
            crate::action::projects::load_current(state, std::path::PathBuf::from(open_path));
        }
    });

    // Persist window geometry + save on an OS close-button (the window is still
    // alive here, unlike `use_drop`). Does *not* open the launcher — an OS close
    // never does; that's reserved for the explicit "Close project" action.
    use_wry_event_handler(move |event, _| {
        use dioxus::desktop::tao::event::{Event as TaoEvent, WindowEvent};
        if let TaoEvent::WindowEvent {
            window_id,
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            if *window_id == win_id {
                crate::action::projects::save_on_close(state, win_id);
            }
        }
    });

    // macOS: dark NSWindow background so a resize doesn't flash white.
    #[cfg(target_os = "macos")]
    use_hook(|| crate::window::paint_ns_background(0.043, 0.055, 0.075));

    // Drop snapshots + de-register this window on close.
    use_drop(move || {
        if let Some(tx) = state.read().cmd_tx.clone() {
            let _ = tx.send(Command::CleanupAll);
        }
        crate::window::unregister_window(win_id);
    });

    // Suffix the root class while a panel drag is active so we can suppress text
    // selection and hold the resize cursor window-wide.
    let root_class = if state.read().resizing.is_some() {
        format!("{ROOT_CLASS} resizing")
    } else {
        ROOT_CLASS.to_string()
    };
    // Active theme tokens (honouring Sync-with-OS), injected on the root below.
    let theme_css = {
        let s = state.read();
        crate::theme::css_for(&crate::theme::effective_id(&s.theme_id, s.sync_os, s.os_dark))
    };

    rsx! {
        style { dangerous_inner_html: crate::CSS }
        div {
            class: "{root_class}",
            tabindex: "0",
            // The active theme's tokens are injected here as CSS variables,
            // overriding the stylesheet `:root` defaults for the whole app
            // subtree. Unknown id → empty string → `:root` still applies.
            style: "{theme_css}",
            "data-density": if state.read().density_compact { "compact" } else { "comfortable" },
            onkeydown: move |e| handle_key(state, e),
            onmousemove: move |e| {
                if state.read().resizing.is_some() {
                    let c = e.client_coordinates();
                    dispatch(state, Action::ResizeMove { x: c.x, y: c.y });
                }
            },
            onmouseup: move |_| dispatch(state, Action::EndResize),

            ui::header::Header {}

            div { class: "ps-body",
                if state.read().sidebar_open {
                    ui::sidebar::Sidebar {}
                    {resize_handle(state, ResizeTarget::Sidebar)}
                } else {
                    ui::sidebar::SidebarRail {}
                }
                ui::workspace::Workspace {}
                if state.read().inspector_open {
                    {resize_handle(state, ResizeTarget::Inspector)}
                    ui::inspector::Inspector {}
                }
            }

            if state.read().log_open { ui::drawer::Drawer {} }

            ui::statusbar::StatusBar {}

            // ---- overlays / modals ----
            // App-global overlays are always-mounted hosts reading the per-window
            // overlay store (see `crate::overlays`); they render nothing until open.
            ui::modals::CmdkHost {}
            ui::modals::SettingsHost {}
            ui::modals::ExportHost {}
            ui::modals::ConfigHost {}
            // Catalog + tab context menus, the remove-confirm dialog, and the
            // nested-cell view are now self-contained containers rendered by the
            // sidebar / workspace (see `ui::components`).
        }
    }
}

async fn drain_events(state: Signal<AppState>, mut evt_rx: UnboundedReceiver<Event>) {
    while let Some(ev) = evt_rx.recv().await {
        apply_event(state, ev);
    }
}

fn handle_key(
    state: Signal<AppState>,
    e: dioxus_core::Event<dioxus::events::KeyboardData>,
) {
    let mods = e.modifiers();
    let meta = mods.meta() || mods.ctrl();
    let shift = mods.shift();
    match e.key() {
        Key::Character(c) if meta && (c == "k" || c == "K") => {
            e.prevent_default();
            crate::overlays::toggle_cmdk();
        }
        // ⌘T new tab · ⇧⌘T reopen the last closed tab (as the tab menu advertises).
        Key::Character(c) if meta && (c == "t" || c == "T") => {
            e.prevent_default();
            if shift {
                dispatch(state, Action::ReopenTab);
            } else {
                dispatch(state, Action::NewTab);
            }
        }
        // ⌘W close the current tab.
        Key::Character(c) if meta && (c == "w" || c == "W") => {
            e.prevent_default();
            let active = state.read().project.active_ws;
            dispatch(state, Action::CloseTab(active));
        }
        Key::Character(c) if meta && !shift && (c == "s" || c == "S") => {
            e.prevent_default();
            dispatch(state, Action::SaveQuery);
        }
        // ⌘, — toggle Settings via the overlay store.
        Key::Character(c) if meta && c == "," => {
            e.prevent_default();
            crate::overlays::toggle_settings();
        }
        Key::Enter if meta => {
            e.prevent_default();
            dispatch(state, Action::RunQuery);
        }
        // ⌘` — cycle focus between open project windows.
        Key::Character(c) if meta && c == "`" => {
            e.prevent_default();
            crate::window::cycle_to_next_window();
        }
        Key::Escape => dispatch(state, Action::CloseOverlays),
        _ => {}
    }
}

/// The engine→state reducer: fold an engine [`Event`] into the shared state.
/// This is not a UI action — it's driven by [`drain_events`].
pub fn apply_event(mut state: Signal<AppState>, ev: Event) {
    let mut s = state.write();
    match ev {
        Event::QueryResult {
            req_id,
            ws_id,
            result,
        } => {
            if s.pending_req != Some(req_id) {
                return;
            }
            s.running = false;
            s.pending_req = None;
            let sql = s
                .project
                .workspaces
                .iter()
                .find(|w| w.id == ws_id)
                .map(|w| w.sql.clone())
                .unwrap_or_default();
            let hid = s.project.next_hist;
            s.project.next_hist += 1;
            match result {
                Ok(out) => {
                    let total = out.total;
                    s.page = out.page;
                    s.project.history.insert(
                        0,
                        HistoryItem {
                            id: hid,
                            sql,
                            ts_label: "just now".into(),
                            ms: out.elapsed_ms,
                            rows: total,
                            ok: true,
                        },
                    );
                    s.query_error = None;
                    s.set_status(
                        LogKind::Ok,
                        format!(
                            "{total} row{} · {} ms",
                            if total == 1 { "" } else { "s" },
                            out.elapsed_ms,
                        ),
                    );
                    s.push_log(
                        LogKind::Ok,
                        format!("Query executed · {total} rows · {} ms", out.elapsed_ms),
                    );
                    s.result = Some(out);
                }
                Err(e) => {
                    tracing::error!("query failed: {e}");
                    let raw = format!("{e}");
                    let qe = QueryError::parse(&raw, &sql);
                    // Surface a one-line status; the full structured error goes to
                    // the results-pane error view and the (expandable) event row.
                    let head = raw.lines().next().unwrap_or(raw.as_str());
                    s.set_status(LogKind::Error, format!("Query failed · {head}"));
                    s.push_log_err(
                        LogKind::Error,
                        format!("Query failed · {}", qe.etype),
                        Some(qe.clone()),
                    );
                    s.query_error = Some(qe);
                    s.result = None;
                    s.project.history.insert(
                        0,
                        HistoryItem {
                            id: hid,
                            sql,
                            ts_label: "just now".into(),
                            ms: 0,
                            rows: 0,
                            ok: false,
                        },
                    );
                }
            }
        }
        Event::ExplainResult {
            req_id,
            ws_id,
            result,
        } => {
            if s.pending_req != Some(req_id) {
                return;
            }
            s.running = false;
            s.pending_req = None;
            match result {
                Ok(plan) => {
                    let ops = plan.physical.len().max(plan.logical.len());
                    let kind = if plan.analyze { "Plan with metrics" } else { "Query plan" };
                    s.set_status(LogKind::Ok, format!("{kind} · {ops} operators"));
                    s.push_log(
                        LogKind::Ok,
                        format!(
                            "EXPLAIN · {} physical / {} logical operators",
                            plan.physical.len(),
                            plan.logical.len()
                        ),
                    );
                    s.query_error = None;
                    s.result = None;
                    s.plan = Some(plan);
                }
                Err(e) => {
                    tracing::error!("explain failed: {e}");
                    let sql = s
                        .project
                        .workspaces
                        .iter()
                        .find(|w| w.id == ws_id)
                        .map(|w| w.sql.clone())
                        .unwrap_or_default();
                    let qe = QueryError::parse(&e, &sql);
                    let head = e.lines().next().unwrap_or(e.as_str());
                    s.set_status(LogKind::Error, format!("Explain failed · {head}"));
                    s.push_log_err(
                        LogKind::Error,
                        format!("Explain failed · {}", qe.etype),
                        Some(qe.clone()),
                    );
                    s.query_error = Some(qe);
                    s.result = None;
                    s.plan = None;
                }
            }
        }
        Event::PageResult { ws_id, page, result } => {
            let active_id = s.project.workspaces.get(s.project.active_ws).map(|w| w.id);
            if active_id == Some(ws_id) {
                match result {
                    Ok(rows) => {
                        if let Some(res) = &mut s.result {
                            res.rows = rows;
                            res.page = page;
                        }
                        s.page = page;
                    }
                    Err(e) => {
                        tracing::error!("page load failed: {e}");
                        s.push_log(LogKind::Error, format!("Page load failed: {e}"));
                        s.set_status(LogKind::Error, format!("Page load failed · {e}"));
                    }
                }
            }
        }
        Event::Registered {
            table,
            path,
            result,
        } => match result {
            Ok(cols) => {
                let n = cols.len();
                if let Some(t) = s.project.tables.iter_mut().find(|t| t.name == table) {
                    let meta = if t.partition_cols.is_empty() {
                        format!("{n} cols")
                    } else {
                        format!("{n} cols · {} partitions", t.partition_cols.len())
                    };
                    t.columns = cols;
                    t.meta = meta;
                    t.status = RegStatus::Ready;
                    t.error = None;
                    // Keep the stored sources as-entered (relative-to-project where
                    // chosen) — don't overwrite with the engine's resolved path.
                } else {
                    s.project.tables.push(CatalogTable {
                        name: table.clone(),
                        meta: format!("{n} cols"),
                        format: "parquet".into(),
                        sources: vec![path],
                        partition_cols: vec![],
                        columns: cols,
                        open: false,
                        status: RegStatus::Ready,
                        error: None,
                    });
                }
                s.push_log(
                    LogKind::Ok,
                    format!("Registered table '{table}' · {n} cols · schema validated"),
                );
                s.set_status(LogKind::Ok, format!("Registered '{table}'"));
                if crate::overlays::OVERLAYS.peek().config {
                    crate::overlays::close_config();
                    s.cfg.status = CfgStatus::Idle;
                }
            }
            Err(e) => {
                // A brand-new registration started from the config modal that never
                // resolved a schema shouldn't leave a ghost row — drop it. Load-time
                // failures (modal closed) keep the row, marked failed, so the
                // persisted definition survives and its path can be fixed.
                if let Some(pos) = s.project.tables.iter().position(|t| t.name == table) {
                    if crate::overlays::OVERLAYS.peek().config && s.project.tables[pos].columns.is_empty() {
                        s.project.tables.remove(pos);
                    } else {
                        s.project.tables[pos].status = RegStatus::Failed;
                        s.project.tables[pos].error = Some(e.clone());
                    }
                }
                s.cfg.status = CfgStatus::Error;
                s.cfg.error = e.clone();
                tracing::error!("register table '{table}' failed: {e}");
                s.push_log(LogKind::Error, format!("Register '{table}' failed: {e}"));
                s.set_status(LogKind::Error, format!("Register failed · {e}"));
            }
        },
        Event::ViewChanged {
            name,
            sql,
            dropped,
            result,
        } => {
            if dropped {
                s.project.views.retain(|v| v.name != name);
                s.push_log(LogKind::Info, format!("Dropped view '{name}'"));
                s.set_status(LogKind::Info, format!("Dropped view '{name}'"));
            } else {
                match result {
                    Ok(cols) => {
                        if let Some(v) = s.project.views.iter_mut().find(|v| v.name == name) {
                            v.columns = cols;
                            v.sql = sql;
                        } else {
                            s.project.views.push(CatalogView {
                                name: name.clone(),
                                sql,
                                meta: "view".into(),
                                columns: cols,
                                open: false,
                            });
                        }
                        s.push_log(LogKind::Ok, format!("Saved view '{name}'"));
                        s.set_status(LogKind::Ok, format!("Saved view '{name}'"));
                    }
                    Err(e) => {
                        tracing::error!("view '{name}' failed: {e}");
                        s.push_log(LogKind::Error, format!("View '{name}' failed: {e}"));
                        s.set_status(LogKind::Error, format!("View failed · {e}"));
                    }
                }
            }
        }
        Event::Deregistered { table } => {
            s.project.tables.retain(|t| t.name != table);
            s.push_log(LogKind::Info, format!("Removed table '{table}'"));
        }
        Event::Exported { result } => match result {
            Ok((path, rows)) => {
                let msg = if rows > 0 {
                    format!("Exported {rows} rows → {path}")
                } else {
                    format!("Exported → {path}")
                };
                s.push_log(LogKind::Ok, msg.clone());
                s.set_status(LogKind::Ok, msg);
            }
            Err(e) => {
                tracing::error!("export failed: {e}");
                s.push_log(LogKind::Error, format!("Export failed: {e}"));
                s.set_status(LogKind::Error, format!("Export failed · {e}"));
            }
        },
        Event::Notice(m) => {
            tracing::warn!("{m}");
            s.push_log(LogKind::Info, m.clone());
            s.set_status(LogKind::Info, m);
        }
    }
}

