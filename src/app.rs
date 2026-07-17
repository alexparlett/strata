//! Root component: owns the `Signal<AppState>`, bridges the DataFusion engine
//! (spawn + event-draining coroutine), routes keyboard shortcuts, and lays out
//! the shell + modals.
//!
//! UI intents are funneled through [`crate::action::dispatch`]; this file only
//! holds the component, the engine wiring, and the engine→state reducer
//! ([`apply_event`]).

use dioxus::desktop::{use_muda_event_handler, use_wry_event_handler};
use dioxus::prelude::*;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::engine::{self, Engine, EngineStoreExt, Event};
use crate::menu::MenuCmd;
use crate::project::ProjectStoreExt;
use crate::query_error::QueryError;
use crate::state::{AppState, CatalogTable, CatalogView, LogKind, RegStatus};
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

    // Persist live editor SQL edits: the controlled editor writes its workspace's
    // `sql` lens directly (bypassing `dispatch`'s autosave), so this effect
    // subscribes to the session store and writes a debounced `session.json`.
    crate::action::projects::use_persist_session(state);

    // Track this window so siblings can be focused / cycled, and so "Close
    // project" knows whether it's the last one.
    let win_id = use_hook(crate::window::register_current_window);

    // Spawn the engine (seeded with the current engine config overrides, W2), drain
    // its events, and load the assigned project.
    use_hook(move || {
        spawn(drain_events(state));
        if !open_path.is_empty() {
            crate::action::projects::load_current(state, std::path::PathBuf::from(open_path));
        }
        // The launch window reopens the rest of last session's projects (once),
        // deferred so window creation doesn't run during this window's mount.
        spawn(async move {
            crate::spawn_startup_rest();
        });
    });

    // Live-apply engine config (W2): re-send the overrides to this window's engine
    // whenever the applied settings change (a Settings ▸ Engine Save), so
    // execution / parser / optimizer / format options take effect without a reopen.
    // `runtime.*` changes emit a Notice from the engine (they need a reopen). Reading
    // `engine_overrides()` subscribes this effect to the shared settings.
    use_effect(move || {
        let overrides = crate::settings::engine_overrides();
        crate::command!(SetEngineConfig(overrides));
    });

    // Global keyboard commands (⌘F/⌘K/…) are OS hotkeys — the webview swallows key events,
    // so a DOM handler can't hear them once focus leaves the app subtree. `crate::hotkeys`
    // registers them while this window is focused; `focused` is relayed from the wry
    // `Focused` event below.
    crate::hotkeys::use_shortcuts(state);

    // Persist window geometry + save on an OS close-button (the window is still
    // alive here, unlike `use_drop`). Does *not* open the launcher — an OS close
    // never does; that's reserved for the explicit "Close project" action.
    use_wry_event_handler(move |event, _| {
        use dioxus::desktop::tao::event::{Event as TaoEvent, WindowEvent};
        if let TaoEvent::WindowEvent {
            window_id, event, ..
        } = event
        {
            if *window_id != win_id {
                return;
            }
            match event {
                WindowEvent::CloseRequested => {
                    crate::action::projects::save_on_close(state, win_id);
                }
                // Follow the OS light/dark switch live (drives Sync-with-OS
                // without a restart). Reactive, so the theme re-applies at once.
                WindowEvent::ThemeChanged(theme) => {
                    use dioxus::desktop::tao::window::Theme;
                    crate::settings::set_os_dark(*theme == Theme::Dark);
                }
                _ => {}
            }
        }
    });

    // Native File-menu commands (S11). The macOS menu is app-global and its events
    // carry only the item id, so act only when this is the focused window; relay the
    // id into a signal a `use_effect` consumes, so the (async) open-folder dialog
    // runs with a reactive scope.
    let mut menu_cmd = use_signal(|| None::<MenuCmd>);
    use_muda_event_handler(move |ev| {
        if crate::window::is_focused_window(win_id) {
            menu_cmd.set(MenuCmd::parse(&ev.id().0));
        }
    });
    use_effect(move || {
        if let Some(id) = menu_cmd() {
            menu_cmd.set(None);
            crate::menu::run_project_command(state, &id);
        }
    });

    // macOS: dark NSWindow background so a resize doesn't flash white.
    #[cfg(target_os = "macos")]
    use_hook(|| crate::window::paint_ns_background(0.043, 0.055, 0.075));

    // Drop snapshots + de-register this window on close.
    use_drop(move || {
        crate::command!(CleanupAll);
        crate::window::unregister_window(win_id);
    });

    let root_class = ROOT_CLASS;
    // Seed the shared settings context + OS appearance (once) and read the effective
    // theme reactively — injected on the root below, so a theme preview / OS switch
    // re-themes this window.
    let theme_css = crate::settings::use_settings();

    rsx! {
        style { dangerous_inner_html: crate::CSS }
        div {
            class: "{root_class}",
            tabindex: "0",
            // The active theme's tokens are injected here as CSS variables,
            // overriding the stylesheet `:root` defaults for the whole app
            // subtree. Unknown id → empty string → `:root` still applies.
            style: "{theme_css}",
            "data-density": if crate::settings::density_compact() { "compact" } else { "comfortable" },
            onkeydown: move |e| handle_key(state, e),

            ui::header::Header {}

            // S23 (RustRover model): a permanent activity rail on the far left, then
            // a right-area column so the bottom drawer pushes the panes up while the
            // thin rail stays full height. No status footer — the rail carries
            // Events/History, run state lives in the results panel.
            div { class: "ps-body",
                ui::activity_rail::ActivityRail {}
                div { class: "ps-right",
                    div { class: "ps-panes",
                        if crate::layout::sidebar_open() {
                            ui::sidebar::Sidebar {}
                        }
                        ui::workbench::Workbench {}
                        if crate::layout::inspector_open() {
                            ui::inspector::Inspector {}
                        }
                    }
                    ui::drawer::BottomDraw {}
                }
            }

            // ---- overlays / modals ----
            // App-global overlays are always-mounted hosts reading the per-window
            // overlay store (see `crate::overlays`); they render nothing until open.
            ui::modals::CmdkHost {}
            ui::modals::ExportHost {}
            ui::modals::ConfigHost {}
            ui::modals::CloseConfirmHost {}
            ui::modals::RunningCloseHost {}
            ui::modals::OpenPromptHost {}
            ui::modals::EngineRestartHost {}
            // Catalog + tab context menus, the remove-confirm dialog, and the
            // nested-cell view are now self-contained containers rendered by the
            // sidebar / workspace (see `ui::components`).
        }
    }
}

