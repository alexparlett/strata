//! The hooks the **window root** calls to stand up + persist per-window state: open the
//! project, restore the session, load history, and drive session autosave. The stores
//! themselves (`ProjectState`, `SessionState`, `History`) and their serde vocabulary
//! (`strata_model`) live elsewhere; this is only the Freya wiring.

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use async_io::Timer;
use freya::prelude::{
    spawn, spawn_forever, use_drop, use_hook, use_provide_context, use_side_effect, use_state,
    Platform, State, TaskHandle, WritableUtils,
};
use freya::radio::{use_init_radio_station, use_radio, use_radio_station, RadioStation};
use strata_core::project::{self as project_io, ProjectDefs, SessionLoadError};
use strata_core::util::{fmt_int, plural};
use strata_engine::register::{register_pass, table_spec, RegOutcome};
use strata_engine::{Connections, TableSpec};
use strata_model::{ConnectionDef, SessionSnapshot, WindowGeom};

use crate::apps::project::contexts::EngineCtx;
use crate::state::ConfigStation;
use crate::task::offload;

use super::catalog::{
    claim_scan, request_scan, use_init_catalog, use_init_catalog_rescan, CatalogRescan, ScanGuard,
    ScanScope,
};
use super::history::{History, HistoryCtx};
use super::log::{log_event, LogCtx, LogLevel};
use super::persist::{persisted_session, persisted_session_offloaded, use_report};
use super::{Chan, ProjChan, ProjectState, SessionState};

/// Initialise this window's Session store and provide it via context, from the snapshot
/// [`open_project`] already restored off disk (state-arch §5) — `None` opens one blank tab.
/// Takes the [`Loaded`] by `Rc` so the render-time cost is a pointer bump: the snapshot is
/// cloned **inside** the initializer, which runs once at mount, never on a re-render. Call
/// once in the window root; returns the station for the root to read / drive.
pub fn use_init_session(loaded: Rc<Loaded>) -> RadioStation<SessionState, Chan> {
    use_init_radio_station::<SessionState, Chan>(move || build_session(loaded.session.clone()))
}

/// Restore the persisted session for `root`. `Ok(None)` opens blank; `Err` means the window
/// cannot exist — the caller surfaces the fault and closes the window
/// ([`ProjectLoadFailed`](crate::apps::project::views::ProjectLoadFailed)), never opening a
/// blank session whose autosave would destroy the real one.
///
/// The two arms of [`SessionLoadError`] are the whole point of the type and are not
/// interchangeable:
///
/// * [`Corrupt`](SessionLoadError::Corrupt) — the bytes were read and are not a session, which a
///   kill mid-autosave produces. Re-reading can only say the same, so this is *recoverable* and
///   refusing to open would brick the project permanently. The tabs are still not thrown away: the
///   file is moved aside **before** the first autosave can overwrite it, and a move that fails
///   fails the load (see [`keep_corrupt_session`]).
/// * [`Unreadable`](SessionLoadError::Unreadable) — the read itself failed, so the contents are
///   unknown and probably *intact*. Nothing here may touch the file, and opening blank is not a
///   harmless middle ground either: the autosave that follows would flatten a good session into
///   one empty tab a few hundred milliseconds later. So this arm takes the other half of the
///   standing rule — **fail loud on unrecoverable**.
fn restore_session(root: &Path) -> Result<Option<SessionSnapshot>, String> {
    match project_io::load_session(root) {
        Ok(snapshot) => Ok(snapshot),
        Err(e @ SessionLoadError::Corrupt(_)) => {
            tracing::error!("load session: {e}");
            keep_corrupt_session(root)?;
            Ok(None)
        }
        Err(e @ SessionLoadError::Unreadable(_)) => Err(format!("load session: {e}")),
    }
}

