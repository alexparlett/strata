//! The hooks the **window root** calls to stand up + persist per-window state: open the
//! project, restore the session, load history, and drive session autosave. The stores
//! themselves (`ProjectState`, `SessionState`, `History`) and their serde vocabulary
//! (`strata_model`) live elsewhere; this is only the Freya wiring.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_io::Timer;
use freya::prelude::{
    spawn, use_provide_context, use_side_effect, use_state, Platform, State, TaskHandle,
    WritableUtils,
};
use freya::radio::{use_init_radio_station, use_radio, use_radio_station, RadioStation};
use strata_core::engine::TableSpec;
use strata_core::project::{self as project_io, SessionLoadError};
use strata_model::WindowGeom;

use crate::apps::project::contexts::EngineCtx;
use crate::state::ConfigStation;

use super::catalog::{
    claim_scan, use_init_catalog_rescan, use_init_catalog_scan, CatalogRescan, ScanGuard,
};
use super::history::{History, HistoryCtx};
use super::{Chan, ProjChan, ProjectState, SessionState};

/// Initialise this window's Session store and provide it via context. Pulls the open
/// project's root from the [`ProjectState`] store already in context (`use_init_project`
/// runs first in the window root) and restores its `.strata/session.json` (state-arch §5);
/// with no project on disk, no session file, or an unparseable one, falls back to a single
/// blank tab — a session file that can't be *read* is the one case that fails the open
/// instead (see [`restore_or_new`]). Call once in the window root; returns the station for
/// the root to read / drive.
pub fn use_init_session() -> RadioStation<SessionState, Chan> {
    let project = use_radio_station::<ProjectState, ProjChan>();
    let root = project.peek().root.clone();
    use_init_radio_station::<SessionState, Chan>(move || restore_or_new(root))
}

/// Restore the persisted session for `root`, falling back to one blank tab. A **corrupt**
/// session file is kept beside itself and yields a blank session rather than bricking the
/// window; a session file that could not be **read** fails the open loud.
///
/// The two arms of [`SessionLoadError`] are the whole point of the type, and they are not
/// interchangeable:
///
/// * [`Corrupt`](SessionLoadError::Corrupt) — the bytes were read and are not a session.
///   Re-reading can only say the same, so this is *recoverable*, and it is the state a kill
///   mid-autosave produces: refusing to open would brick the project permanently. The tabs
///   still aren't thrown away — the file is moved aside **before** the first autosave can
///   overwrite it, and if it can't be moved we fail rather than overwrite it (see
///   [`keep_corrupt_session`]).
/// * [`Unreadable`](SessionLoadError::Unreadable) — the read itself failed (permission
///   denied, EIO, a network mount that went away). The contents are unknown and very
///   probably *intact*, so nothing here may touch the file — and opening blank isn't a
///   harmless middle ground either, because the autosave that follows would flatten a
///   perfectly good session into one empty tab a few hundred milliseconds later. That is
///   the silent-destruction case the project's standing rule exists to prevent, so this arm
///   takes the other half of that rule: **fail loud on unrecoverable**, like a project root
///   that won't canonicalize ([`resolve_launch_root`]) or defs that won't load
///   ([`open_project`]). Interim shape, same as those two: the eventual handling is to close
///   the window and surface the fault (P4-01/P4-02/P4-13); until that plumbing exists the
///   panic keeps the fault loud instead of papering over it with a blank session that
///   destroys the real one.
fn restore_or_new(root: PathBuf) -> SessionState {
    // Missing is the expected case (`Ok(None)`): a fresh project with no session yet.
    let restored = match project_io::load_session(&root) {
        Ok(snapshot) => snapshot,
        Err(e @ SessionLoadError::Corrupt(_)) => {
            tracing::error!("load session: {e}");
            keep_corrupt_session(&root)
                .unwrap_or_else(|e| panic!("open project `{}`: {e}", root.display()));
            None
        }
        Err(e @ SessionLoadError::Unreadable(_)) => {
            panic!("open project `{}`: load session: {e}", root.display())
        }
    }
    .and_then(SessionState::from_snapshot);

    restored.unwrap_or_else(new_session)
}