async fn drain_events(state: Signal<AppState>) {
    let mut evt_rx = crate::engine::Engine::take_evt_rx();
    while let Some(ev) = evt_rx.recv().await {
        apply_event(state, ev);
    }
}

// Global commands (⌘F/⌘K/…) arrive via the OS hotkey layer (`crate::hotkeys`), which is
// focus-independent. This DOM handler runs only the *non*-global commands (Esc → Cancel),
// so the two layers never double-fire. It's best-effort: Esc-to-cancel-a-query works only
// when focus is in the app subtree, which is fine — an open overlay dismisses its own Esc.
fn handle_key(state: Signal<AppState>, e: dioxus_core::Event<dioxus::events::KeyboardData>) {
    if let Some(cmd) = crate::keymap::resolve(&e) {
        if !crate::keymap::is_global(cmd) && crate::keymap::run(state, cmd) {
            e.prevent_default();
        }
    }
}

/// The engine→state reducer: fold an engine [`Event`] into the shared state.
/// This is not a UI action — it's driven by [`drain_events`].
pub fn apply_event(state: Signal<AppState>, ev: Event) {
    // Set when an engine event durably changes the project (a config register
    // adds/edits a table). Engine events aren't dispatched, so they don't hit the
    // normal autosave path — we persist explicitly at the end.
    let mut autosave_after = false;
    match ev {
        Event::QueryResult {
            req_id,
            ws_id,
            result,
        } => {
            // Route to the owning tab; drop the result if that tab is gone or has
            // since started a newer query (its `pending_req` moved on).
            if !crate::runs::is_pending(ws_id, req_id) {
                return;
            }
            // The owning workspace's SQL now lives in the reactive session store.
            let sql = crate::session::snapshot()
                .workspaces
                .iter()
                .find(|w| w.id == ws_id)
                .map(|w| w.sql.clone())
                .unwrap_or_default();
            match result {
                Ok((out, batch)) => {
                    let total = out.total;
                    let elapsed = out.elapsed_ms;
                    let page = out.page;
                    crate::project::record_run(sql, elapsed, total);
                    crate::event_ok!("Query executed · {total} rows · {} ms", elapsed);
                    crate::runs::edit_existing(ws_id, |run| {
                        run.running = false;
                        run.pending_req = None;
                        run.page = page;
                        run.query_error = None;
                        run.result = Some(out);
                        run.page_batch = Some(batch);
                        run.sel = None;
                        run.sel_anchor = None;
                        run.sort = None;
                        run.ran_at = Some(std::time::Instant::now());
                    });
                }
                Err(e) => {
                    tracing::error!("query failed: {e}");
                    let raw = format!("{e}");
                    let qe = QueryError::parse(&raw, &sql);
                    crate::events::push_err(
                        LogKind::Error,
                        format!("Query failed · {}", qe.etype),
                        Some(qe.clone()),
                        Some(ws_id),
                    );
                    crate::project::record_fail(sql);
                    crate::runs::edit_existing(ws_id, |run| {
                        run.running = false;
                        run.pending_req = None;
                        run.query_error = Some(qe);
                        run.result = None;
                    });
                }
            }
        }
        Event::QueryCancelled {
            req_id,
            ws_id,
            elapsed_ms,
        } => {
            // Drop if the tab moved on or closed.
            if !crate::runs::is_pending(ws_id, req_id) {
                return;
            }
            let sql = crate::session::snapshot()
                .workspaces
                .iter()
                .find(|w| w.id == ws_id)
                .map(|w| w.sql.clone())
                .unwrap_or_default();
            crate::project::record_cancel(sql, elapsed_ms);
            crate::event_warn!("Query cancelled · {elapsed_ms} ms");
            crate::runs::edit_existing(ws_id, |run| {
                run.running = false;
                run.pending_req = None;
                // Cancellation isn't an error — leave any prior result / error as-is.
            });
        }
        Event::ExplainResult {
            req_id,
            ws_id,
            result,
        } => {
            if !crate::runs::is_pending(ws_id, req_id) {
                return;
            }
            match result {
                Ok(plan) => {
                    crate::event_ok!(
                        "EXPLAIN · {} physical / {} logical operators",
                        plan.physical.len(),
                        plan.logical.len()
                    );
                    crate::runs::edit_existing(ws_id, |run| {
                        run.running = false;
                        run.pending_req = None;
                        run.query_error = None;
                        run.result = None;
                        run.plan = Some(plan);
                    });
                }
                Err(e) => {
                    tracing::error!("explain failed: {e}");
                    let sql = crate::session::snapshot()
                        .workspaces
                        .iter()
                        .find(|w| w.id == ws_id)
                        .map(|w| w.sql.clone())
                        .unwrap_or_default();
                    let qe = QueryError::parse(&e, &sql);
                    crate::events::push_err(
                        LogKind::Error,
                        format!("Explain failed · {}", qe.etype),
                        Some(qe.clone()),
                        Some(ws_id),
                    );
                    crate::runs::edit_existing(ws_id, |run| {
                        run.running = false;
                        run.pending_req = None;
                        run.query_error = Some(qe);
                        run.result = None;
                        run.plan = None;
                    });
                }
            }
        }
        Event::PageResult {
            ws_id,
            page,
            result,
        } => match result {
            Ok((rows, batch)) => {
                crate::runs::edit_existing(ws_id, |run| {
                    if let Some(res) = &mut run.result {
                        res.rows = rows;
                        res.page = page;
                    }
                    run.page_batch = Some(batch);
                    run.page = page;
                });
            }
            Err(e) => {
                tracing::error!("page load failed: {e}");
                crate::event_error!("Page load failed: {e}");
            }
        },
        Event::Registered {
            table,
            path,
            result,
        } => match result {
            Ok(cols) => {
                let n = cols.len();
                // A config-originated register finalizes from the stashed row data
                // (the project was untouched until now); otherwise it's a load-time
                // register updating the row that project-open already created.
                if let Some(p) = crate::overlays::take_pending_register(&table) {
                    let meta = if p.partition_cols.is_empty() {
                        format!("{n} cols")
                    } else {
                        format!("{n} cols · {} partitions", p.partition_cols.len())
                    };
                    // Replace any existing row of this name (an edit re-register).
                    crate::project::upsert_table(CatalogTable {
                        name: table.clone(),
                        meta,
                        format: p.format,
                        sources: p.sources,
                        partition_cols: p.partition_cols,
                        columns: cols,
                        open: false,
                        status: RegStatus::Ready,
                        error: None,
                    });
                    crate::overlays::close_config();
                    autosave_after = true;
                } else {
                    let store = crate::project::store();
                    let mut t = store.tables();
                    let mut tables = t.write();
                    if let Some(t) = tables.iter_mut().find(|t| t.name == table) {
                        let meta = if t.partition_cols.is_empty() {
                            format!("{n} cols")
                        } else {
                            format!("{n} cols · {} partitions", t.partition_cols.len())
                        };
                        t.columns = cols;
                        t.meta = meta;
                        t.status = RegStatus::Ready;
                        t.error = None;
                    } else {
                        tables.push(CatalogTable {
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
                }
                crate::event_ok!("Registered table '{table}' · {n} cols · schema validated");
            }
            Err(e) => {
                // A config-originated register that failed → keep the window open
                // with an inline error; the project was never touched, so there's
                // nothing to clean up. A load-time failure marks the existing row
                // failed so its definition survives and its path can be fixed.
                if crate::overlays::take_pending_register(&table).is_some() {
                    crate::overlays::set_config_err(e.clone());
                } else {
                    let store = crate::project::store();
                    let mut t = store.tables();
                    let mut tables = t.write();
                    if let Some(t) = tables.iter_mut().find(|t| t.name == table) {
                        t.status = RegStatus::Failed;
                        t.error = Some(e.clone());
                    }
                }
                tracing::error!("register table '{table}' failed: {e}");
                crate::event_error!("Register '{table}' failed: {e}");
            }
        },
        Event::ViewChanged {
            name,
            sql,
            dropped,
            result,
        } => {
            if dropped {
                crate::project::remove_view(&name);
                autosave_after = true;
                crate::event_info!("Dropped view '{name}'");
            } else {
                match result {
                    Ok(cols) => {
                        let store = crate::project::store();
                        let mut v = store.views();
                        let mut views = v.write();
                        if let Some(v) = views.iter_mut().find(|v| v.name == name) {
                            v.columns = cols;
                            v.sql = sql;
                        } else {
                            views.push(CatalogView {
                                name: name.clone(),
                                sql,
                                meta: "view".into(),
                                columns: cols,
                                open: false,
                            });
                        }
                        drop(views);
                        crate::event_ok!("Saved view '{name}'");
                        autosave_after = true;
                    }
                    Err(e) => {
                        tracing::error!("view '{name}' failed: {e}");
                        crate::event_error!("View '{name}' failed: {e}");
                    }
                }
            }
        }
        Event::Deregistered { table } => {
            crate::project::remove_table(&table);
            autosave_after = true;
            crate::event_info!("Removed table '{table}'");
        }
        Event::Exported { result } => match result {
            Ok((path, rows)) => {
                let msg = if rows > 0 {
                    format!("Exported {rows} rows → {path}")
                } else {
                    format!("Exported → {path}")
                };
                crate::event_ok!("{msg}");
            }
            Err(e) => {
                tracing::error!("export failed: {e}");
                crate::event_error!("Export failed: {e}");
            }
        },
        Event::Functions {
            scalar,
            aggregate,
            window,
        } => {
            // The engine's registered functions (A9/F5) — feed the SQL language service.
            crate::engine::Engine::set_functions(crate::sql::FunctionCatalog {
                scalar,
                aggregate,
                window,
            });
        }
        Event::Notice(m) => {
            tracing::warn!("{m}");
            crate::event_info!("{m}");
        }
        Event::EngineRestartRequired => {
            // A saved `datafusion.runtime.*` change can't apply to the running engine
            // (W2) — offer a window restart via the prompt.
            crate::overlays::open_engine_restart();
        }
    }
    if autosave_after {
        crate::action::projects::autosave(state);
    }
}
