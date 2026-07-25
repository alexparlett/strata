//! The hooks the **window root** calls to stand up + persist per-window state: open the
//! project, restore the session, load history, and drive session autosave. The stores
//! themselves (`ProjectState`, `SessionState`, `History`) and their serde vocabulary
//! (`strata_model`) live elsewhere; this is only the Freya wiring.

use std::path::PathBuf;
use std::time::Duration;

use async_io::Timer;
use freya::prelude::{
    spawn, spawn_forever, use_hook, use_provide_context, use_side_effect, use_state, Platform,
    State, TaskHandle, WritableUtils,
};
use freya::radio::{use_init_radio_station, use_radio, use_radio_station, RadioStation};
use strata_core::engine::TableSpec;
use strata_core::project as project_io;
use strata_model::WindowGeom;

use crate::apps::project::contexts::EngineCtx;

use super::catalog::{use_init_catalog_scan, CatalogScan};
use super::history::{History, HistoryCtx};
use super::{Chan, ProjChan, ProjectState, SessionState};

/// Initialise this window's Session store and provide it via context. Pulls the open
/// project's root from the [`ProjectState`] store already in context (`use_init_project`
/// runs first in the window root) and restores its `.strata/session.json` (state-arch §5);
/// with no project on disk, no session file, or an unparseable one, falls back to a single
/// blank tab. Call once in the window root; returns the station for the root to read /
/// drive.
pub fn use_init_session() -> RadioStation<SessionState, Chan> {
    let project = use_radio_station::<ProjectState, ProjChan>();
    let root = project.peek().root.clone();
    use_init_radio_station::<SessionState, Chan>(move || restore_or_new(root))
}

/// Restore the persisted session for `root`, falling back to one blank tab. A corrupt
/// session file logs and yields a blank session rather than bricking the window.
fn restore_or_new(root: PathBuf) -> SessionState {
    // A present-but-unparseable session file is surfaced, not silently discarded — losing
    // the user's tabs to a blank fallback is exactly what we don't want. Missing is a
    // different, expected case (`Ok(None)`): a fresh project with no session yet.
    let restored = project_io::load_session(&root)
        .unwrap_or_else(|e| panic!("load session: {e}"))
        .and_then(SessionState::from_snapshot);

    restored.unwrap_or_else(new_session)
}

/// A brand-new session: one blank scratch tab. Used when the opened project has no
/// `session.json` yet (autosave writes it on the first change).
fn new_session() -> SessionState {
    let mut s = SessionState::default();
    s.open_blank();
    s
}

/// Initialise this window's Project store — open the project folder (argv\[1\], default
/// the repo's `sample/`), scaffolding a fresh `.strata/` when the folder has none — and
/// kick off engine registration of its defs (tables, then views). Also provides the
/// window's [`CatalogScan`] flag, since this is where the first pass starts. Call once in
/// the window root, after the engine is in context.
///
/// The open itself is synchronous (one small JSON read, needed before anything can
/// render meaningfully); registration is IO-heavy (schema inference reads file footers)
/// and runs as a spawned task, landing results row by row through [`ProjChan::Tables`] /
/// [`ProjChan::Views`] so rows flip `Loading → Ready/Failed` as answers arrive.
pub fn use_init_project(engine: &EngineCtx, root: PathBuf) -> RadioStation<ProjectState, ProjChan> {
    let station = use_init_radio_station::<ProjectState, ProjChan>(move || open_project(root));
    let scan = use_init_catalog_scan();
    let engine = engine.clone();
    use_hook(move || {
        let (tables, views) = whole_catalog(station);
        spawn(scan_catalog(engine, station, scan, tables, views));
    });
    station
}

/// Every def in the catalog, as the work list of a full pass: table names, then view names in
/// dependency order.
///
/// Read **before** the rows are reset, because resetting is what throws the ordering
/// information away — a `Loading` row has no `view_deps` to sort by (at project open nothing
/// has answered yet either, which is what the scan's fixed-point retry is for).
fn whole_catalog(station: RadioStation<ProjectState, ProjChan>) -> (Vec<String>, Vec<String>) {
    let p = station.peek();
    let tables = p.tables.iter().map(|t| t.def.name.clone()).collect();
    let views = p.refresh_order(p.views.iter().map(|v| v.def.name.clone()).collect());
    (tables, views)
}