/// Build the Session store from the restored snapshot, falling back to one blank tab (a
/// fresh project, or a corrupt session kept aside).
fn build_session(seed: Option<SessionSnapshot>) -> SessionState {
    seed.and_then(SessionState::from_snapshot)
        .unwrap_or_else(new_session)
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

/// Initialise this window's Project store from the `defs` [`open_project`] already loaded,
/// and drive engine registration of them (connections, then tables, then views). Also
/// provides the window's [`Catalog`](super::catalog::Catalog) state and [`CatalogRescan`]
/// counter, since this is where the scans run. Call once in the window root, after the
/// engine is in context.
///
/// Registration is IO-heavy (schema inference reads file footers)
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
///
/// `log` is the window's event log (P3-13): the open itself and every def the pass answers for
/// are recorded there. Handed in rather than consumed from context so the root reads as what it
/// is — the log is stood up first, precisely because this is its first writer.
pub fn use_init_project(
    engine: &EngineCtx,
    log: LogCtx,
    root: PathBuf,
    loaded: Rc<Loaded>,
) -> RadioStation<ProjectState, ProjChan> {
    let station = use_init_radio_station::<ProjectState, ProjChan>(move || {
        ProjectState::from_defs(loaded.defs.clone(), root)
    });
    let catalog = use_init_catalog();
    let rescan = use_init_catalog_rescan();
    let engine = engine.clone();
    use_hook(move || {
        let name = station.peek().name.clone();
        log_event(log, LogLevel::Info, format!("Opened project '{name}'"));
    });
    use_side_effect(move || {
        let request = rescan.read().clone();
        let Some(guard) = claim_scan(catalog) else {
            return;
        };
        let work = plan_scan(&station.peek(), &request.scope);
        if request.seq > 0 {
            reset_rows(station, &request.scope, &work.views);
        }
        spawn(scan_catalog(engine.clone(), station, log, guard, work));
    });
    station
}

/// What one scan will re-register, by def name — connections, then tables, then views **in
/// dependency order**, which is also the order the pass settles them in.
///
/// A struct and not the three `Vec<String>`s it holds: they are the same type, they travel
/// together through the driver and the fold, and positional arguments would let two of them
/// swap with nothing to notice.
struct ScanWork {
    /// connection names — how the project addresses one, and what the engine registers
    /// under. Not buckets: `s3://lake` and `gs://lake` are two connections sharing one.
    connections: Vec<String>,
    tables: Vec<String>,
    views: Vec<String>,
}

impl ScanWork {
    /// Nothing to do — the row a scoped request named went between the request and the
    /// driver serving it.
    fn none() -> Self {
        Self {
            connections: Vec::new(),
            tables: Vec::new(),
            views: Vec::new(),
        }
    }
}

/// A scan's work list. Read before any row is reset (see the driver).
///
/// The two scopes differ only in reach. `All` is every def. `Table` is the one row plus
/// [`ProjectState::views_to_refresh`] — the views that read it (transitively) and every view
/// currently failing, because re-registering a table does not re-plan the views above it: their
/// plans captured the old provider by `Arc` and would go on scanning the files the pass just
/// replaced, with the old schema.
///
/// **Connections belong to `All` only** (W7). A table's Refresh does not re-connect: the store
/// its bucket needs is already registered from the open, and re-resolving a credential chain
/// per row Refresh would put a network round trip behind a gesture that is about one table's
/// files. The case that needs a re-connect — the user fixes a region, or runs `aws sso login`
/// — is exactly what ↻ is for.
///
/// Takes the state rather than the station, because it only reads it — and because that is what
/// lets the name reconciliation below be pinned by a test that needs no window.
fn plan_scan(p: &ProjectState, scope: &ScanScope) -> ScanWork {
    match scope {
        ScanScope::All => ScanWork {
            connections: p.connections.iter().map(|c| c.def.named()).collect(),
            tables: p.tables.iter().map(|t| t.def.name.clone()).collect(),
            views: p.refresh_order(p.views.iter().map(|v| v.def.name.clone()).collect()),
        },
        ScanScope::Table(name) => match p
            .tables
            .iter()
            .find(|t| ProjectState::same_name(&t.def.name, name))
        {
            Some(row) => ScanWork {
                connections: Vec::new(),
                tables: vec![row.def.name.clone()],
                views: p.views_to_refresh(&row.def.name),
            },
            None => ScanWork::none(),
        },
    }
}

/// Drop the rows this pass will re-answer back to `Loading` — and only those. A row Refresh
/// leaves the rest of the catalog wearing the verdicts it already has, which is the whole
/// difference between asking about one table and re-scanning the project.
fn reset_rows(
    mut station: RadioStation<ProjectState, ProjChan>,
    scope: &ScanScope,
    views: &[String],
) {
    match scope {
        ScanScope::All => {
            station
                .write_channel(ProjChan::Connections)
                .reload_connections();
            station.write_channel(ProjChan::Tables).reload_tables();
            station.write_channel(ProjChan::Views).reload_views();
        }
        ScanScope::Table(name) => {
            station.write_channel(ProjChan::Tables).reload_table(name);
            if !views.is_empty() {
                let mut p = station.write_channel(ProjChan::Views);
                for view in views {
                    p.reload_view(view);
                }
            }
        }
    }
}

/// A catalog row's **Refresh table** (P3-06): ask for a pass over *one* table and the views it
/// would otherwise leave stale. Same request path as the ↻ below — the menu item can't spawn the
/// pass itself for the same reason the button can't, and rather more sharply: the item's own
/// scope is a `MenuButton` that the very same press closes, so a task spawned there is dropped
/// before it is ever polled, and the rows it just reset would spin forever.
pub fn refresh_table(rescan: CatalogRescan, name: String) {
    request_scan(rescan, ScanScope::Table(name));
}

/// Re-read one table's own facts and land them on its row — **not** a scan pass (ED-05).
///
/// What an `INSERT` needs, and deliberately much less than [`refresh_table`]. A re-scan
/// re-*registers*, which builds a fresh provider and leaves every view above it holding a stale
/// `Arc`. An append cannot make a view stale — the sink schema-checks before it writes, and the old
/// provider re-LISTs per scan anyway — so going through the pass would break the views and repair
/// them for nothing, re-infer a schema that could not have moved, and flash every affected row
/// through `Loading`. So: ask the engine what the row says now, and land it. No epoch bump either,
/// because the count moved rather than what any name resolves to.
///
/// `spawn_forever`, for [`drop_confirm`]'s reason: the answer belongs to the window's store, and
/// the tab whose statement asked for it may be closed before it arrives.
///
/// **And so the store is asked whether it is still there before it is written.** Root-scoped means
/// this task outlives the *project subtree*, which a re-root or an engine restart remounts
/// wholesale — the `EngineCtx` clone keeps the outgoing engine alive, so the call comes back to a
/// station whose owner has been dropped, and writing it then panics. A row count nobody is left to
/// show is simply dropped.
///
/// [`drop_confirm`]: crate::apps::project::views::DropConfirm
pub fn refresh_table_rows(
    engine: EngineCtx,
    mut project: RadioStation<ProjectState, ProjChan>,
    name: String,
) {
    spawn_forever(async move {
        let meta = engine.table_meta(name.clone()).await;
        if !project.is_alive() {
            return;
        }
        match meta {
            Ok(meta) => project
                .write_channel(ProjChan::Tables)
                .table_reread(&name, meta),
            Err(e) => tracing::error!("table meta '{name}': {e}"),
        }
    });
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
pub fn refresh_catalog(rescan: CatalogRescan) {
    request_scan(rescan, ScanScope::All);
}

/// One catalog scan over `tables` + `views` — the project-open pass, every ↻ and every row
/// Refresh are the same pass at different widths, so none of them can run while another is in
/// flight.
///
/// The [`ScanGuard`] is **owned by this future** and never touched: that is the release
/// mechanism. Settling drops it, and so does cancelling — a `set(false)` written after the
/// `.await` would only run on the first of those.
async fn scan_catalog(
    engine: EngineCtx,
    station: RadioStation<ProjectState, ProjChan>,
    log: LogCtx,
    _scan: ScanGuard,
    work: ScanWork,
) {
    register_defs(engine, station, log, work).await;
}

/// What the project subtree needs off disk before it can mount: the defs and the persisted
/// session, loaded together so a window either has everything or shows the fault. Serde
/// values, not stores — the stores are built by the init hooks' initializers, so they are
/// still only ever built full, never a rootless default. No derives: the window holds it
/// behind an `Rc` whose pointer is its identity (built once per mount).
pub struct Loaded {
    pub defs: ProjectDefs,
    pub session: Option<SessionSnapshot>,
}

/// Open the project folder `root`: load its `project.json` defs (scaffolding a fresh
/// `.strata/` when the folder has none), then restore its session ([`restore_session`]).
/// `Err` means the window cannot exist — the caller renders the fault and closes
/// ([`ProjectLoadFailed`](crate::apps::project::views::ProjectLoadFailed)); the error
/// strings already name the file with its path.
pub fn open_project(root: &Path) -> Result<Loaded, String> {
    let defs = if project_io::exists_at(root) {
        project_io::load_defs(root)
    } else {
        project_io::scaffold(root)
    }?;
    let session = restore_session(root)?;
    Ok(Loaded { defs, session })
}

/// [`open_project`] **off the render thread** — what `ProjectRoot`'s loading arm awaits, and
/// the only way the subtree runs it.
///
/// What it reads is unchanged; where it runs is the point. Every step of `open_project` is
/// synchronous `std::fs` (and one `fs::rename`, for a corrupt session), and Freya polls its
/// futures on the thread that draws every window — so run in the render pass, a project on a
/// mount that stopped answering froze the whole app, and the fault dialog's Try again was a
/// button that re-entered the freeze on demand. See [`offload`] for why a thread rather than a
/// pool, and for what "cancel" can and cannot mean here.
///
/// The `Rc` is taken on this side of the hop: [`Loaded`] crosses the thread as the plain serde
/// values it is, and the pointer whose identity `ProjectLoaded` compares is minted once the
/// answer is home.
pub async fn load_project(root: PathBuf) -> Result<Rc<Loaded>, String> {
    let named = root.clone();
    match offload(move || open_project(&root)).await {
        Some(Ok(loaded)) => Ok(Rc::new(loaded)),
        Some(Err(e)) => {
            tracing::error!("open project {}: {e}", named.display());
            Err(e)
        }
        None => {
            let e = format!(
                "open project {}: the load did not complete",
                named.display()
            );
            tracing::error!("{e}");
            Err(e)
        }
    }
}

/// Register the named defs on the engine and fold what it answered into the store. One
/// pass, shared by project open, the sidebar's ↻ re-scan ([`refresh_catalog`]) and a
/// row's Refresh ([`refresh_table`]) — a re-scan *is* a re-registration, so there is one
/// implementation of "make the engine match the defs", not several that can drift. The
/// three differ only in the work list they hand in. The engine-facing half — connections
/// first, then tables, then views by fixed-point rounds — is `strata-engine`'s
/// [`register_pass`] (AA-01, so a headless host runs the same sequence); this keeps what is
/// genuinely the store's: `Reg<T>` rows and log entries, folded per outcome as each settles.
///
/// `views` is taken **in order**, which the caller has already sorted so a view is
/// re-created after everything it reads ([`ProjectState::refresh_order`]). That ordering
/// is only knowable once the views have answered at least once; at project open none of
/// them have, which is what the pass's fixed-point retry is for.
///
/// Every answer the engine gives is also **recorded in the event log** (P3-13) — one event per def,
/// on either arm, for every width of pass. Not a synthesized "re-scanned N tables" summary: the
/// per-def answers are the facts the pass observed, and a count derived from them would be a second
/// derivation of the same thing. A pass whose work list is empty (a table dropped between the
/// request and the driver) records nothing, because nothing happened.
///
/// A name with no def is skipped — the row went while the pass was being planned.
async fn register_defs(
    engine: EngineCtx,
    mut station: RadioStation<ProjectState, ProjChan>,
    log: LogCtx,
    work: ScanWork,
) {
    let (connections, tables, views) = {
        let p = station.peek();
        let root = p.root.clone();
        let connections: Vec<ConnectionDef> = work
            .connections
            .into_iter()
            .filter_map(|name| {
                Some(
                    p.connections
                        .iter()
                        .find(|c| c.def.named() == name)?
                        .def
                        .clone(),
                )
            })
            .collect();
        let known = Connections::of(
            &p.connections
                .iter()
                .map(|c| c.def.clone())
                .collect::<Vec<_>>(),
        );
        let tables: Vec<TableSpec> = work
            .tables
            .into_iter()
            .filter_map(|name| {
                let def = &p.tables.iter().find(|t| t.def.name == name)?.def;
                Some(table_spec(&root, def, &known))
            })
            .collect();
        let views: Vec<(String, String)> = work
            .views
            .into_iter()
            .filter_map(|name| {
                let sql = p.views.iter().find(|v| v.def.name == name)?.def.sql.clone();
                Some((name, sql))
            })
            .collect();
        (connections, tables, views)
    };

    register_pass(
        &engine,
        connections,
        tables,
        views,
        |outcome| match outcome {
            RegOutcome::Connection { name, result } => match result {
                Ok(()) => {
                    log_event(log, LogLevel::Ok, format!("Connected '{name}'"));
                    station
                        .write_channel(ProjChan::Connections)
                        .connection_registered(&name);
                }
                Err(e) => {
                    tracing::error!("connect '{name}' failed: {e}");
                    log_event(
                        log,
                        LogLevel::Error,
                        format!("Connection '{name}' failed: {e}"),
                    );
                    station
                        .write_channel(ProjChan::Connections)
                        .connection_failed(&name, e);
                }
            },
            RegOutcome::Table { name, result } => match result {
                Ok(meta) => {
                    log_event(
                        log,
                        LogLevel::Ok,
                        format!(
                            "Registered table '{name}' · {}{}",
                            plural(meta.columns.len(), "column"),
                            meta.rows
                                .map(|rows| format!(" · {} rows", fmt_int(rows)))
                                .unwrap_or_default()
                        ),
                    );
                    station
                        .write_channel(ProjChan::Tables)
                        .table_registered(&name, meta);
                }
                Err(e) => {
                    tracing::error!("register table '{name}' failed: {e}");
                    log_event(
                        log,
                        LogLevel::Error,
                        format!("Table '{name}' failed to register: {e}"),
                    );
                    station
                        .write_channel(ProjChan::Tables)
                        .table_failed(&name, e);
                }
            },
            RegOutcome::View { name, result } => match result {
                Ok(meta) => {
                    log_event(
                        log,
                        LogLevel::Ok,
                        format!(
                            "Registered view '{name}' · {}",
                            plural(meta.columns.len(), "column")
                        ),
                    );
                    station
                        .write_channel(ProjChan::Views)
                        .view_registered(&name, meta);
                }
                Err(e) => {
                    tracing::error!("create view '{name}' failed: {e}");
                    log_event(
                        log,
                        LogLevel::Error,
                        format!("View '{name}' failed to register: {e}"),
                    );
                    station.write_channel(ProjChan::Views).view_failed(&name, e);
                }
            },
        },
    )
    .await;
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
/// `restored` is the geometry **the loaded session remembers**, taken from the session this project
/// actually loaded rather than from the read that placed the window — that read has a deadline and
/// may come back empty, and seeding `None` would let the first save replace a good remembered size
/// with the default the window opened at.
///
/// **Our fill, and fullscreen, are not remembered.** Either has the *screen's* geometry rather than
/// a size the user picked, so persisting it would reopen every later launch at screen size. While
/// the window is in either state the save keeps rewriting the last geometry it had in neither.
///
/// A window the **user** sized to fill the screen does persist, which is why the test is
/// `filled_by_app` and not `Platform::is_maximized` alone — that mirrors macOS's `isZoomed`, a
/// frame comparison, so a hand-tiled window reads as zoomed too. The `&& is_filled` keeps the pair
/// self-healing: leaving fill drops the guard either way.
///
/// The subscription is inside the effect's reactive scope, not the caller's render, so a keystroke
/// re-runs only this effect.
///
/// **And one last save on the way down.** The debounce is a task and a task dies with its scope, so
/// a project that goes away inside the debounce window would lose whatever was typed in it — both
/// ways it can go are ordinary, so the settled session is written once more from `use_drop`.
pub fn use_autosave(restored: Option<WindowGeom>, filled_by_app: State<bool>) {
    let subscribe = use_radio::<SessionState, Chan>(Chan::Persist);
    let session = use_radio_station::<SessionState, Chan>();
    let project = use_radio_station::<ProjectState, ProjChan>();
    let report = use_report();
    let platform = Platform::get();
    let root_size = platform.root_size;
    let window_position = platform.window_position;
    let is_filled = platform.is_maximized;
    let is_fullscreen = platform.is_fullscreen;
    let mut pending = use_state(|| None::<TaskHandle>);
    let mut armed = use_state(|| false);
    let mut dirty = use_state(|| false);
    let mut written = use_state(|| None::<SessionSnapshot>);
    let normal_geom = use_state(move || restored);

    use_side_effect(move || {
        let _ = subscribe.read().active;
        let _ = root_size.read();
        let _ = window_position.read();
        let _ = is_filled.read();
        let _ = is_fullscreen.read();
        let _ = filled_by_app.read();
        if !*armed.peek() {
            armed.set(true);
            return;
        }
        if !*dirty.peek() {
            dirty.set(true);
        }
        if let Some(task) = *pending.peek() {
            task.cancel();
        }
        let task = spawn(async move {
            Timer::after(AUTOSAVE_DEBOUNCE).await;
            let root = project.peek().root.clone();
            let mut snapshot = session.peek().snapshot();
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
            if written.peek().as_ref() == Some(&snapshot) {
                return;
            }
            if !persisted_session_offloaded(&root, &snapshot, report).await {
                return;
            }
            written.set(Some(snapshot));
        });
        pending.set(Some(task));
    });

    use_drop(move || {
        if !*dirty.peek() {
            return;
        }
        let root = project.peek().root.clone();
        let mut snapshot = session.peek().snapshot();
        snapshot.window = *normal_geom.peek();
        if written.peek().as_ref() == Some(&snapshot) {
            return;
        }
        persisted_session(&root, &snapshot, report);
    });
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::process;

    use strata_engine::TableMeta;
    use strata_model::{SourceFormat, TableDef, TableOrigin};

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
    /// matter: opening at all (a failed open would make a project killed mid-autosave
    /// permanently unopenable — the case `SessionLoadError::Corrupt` exists for), and
    /// keeping the bytes, since the autosave that follows this open would otherwise
    /// overwrite the only copy of the user's tabs.
    #[test]
    fn a_corrupt_session_is_kept_aside_and_opens_blank() {
        let root = scratch("corrupt");
        let path = project_io::session_path(&root);
        fs::write(&path, "{ not json").unwrap();

        let restored = restore_session(&root);

        assert!(
            matches!(restored, Ok(None)),
            "opens blank rather than failing"
        );
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

        restore_session(&root).unwrap();

        assert_eq!(fs::read_to_string(&kept).unwrap(), "{ newer garbage");

        let _ = fs::remove_dir_all(&root);
    }

    /// If the corrupt file can't be moved out of autosave's way, the open **fails** rather
    /// than proceeding to overwrite it — a truncated `session.json` still holds most of the
    /// user's SQL verbatim, and this open's first autosave would land right on top of it.
    /// (A directory at the destination is the portable way to make `rename` fail.)
    #[test]
    fn a_corrupt_session_that_cannot_be_kept_fails_the_open() {
        let root = scratch("corrupt-unkeepable");
        fs::write(project_io::session_path(&root), "{ not json").unwrap();
        let blocker = project_io::corrupt_session_path(&root);
        fs::create_dir_all(blocker.join("occupied")).unwrap();

        let err = restore_session(&root).err().expect("fails the open");

        assert!(
            err.contains("could not keep the unparseable session"),
            "{err}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// A session file that couldn't be **read** is the opposite case: its bytes are unknown
    /// and probably fine, so the open fails instead of opening blank — the blank session's
    /// own autosave is what would destroy it. Nothing is moved aside.
    /// (A directory at `session.json` is the portable way to make `read` fail.)
    #[test]
    fn an_unreadable_session_fails_the_open() {
        let root = scratch("unreadable");
        fs::create_dir_all(project_io::session_path(&root)).unwrap();

        let err = restore_session(&root).err().expect("fails the open");

        assert!(err.contains("unreadable"), "{err}");

        let _ = fs::remove_dir_all(&root);
    }

    /// …and it really is left alone: the same setup, with the file checked after the `Err`.
    #[test]
    fn an_unreadable_session_is_never_moved_aside() {
        let root = scratch("unreadable-untouched");
        let path = project_io::session_path(&root);
        fs::create_dir_all(&path).unwrap();

        let opened = restore_session(&root);

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

        let restored = restore_session(&root);

        assert!(matches!(restored, Ok(None)));
        assert!(!project_io::corrupt_session_path(&root).exists());

        let _ = fs::remove_dir_all(&root);
    }

    /// The blank fallback both `Ok(None)` arms share: one scratch tab, ready to type into.
    #[test]
    fn no_seed_opens_one_blank_tab() {
        assert_eq!(build_session(None).order.len(), 1, "one blank scratch tab");
    }

    /// The defs half fails the open the same way the session half does, naming the file —
    /// the coverage the old panic never had.
    #[test]
    fn unloadable_defs_fail_the_open() {
        let root = scratch("defs-garbage");
        fs::write(
            project_io::strata_dir(&root).join("project.json"),
            "{ not defs",
        )
        .unwrap();

        let err = open_project(&root).err().expect("fails the open");

        assert!(err.contains("project.json"), "{err}");

        let _ = fs::remove_dir_all(&root);
    }

    /// A failed defs load leaves the session file exactly where it is: the composite
    /// short-circuits before the session half runs, so a doubly-broken project keeps its
    /// corrupt `session.json` in place — not renamed aside — for the open that eventually
    /// succeeds. Pins the `?`-before-`restore_session` ordering, which nothing else does:
    /// a reorder would mutate the disk on an open that fails anyway.
    #[test]
    fn a_defs_failure_leaves_the_session_untouched() {
        let root = scratch("defs-first");
        fs::write(
            project_io::strata_dir(&root).join("project.json"),
            "{ not defs",
        )
        .unwrap();
        fs::write(project_io::session_path(&root), "{ not json").unwrap();

        let err = open_project(&root).err().expect("fails the open");

        assert!(err.contains("project.json"), "{err}");
        assert_eq!(
            fs::read_to_string(project_io::session_path(&root)).unwrap(),
            "{ not json",
            "the session file is untouched"
        );
        assert!(
            !project_io::corrupt_session_path(&root).exists(),
            "and nothing was set aside"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The composite's happy path: a folder with no project scaffolds one named after
    /// itself and opens with no session to restore.
    #[test]
    fn a_fresh_folder_scaffolds_and_opens() {
        let root = scratch("scaffold");

        let loaded = open_project(&root).unwrap();

        assert_eq!(
            loaded.defs.name,
            root.file_name().unwrap().to_string_lossy()
        );
        assert!(loaded.session.is_none(), "no session yet");
        assert!(project_io::exists_at(&root), "the scaffold wrote the defs");

        let _ = fs::remove_dir_all(&root);
    }

    /// **A re-scan request's name is not always a def's own spelling**, and the pass registers
    /// from defs — so it resolves the request case-insensitively and then carries the def's name.
    ///
    /// The engine answers under the *planner's* identity: `mytable` for a def saved as `MyTable`
    /// (pinned engine-side by `a_statement_names_a_table_as_the_planner_folds_it`). An exact match
    /// here plans an empty pass, which is a silent no-op — the row simply never refreshes.
    #[test]
    fn a_rescan_finds_its_row_whatever_case_the_request_names_it() {
        let mut p = ProjectState::from_defs(
            ProjectDefs {
                tables: vec![TableDef {
                    name: "MyTable".into(),
                    format: SourceFormat::Arrow,
                    connection: None,
                    sources: vec![".strata/tables/mytable/".into()],
                    partition_cols: Vec::new(),
                    origin: TableOrigin::Internal,
                }],
                ..Default::default()
            },
            PathBuf::from("/strata-plan-scan"),
        );
        p.table_registered(
            "MyTable",
            TableMeta {
                columns: Vec::new(),
                rows: Some(1),
            },
        );

        let work = plan_scan(&p, &ScanScope::Table("mytable".into()));

        assert_eq!(
            work.tables,
            vec!["MyTable".to_string()],
            "the folded request found the row, and the pass carries the def's name"
        );
        assert!(
            plan_scan(&p, &ScanScope::Table("gone".into()))
                .tables
                .is_empty(),
            "a name with no row at all is still an empty pass"
        );
    }
}