/// Move an unparseable `session.json` aside to `session.json.corrupt`, so the autosave that
/// follows this open can't overwrite the only copy of the user's tabs — they stay
/// recoverable by hand. `Err` when it could not be moved.
///
/// The destination comes from [`project_io::corrupt_session_path`] rather than being
/// suffixed here: `.strata/`'s layout is that module's to own, and it has to keep the same
/// name out of the project's `.gitignore` (a gitignore line matches literally, so the
/// `session.json` entry does not cover this one).
///
/// An existing `session.json.corrupt` is **replaced** — `fs::rename` overwrites its
/// destination, which is the behaviour that file's one fixed name is chosen for: the
/// *current* corruption is the one worth keeping, not a numbered museum of old ones.
///
/// A failed rename is an error rather than a logged shrug, because the corrupt bytes are
/// still the user's only copy of their tabs (a truncated `session.json` usually holds most
/// of their SQL verbatim) and they are still sitting at the path autosave is about to
/// overwrite. Opening anyway would destroy them; the caller fails the open instead.
fn keep_corrupt_session(root: &Path) -> Result<(), String> {
    let path = project_io::session_path(root);
    let kept = project_io::corrupt_session_path(root);
    match fs::rename(&path, &kept) {
        Ok(()) => {
            tracing::error!("kept the unparseable session at {}", kept.display());
            Ok(())
        }
        Err(e) => Err(format!(
            "could not keep the unparseable session {} at {}: {e}",
            path.display(),
            kept.display()
        )),
    }
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
/// drive engine registration of its defs (tables, then views). Also provides the window's
/// [`CatalogScan`](super::catalog::CatalogScan) flag and [`CatalogRescan`] counter, since
/// this is where the scans run. Call once in the window root, after the engine is in
/// context.
///
/// The open itself is synchronous (one small JSON read, needed before anything can
/// render meaningfully); registration is IO-heavy (schema inference reads file footers)
/// and runs as a spawned task, landing results row by row through [`ProjChan::Tables`] /
/// [`ProjChan::Views`] so rows flip `Loading → Ready/Failed` as answers arrive.
///
/// **This is the window's one scan driver** — project open and every ↻ come through the same
/// effect, so there is a single place that claims the flag and spawns the pass. The ↻ can't
/// spawn its own (see [`ScanRequest`](super::catalog::ScanRequest)): a task spawned from an
/// event handler belongs to that handler's scope, and collapsing the sidebar mid-scan would
/// cancel a pass the whole catalog is waiting on. Spawned from here it belongs to the window
/// root — the same scope that owns `ProjectState`, which is what the pass writes.
///
/// The effect's mount run *is* the project-open pass; every later run is a ↻ (it subscribes
/// to the request counter and nothing else — reading the scan flag here would re-fire the
/// effect on the flag's own release and loop).
pub fn use_init_project(engine: &EngineCtx, root: PathBuf) -> RadioStation<ProjectState, ProjChan> {
    let station = use_init_radio_station::<ProjectState, ProjChan>(move || open_project(root));
    let scan = use_init_catalog_scan();
    let rescan = use_init_catalog_rescan();
    let engine = engine.clone();
    use_side_effect(move || {
        let requested = rescan.read().0;
        // Claimed synchronously here, before anything is spawned, so two requests in one
        // executor tick can't both get through (see `claim_scan`). The guard released by
        // `Drop` — not by the pass's last statement — is what keeps a cancelled pass from
        // latching the flag (see `ScanGuard`).
        let Some(guard) = claim_scan(scan) else {
            return;
        };
        // Rows drop to `Loading` so the pane reads as re-scanning rather than as settled
        // data. Only for a real ↻: at mount every row is already `Loading`, and writing
        // them again would wake the catalog's subscribers for nothing.
        let mut station = station;
        if requested > 0 {
            station.write_channel(ProjChan::Tables).reload_tables();
            station.write_channel(ProjChan::Views).reload_views();
        }
        spawn(scan_catalog(engine.clone(), station, guard));
    });
    station
}

/// The sidebar's ↻ (P3-03): ask for a re-scan of the whole catalog — re-infer every table's
/// schema from its def, then re-create every view over what that found. Bumps the window's
/// [`CatalogRescan`] counter; the driver in [`use_init_project`] runs the pass.
///
/// **Re-scan is re-registration**, from the defs, not a walk of what the engine happens to
/// hold. `Engine::register` deregisters and rebuilds each table from a re-`infer_schema`d
/// config (see `catalog::register_external`), which is the same re-infer a walk of the live
/// providers would do — and, because the def is the input, it *also* retries a table whose
/// first registration failed. That is the case the button most needs to serve: the user fixes
/// a path or restores a file and presses ↻. A live-provider walk can't do it at all, since a
/// failed row has no provider to rebuild from.
///
/// A no-op while a scan is already running: the button is disabled for the duration, and the
/// driver's claim guards the rest.
///
/// A re-scan that *fails* leaves the table deregistered (`register_external` deregisters before
/// it infers), which is load-time semantics and the honest outcome: the files really are
/// unreadable now, the row says `Failed`, and any view over it fails its own re-create rather
/// than quietly answering from the provider that no longer matches the disk.
///
/// Only the inferred *schema* is refreshed. File sets, row counts and partition values are
/// already live: we run no `ListFilesCache`, so DataFusion re-`LIST`s per scan.
pub fn refresh_catalog(mut rescan: CatalogRescan) {
    rescan.write().0 += 1;
}

/// One catalog scan. The project-open pass and every ↻ are the same pass, so neither can run
/// while the other is in flight.
///
/// The [`ScanGuard`] is **owned by this future** and never touched: that is the release
/// mechanism. Settling drops it, and so does cancelling — a `set(false)` written after the
/// `.await` would only run on the first of those.
async fn scan_catalog(
    engine: EngineCtx,
    station: RadioStation<ProjectState, ProjChan>,
    _scan: ScanGuard,
) {
    register_defs(engine, station).await;
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

/// Register the project's defs on the engine: every table (relative sources resolved
/// against the project folder), then every view. One pass, shared by project open and the
/// sidebar's ↻ re-scan ([`refresh_catalog`]) — a re-scan *is* a re-registration, so there is
/// one implementation of "make the engine match the defs", not two that can drift.
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
/// store already in context (like [`use_init_session`]), and the kept-runs cap from the
/// app-global `Settings::max_history` — the same setting the in-memory window is trimmed to
/// on every record. Call once in the window root.
///
/// `config` is handed in rather than consumed from context, like the theme derivation's
/// station: this runs at the window root, where the global is right there.
pub fn use_init_history(config: ConfigStation) -> HistoryCtx {
    let project = use_radio_station::<ProjectState, ProjChan>();
    let root = project.peek().root.clone();
    let cap = super::history::history_cap(config);
    use_provide_context(move || State::create(History::load(&root, cap)))
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

#[cfg(test)]
mod tests {
    use std::env;
    use std::process;

    use super::*;

    /// A scratch project folder with a `.strata/`, unique per test (they run in threads of
    /// one process, so the pid alone wouldn't separate them).
    fn scratch(what: &str) -> PathBuf {
        let root = env::temp_dir().join(format!("strata-session-{what}-{}", process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(project_io::strata_dir(&root)).unwrap();
        root
    }

    /// A corrupt `session.json` opens a blank window **and** keeps the file. Both halves
    /// matter: opening at all (the previous panic made a project killed mid-autosave
    /// permanently unopenable — the case `SessionLoadError::Corrupt` exists for), and
    /// keeping the bytes, since the autosave that follows this open would otherwise
    /// overwrite the only copy of the user's tabs.
    #[test]
    fn a_corrupt_session_is_kept_aside_and_opens_blank() {
        let root = scratch("corrupt");
        let path = project_io::session_path(&root);
        fs::write(&path, "{ not json").unwrap();

        let restored = restore_or_new(root.clone());

        assert_eq!(restored.order.len(), 1, "one blank scratch tab");
        assert!(!path.exists(), "the unparseable file is out of the way");
        let kept = fs::read_to_string(project_io::corrupt_session_path(&root))
            .expect("kept beside itself");
        assert_eq!(kept, "{ not json", "…verbatim, for hand recovery");

        let _ = fs::remove_dir_all(&root);
    }

    /// A second corruption replaces the first. One fixed name is the deliberate choice
    /// (`corrupt_session_path`): the corruption the user is about to go looking for is the
    /// one that just happened.
    #[test]
    fn keeping_a_corrupt_session_replaces_an_older_one() {
        let root = scratch("corrupt-twice");
        let kept = project_io::corrupt_session_path(&root);
        fs::write(&kept, "the previous corruption").unwrap();
        fs::write(project_io::session_path(&root), "{ newer garbage").unwrap();

        restore_or_new(root.clone());

        assert_eq!(fs::read_to_string(&kept).unwrap(), "{ newer garbage");

        let _ = fs::remove_dir_all(&root);
    }

    /// If the corrupt file can't be moved out of autosave's way, the open **fails** rather
    /// than proceeding to overwrite it — a truncated `session.json` still holds most of the
    /// user's SQL verbatim, and this open's first autosave would land right on top of it.
    /// (A directory at the destination is the portable way to make `rename` fail.)
    #[test]
    #[should_panic(expected = "could not keep the unparseable session")]
    fn a_corrupt_session_that_cannot_be_kept_fails_the_open() {
        let root = scratch("corrupt-unkeepable");
        fs::write(project_io::session_path(&root), "{ not json").unwrap();
        let blocker = project_io::corrupt_session_path(&root);
        fs::create_dir_all(blocker.join("occupied")).unwrap();

        restore_or_new(root);
    }

    /// A session file that couldn't be **read** is the opposite case: its bytes are unknown
    /// and probably fine, so the open fails loud instead of opening blank — the blank
    /// session's own autosave is what would destroy it. Nothing is moved aside.
    /// (A directory at `session.json` is the portable way to make `read` fail.)
    #[test]
    #[should_panic(expected = "unreadable")]
    fn an_unreadable_session_fails_the_open() {
        let root = scratch("unreadable");
        fs::create_dir_all(project_io::session_path(&root)).unwrap();

        restore_or_new(root);
    }

    /// …and it really is left alone: the same setup, checked after the panic is caught.
    #[test]
    fn an_unreadable_session_is_never_moved_aside() {
        let root = scratch("unreadable-untouched");
        let path = project_io::session_path(&root);
        fs::create_dir_all(&path).unwrap();

        let opened = std::panic::catch_unwind(|| restore_or_new(root.clone()));

        assert!(opened.is_err(), "the open fails rather than opening blank");
        assert!(path.is_dir(), "the unreadable path is untouched");
        assert!(
            !project_io::corrupt_session_path(&root).exists(),
            "and nothing was set aside"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The expected case is untouched by all that: no file at all is `Ok(None)`, a fresh
    /// project, and nothing is written aside.
    #[test]
    fn a_missing_session_is_not_treated_as_corruption() {
        let root = scratch("fresh");

        let restored = restore_or_new(root.clone());

        assert_eq!(restored.order.len(), 1);
        assert!(!project_io::corrupt_session_path(&root).exists());

        let _ = fs::remove_dir_all(&root);
    }
}