/// The sidebar's ↻ (P3-03): re-scan the whole catalog — re-infer every table's schema from
/// its def, then re-create every view over what that found.
///
/// **Re-scan is re-registration**, from the defs, not a walk of what the engine happens to
/// hold. `Engine::register` deregisters and rebuilds each table from a re-`infer_schema`d
/// config (see `catalog::register_external`), which is the same re-infer a walk of the live
/// providers would do — and, because the def is the input, it *also* retries a table whose
/// first registration failed. That is the case the button most needs to serve: the user fixes
/// a path or restores a file and presses ↻. A live-provider walk can't do it at all, since a
/// failed row has no provider to rebuild from.
///
/// Rows drop to `Loading` first so the pane reads as re-scanning rather than as settled data;
/// answers land through the normal registration path. A no-op while a scan is already running
/// — the button is disabled for the duration, and this guards the rest.
///
/// A re-scan that *fails* leaves the table deregistered (`register_external` deregisters before
/// it infers), which is load-time semantics and the honest outcome: the files really are
/// unreadable now, the row says `Failed`, and any view over it fails its own re-create rather
/// than quietly answering from the provider that no longer matches the disk.
///
/// Only the inferred *schema* is refreshed. File sets, row counts and partition values are
/// already live: we run no `ListFilesCache`, so DataFusion re-`LIST`s per scan.
pub fn refresh_catalog(
    engine: EngineCtx,
    mut station: RadioStation<ProjectState, ProjChan>,
    scan: CatalogScan,
) {
    if *scan.peek() {
        return;
    }
    let (tables, views) = whole_catalog(station);
    station.write_channel(ProjChan::Tables).reload_tables();
    station.write_channel(ProjChan::Views).reload_views();
    spawn_scan(engine, station, scan, tables, views);
}

/// A catalog row's **Refresh table** (P3-06): re-infer *one* table's schema from its def, then
/// re-create the views that would otherwise be left reading the provider it replaced.
///
/// The same pass as the sidebar's ↻ ([`refresh_catalog`]), narrowed to one row — same
/// re-registration semantics (so a failed table is retried), same flag, same landing path. What
/// is narrower is only *which* rows drop to `Loading`: this one, plus the views
/// [`ProjectState::views_to_refresh`] names. Every other row keeps the verdict it already has,
/// which is the whole difference between asking about a table and re-scanning the project.
///
/// A no-op while any scan is in flight — the menu item is disabled for the duration, and this
/// guards the rest.
pub fn refresh_table(
    engine: EngineCtx,
    mut station: RadioStation<ProjectState, ProjChan>,
    scan: CatalogScan,
    name: String,
) {
    if *scan.peek() {
        return;
    }
    // Read the work list before anything is reset: `views_to_refresh` orders by what the rows
    // currently know, and resetting them is what discards it.
    let views = {
        let p = station.peek();
        if !p.tables.iter().any(|t| t.def.name == name) {
            return;
        }
        p.views_to_refresh(&name)
    };
    station.write_channel(ProjChan::Tables).reload_table(&name);
    if !views.is_empty() {
        let mut p = station.write_channel(ProjChan::Views);
        for view in &views {
            p.reload_view(view);
        }
    }
    spawn_scan(engine, station, scan, vec![name], views);
}

/// Start a scan **outside the scope that ordered it**.
///
/// `spawn` binds a task to `current_scope_id()`, which during an event is the scope of the element
/// that owns the handler — a `MenuButton` inside the row's context menu, or the sidebar's ↻. Both
/// can be gone in the same tick: the menu item closes the menu, and collapsing the sidebar
/// unmounts the button. Scope teardown drops that scope's tasks *before the future is ever
/// polled*, so the rows would be reset to `Loading`, the engine never asked, and every affected
/// row — the table and the views over it — would spin forever with nothing coming. That is exactly
/// what a press of Refresh did until this existed. (`drop_confirm` hit the same trap: the def went,
/// the file was written, and DataFusion was never told.)
///
/// The pass has to outlive whatever ordered it, so it belongs to the root. It is safe there
/// because it holds only `Copy` handles: a `RadioStation` write into a store whose window has gone
/// notifies nobody, and the flag it clears is that window's own.
fn spawn_scan(
    engine: EngineCtx,
    station: RadioStation<ProjectState, ProjChan>,
    scan: CatalogScan,
    tables: Vec<String>,
    views: Vec<String>,
) {
    spawn_forever(scan_catalog(engine, station, scan, tables, views));
}

/// One catalog scan over `tables` + `views`, flag held for its duration — the project-open pass,
/// every ↻ and every row Refresh are the same pass at different widths, so none of them can run
/// while another is in flight.
async fn scan_catalog(
    engine: EngineCtx,
    station: RadioStation<ProjectState, ProjChan>,
    mut scan: CatalogScan,
    tables: Vec<String>,
    views: Vec<String>,
) {
    scan.set(true);
    register_defs(engine, station, tables, views).await;
    scan.set(false);
}

