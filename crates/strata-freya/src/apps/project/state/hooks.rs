//! The hooks the **window root** calls to stand up + persist per-window state: open the
//! project, restore the session, load history, and drive session autosave. The stores
//! themselves (`ProjectState`, `SessionState`, `History`) and their serde vocabulary
//! (`strata_model`) live elsewhere; this is only the Freya wiring.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use async_io::Timer;
use freya::prelude::{
    spawn, use_hook, use_provide_context, use_side_effect, use_state, Platform, State, TaskHandle,
    WritableUtils,
};
use freya::radio::{use_init_radio_station, use_radio, use_radio_station, RadioStation};
use strata_core::engine::TableSpec;
use strata_core::project as project_io;
use strata_model::WindowGeom;

use crate::apps::project::contexts::EngineCtx;

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
/// kick off engine registration of its defs (tables, then views). Call once in the
/// window root, after the engine is in context.
///
/// The open itself is synchronous (one small JSON read, needed before anything can
/// render meaningfully); registration is IO-heavy (schema inference reads file footers)
/// and runs as a spawned task, landing results row by row through [`ProjChan::Tables`] /
/// [`ProjChan::Views`] so rows flip `Loading → Ready/Failed` as answers arrive.
pub fn use_init_project(engine: &EngineCtx, root: PathBuf) -> RadioStation<ProjectState, ProjChan> {
    let station = use_init_radio_station::<ProjectState, ProjChan>(move || open_project(root));
    let engine = engine.clone();
    use_hook(move || {
        spawn(register_defs(engine, station));
    });
    station
}

/// Resolve the launch project's folder: argv\[1\] as the project folder, defaulting to
/// `sample/` (the committed sample project) until the launcher / open-dialog lands
/// (P4-02/P4-13 UI). Called once in [`ProjectApp::window`](crate::apps::project::ProjectApp)
/// **before** the window opens, so its saved geometry can seed the window.
///
/// A project **can't exist without a root**, so a folder that won't canonicalize is an
/// unrecoverable open error. The eventual handling is to close the window (and reopen the
/// launcher if it was the last one) — see P4-01/P4-02/P4-13; until that multi-window
/// plumbing exists, this fails fast so the fault is loud, not papered over.
pub fn resolve_launch_root() -> PathBuf {
    let arg = env::args().nth(1).unwrap_or_else(|| "sample".into());
    fs::canonicalize(&arg).unwrap_or_else(|e| panic!("open project folder `{arg}`: {e}"))
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

/// Register the opened project's defs on the engine: every table (relative sources
/// resolved against the project folder), then every view.
///
/// Views can read other views, and DataFusion requires a view's dependencies to exist
/// when its `CREATE VIEW` plans — but the defs file carries no dependency order (it's
/// sorted alphabetically). Rather than parse SQL to topo-sort, retry to a fixed point:
/// each round creates what it can, and a view whose dependency landed last round
/// succeeds this round. No progress → the remainder are genuinely broken (bad SQL or a
/// missing table) and their errors land on their rows.
async fn register_defs(engine: EngineCtx, mut station: RadioStation<ProjectState, ProjChan>) {
    // Snapshot the work up front (peek — a task has no reactive context): results land
    // by name, so concurrent def edits can't be clobbered by a stale row write.
    let (tables, views) = {
        let p = station.peek();
        let root = p.root.clone();
        let tables: Vec<(String, TableSpec)> = p
            .tables
            .iter()
            .map(|t| {
                (
                    t.def.name.clone(),
                    TableSpec {
                        name: t.def.name.clone(),
                        paths: t
                            .def
                            .sources
                            .iter()
                            .map(|s| project_io::resolve_source(&root, s))
                            .collect(),
                        format: t.def.format.clone(),
                        partitions: t.def.partition_cols.clone(),
                    },
                )
            })
            .collect();
        let views: Vec<(String, String)> = p
            .views
            .iter()
            .map(|v| (v.def.name.clone(), v.def.sql.clone()))
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