/// Build the Project store for the resolved `root`: load its `project.json` defs, or
/// scaffold a fresh `.strata/` when the folder has none. A defs file that won't load /
/// scaffold is unrecoverable (fails fast) — the store is only ever built full, never a
/// rootless default.
fn open_project(root: PathBuf) -> ProjectState {
    let defs = if project_io::exists_at(&root) {
        project_io::load_defs(&root)
    } else {
        project_io::scaffold(&root)
    }
    .unwrap_or_else(|e| panic!("open project `{}`: {e}", root.display()));
    ProjectState::from_defs(defs, root)
}

/// Register the named defs on the engine: each table (relative sources resolved against the
/// project folder), then each view. One pass, shared by project open, the sidebar's ↻ re-scan
/// ([`refresh_catalog`]) and a row's Refresh ([`refresh_table`]) — a re-scan *is* a
/// re-registration, so there is one implementation of "make the engine match the defs", not
/// several that can drift. The three differ only in the work list they hand in.
///
/// `views` is taken **in order**, which the caller has already sorted so a view is re-created
/// after everything it reads ([`ProjectState::refresh_order`]). That ordering is only knowable
/// once the views have answered at least once; at project open none of them have, which is what
/// the fixed-point retry below is for. DataFusion requires a view's dependencies to exist when
/// its `CREATE VIEW` plans, so rather than parse SQL to topo-sort, each round creates what it
/// can and a view whose dependency landed last round succeeds this round. No progress → the
/// remainder are genuinely broken (bad SQL or a missing table) and their errors land on their
/// rows.
///
/// A name with no def is skipped — the row went while the pass was being planned.
async fn register_defs(
    engine: EngineCtx,
    mut station: RadioStation<ProjectState, ProjChan>,
    tables: Vec<String>,
    views: Vec<String>,
) {
    // Snapshot the work up front (peek — a task has no reactive context): results land
    // by name, so concurrent def edits can't be clobbered by a stale row write.
    let (tables, views) = {
        let p = station.peek();
        let root = p.root.clone();
        let tables: Vec<(String, TableSpec)> = tables
            .into_iter()
            .filter_map(|name| {
                let def = &p.tables.iter().find(|t| t.def.name == name)?.def;
                Some((
                    name,
                    TableSpec {
                        name: def.name.clone(),
                        paths: def
                            .sources
                            .iter()
                            .map(|s| project_io::resolve_source(&root, s))
                            .collect(),
                        format: def.format.clone(),
                        partitions: def.partition_cols.clone(),
                    },
                ))
            })
            .collect();
        let views: Vec<(String, String)> = views
            .into_iter()
            .filter_map(|name| {
                let sql = p.views.iter().find(|v| v.def.name == name)?.def.sql.clone();
                Some((name, sql))
            })
            .collect();
        (tables, views)
    };

    for (name, spec) in tables {
        match engine.register(spec).await {
            Ok(meta) => station
                .write_channel(ProjChan::Tables)
                .table_registered(&name, meta),
            Err(e) => {
                tracing::error!("register table '{name}' failed: {e}");
                station
                    .write_channel(ProjChan::Tables)
                    .table_failed(&name, e);
            }
        }
    }

    let mut pending = views;
    while !pending.is_empty() {
        let before = pending.len();
        let mut failed = Vec::new();
        for (name, sql) in pending {
            match engine.create_view(name.clone(), sql.clone()).await {
                Ok(meta) => station
                    .write_channel(ProjChan::Views)
                    .view_registered(&name, meta),
                Err(e) => failed.push((name, sql, e)),
            }
        }
        if failed.len() == before {
            // A full round without progress — the rest are genuinely broken.
            for (name, _, e) in failed {
                tracing::error!("create view '{name}' failed: {e}");
                station.write_channel(ProjChan::Views).view_failed(&name, e);
            }
            break;
        }
        pending = failed.into_iter().map(|(n, s, _)| (n, s)).collect();
    }
}

/// Initialise this window's query-history satellite: load `.strata/history.jsonl` and
/// provide the [`History`] store via context. Reads the project root from the `ProjectState`
/// store already in context (like [`use_init_session`]). Call once in the window root.
pub fn use_init_history() -> HistoryCtx {
    let project = use_radio_station::<ProjectState, ProjChan>();
    let root = project.peek().root.clone();
    use_provide_context(move || State::create(History::load(&root)))
}

/// How long the session must sit quiet before autosave writes. Every change cancels and
/// re-arms, so a typing burst (or a flurry of tab / window ops) writes once, on the settled
/// state.
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Drive debounced autosave of `.strata/session.json`. Call once in the window root, after
/// the Session + Project stores are in context. Subscribes to [`Chan::Persist`] — the fan-in
/// every structural / buffer / view-mode write derives — plus the window's live geometry
/// ([`Platform`]), and on any change arms a cancel-and-rearm timer that snapshots the session
/// (+ geometry) and writes it. The mount pass is skipped (the just-loaded session already
/// matches disk).
///
/// `restored` is the geometry the window was *created* with (`ProjectApp::window` read it off
/// the same session file) — the seed for the "last normal geometry" the save persists, see
/// below.
///
/// **Our fill, and fullscreen, are not remembered.** A window we filled (the header's
/// double-press) or that is in native fullscreen has the *screen's* geometry, not a size the user
/// picked; persisting it would reopen every later launch at screen size and quietly lose their
/// real one. So while the window is in either state the save keeps rewriting the last geometry it
/// had in neither — normal IDE behaviour.
///
/// A window the **user** sized to fill the screen is the opposite case and does persist, which is
/// why the test is `filled_by_app` and not `Platform::is_maximized` alone: that mirrors macOS's
/// `isZoomed`, a *frame comparison*, so a hand-tiled window reads as zoomed too (see
/// `views::header::title_bar_press`). The `&& is_filled` keeps the pair self-healing — a stale
/// mark can never freeze the geometry, because leaving fill drops the guard either way.
///
/// The subscription is inside the effect's reactive scope, not the caller's render, so a
/// keystroke re-runs only this effect — the root render is untouched.
pub fn use_autosave(restored: Option<WindowGeom>, filled_by_app: State<bool>) {
    // One handle to subscribe the effect (Persist), one to peek the value at fire time.
    let subscribe = use_radio::<SessionState, Chan>(Chan::Persist);
    let session = use_radio_station::<SessionState, Chan>();
    let project = use_radio_station::<ProjectState, ProjChan>();
    // The window's live geometry + window state (logical units). All `Copy` State signals —
    // reading them in the effect also makes a resize / move / fill trigger a save.
    let platform = Platform::get();
    let root_size = platform.root_size;
    let window_position = platform.window_position;
    let is_filled = platform.is_maximized;
    let is_fullscreen = platform.is_fullscreen;
    let mut pending = use_state(|| None::<TaskHandle>);
    let mut armed = use_state(|| false);
    // The last geometry the window had while neither filled nor fullscreen — what a restart
    // restores to. Seeded with what the window opened at, so filling before ever resizing still
    // persists the real size rather than dropping it.
    let normal_geom = use_state(move || restored);

    use_side_effect(move || {
        // These reads bind the effect to session edits (`Chan::Persist`) and to window
        // resize / move / fill; the values themselves are captured at fire time in the task, so
        // the debounce always writes the settled state.
        let _ = subscribe.read().active;
        let _ = root_size.read();
        let _ = window_position.read();
        let _ = is_filled.read();
        let _ = is_fullscreen.read();
        let _ = filled_by_app.read();
        // Skip the mount pass: nothing has changed since load, and the loaded session is
        // already on disk — only real edits should rewrite the file.
        if !*armed.peek() {
            armed.set(true);
            return;
        }
        if let Some(task) = *pending.peek() {
            task.cancel();
        }
        let task = spawn(async move {
            Timer::after(AUTOSAVE_DEBOUNCE).await;
            // Any newer change would have cancelled this task before now.
            let root = project.peek().root.clone();
            let mut snapshot = session.peek().snapshot();
            // Geometry from *our* fill, or from fullscreen, is the screen's rather than the
            // user's — remember only the normal one, and keep writing it while either holds.
            let mut normal_geom = normal_geom;
            let transient = (*filled_by_app.peek() && *is_filled.peek()) || *is_fullscreen.peek();
            if !transient {
                let (pos, size) = (window_position.peek(), root_size.peek());
                normal_geom.set(Some(WindowGeom {
                    x: pos.x,
                    y: pos.y,
                    width: size.width,
                    height: size.height,
                }));
            }
            snapshot.window = *normal_geom.peek();
            if let Err(e) = project_io::save_session(&root, &snapshot) {
                tracing::error!("autosave session: {e}");
            }
        });
        pending.set(Some(task));
    });
}
