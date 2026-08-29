//! The app's **window model**: which windows exist right now, the one path a project is
//! opened through, and the quit-vs-close split.
//!
//! **Two registers of "open", deliberately apart.** The config store's `open_projects` is
//! the *persisted* set — what "Reopen projects on startup" restores and what the launcher /
//! project switcher read to tell open from merely recent. [`Windows`] is the *live* one: the
//! winit [`WindowId`] behind each window, so "this project is already open" can be answered
//! with a focus instead of a second window. Window ids are process-local and meaningless on
//! disk, which is why they are not in `AppConfig`.
//!
//! **Quit is not close, and close is not quit** (RustRover's behaviour, which is what we're
//! after):
//!
//! - **Quit** (⌘Q · menu Quit · dock Quit) closes *every* window and leaves the persisted
//!   open-set alone, so the next launch reopens exactly what was on screen. [`begin_quit`]
//!   is what tells [`use_claim_open`] to keep its entry on the way out.
//! - **Close project** (red button · File ▸ Close Project · ⇧⌘W) closes *one* window and
//!   drops it from the open-set — and when it was the app's last window the **launcher**
//!   takes its place ([`close_this_window`]) rather than the app quitting. Closing every
//!   window by hand therefore means "start me at the launcher next time", which is exactly
//!   the distinction quitting must not make.
//!
//! [`use_claim_open`]: crate::state::use_claim_open

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use freya::prelude::*;
use freya::winit::window::WindowId;
use strata_core::project::STRATA_DIR;

use crate::apps::configure::ConfigureTarget;
use crate::apps::launcher::LauncherApp;
use crate::apps::project::{window_geometry, ProjectApp};
use crate::apps::source::SourceTarget;
use crate::menu::{use_file_menu, MenuScope};
use crate::state::{abandon_install, write_config, AppCtx, ConfigChan, ConfigStation};

/// What a window is showing. The project variant carries its folder (the
/// `RecentProject::path` string), which is how "is this project open?" is answered.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WindowKind {
    Launcher,
    Project(String),
    /// The Settings window — one app-wide, pinned above whichever window last asked for it
    /// ([`Windows::settings_owner`]). See [`crate::platform::settings`].
    Settings,
    /// An Export window, pinned above the project window that opened it.
    ///
    /// It names nothing, unlike the two below. Unlike Settings there is **no single-instance
    /// rule** — an export window is opened *on a result* and carries that result's facts, so
    /// "already open" can't mean "focus it": the open one is showing something else. Two exports
    /// of two different results at once is a reasonable thing to want, and each closes itself
    /// when its write lands. So the registry needs to know only that this window is not one the
    /// user *works* in ([`WindowKind::is_workspace`]); which window it belongs to is a launch
    /// value, held where it is read ([`crate::platform::owner`]) rather than copied here to go
    /// stale.
    Export,
    /// A Configure window, pinned above the project window that opened it.
    ///
    /// **One per target**, unlike Export: it is opened on a *def*, which is shared mutable
    /// state, so two windows on one def would both write it and the second would revert the
    /// first. Two different tables at once is fine, and so is one table in each of two
    /// projects — hence the owner beside the target, since one owner window shows one project
    /// (see [`crate::platform::configure`]).
    Configure {
        owner: WindowId,
        target: ConfigureTarget,
    },
    /// A data source editor, pinned above the project window that opened it.
    ///
    /// **One per target**, on Configure's terms and for its reason: it is opened on a *def*, so
    /// two windows on one data source would both `upsert_source` and both persist. The owner
    /// sits beside the target because one owner window shows one project, and two projects can
    /// each hold a data source to `s3://lake`.
    Source {
        owner: WindowId,
        target: SourceTarget,
    },
}

impl WindowKind {
    /// Whether this is a window the user *works* in — a project or the welcome screen.
    /// None of Settings, Export, Configure or the data source editor is: each is a panel over one
    /// of these, so it can neither be the app's last window nor keep the launcher from taking a
    /// closing project's place.
    fn is_workspace(&self) -> bool {
        !matches!(
            self,
            Self::Settings | Self::Export | Self::Configure { .. } | Self::Source { .. }
        )
    }
}

/// Every window this app has open, by id. Reactive: a window opening or closing anywhere
/// wakes whoever reads it (the project window mirrors "am I the last one?" into its close
/// guard from here).
#[derive(Default, Clone, PartialEq)]
pub struct Windows {
    by_id: HashMap<WindowId, WindowKind>,
    /// The window the Settings window is currently pinned above, while it is open. Kept
    /// beside the ids it names rather than in a global of its own: it is a fact about which
    /// live windows relate to which, which is exactly what this registry is.
    settings_owner: Option<WindowId>,
}

impl Windows {
    /// Whether the app is down to one window (or none — a window that hasn't yet resolved
    /// its id). What decides whether a close hands over to the launcher.
    ///
    /// Counts workspace windows only. The Settings window closes with its owner, so counting
    /// it would let the last project close onto an empty app: "not the last window" would be
    /// true right up to the moment both went.
    pub fn is_last(&self) -> bool {
        self.workspace_count() <= 1
    }

    /// How many windows the user *works* in are open. The panels are excluded for the same
    /// reason [`is_last`](Self::is_last) excludes them: each is a panel over one of these, so
    /// it is never somewhere to be sent and never the app's last window.
    pub fn workspace_count(&self) -> usize {
        self.by_id.values().filter(|k| k.is_workspace()).count()
    }

    /// The workspace window after `current`, wrapping — what `Command::CycleWindow` moves
    /// focus to. `None` when `current` is the only one, which is the command declining rather
    /// than a failure: there is nowhere to go, so the press falls through.
    ///
    /// Ordered by [`WindowId`], which is arbitrary but **stable** for a window's life — so
    /// cycling walks the same ring every time rather than reshuffling on each press, which a
    /// `HashMap`'s iteration order would do. It is deliberately not open-order: nothing records
    /// that, and inventing a second index to hold it would be a register to keep in step with
    /// this one for a tie-break nobody can perceive.
    pub fn cycle_from(&self, current: WindowId) -> Option<WindowId> {
        let mut ring: Vec<WindowId> = self
            .by_id
            .iter()
            .filter(|(_, kind)| kind.is_workspace())
            .map(|(id, _)| *id)
            .collect();
        ring.sort_unstable();
        let here = ring.iter().position(|id| *id == current)?;
        ring.get((here + 1) % ring.len())
            .copied()
            .filter(|id| *id != current)
    }

    /// The window showing the project rooted at `path`, if one is open.
    pub fn project(&self, path: &str) -> Option<WindowId> {
        self.by_id.iter().find_map(|(id, kind)| match kind {
            WindowKind::Project(root) if root == path => Some(*id),
            _ => None,
        })
    }

    /// The launcher window, if it is open (there is only ever one).
    pub fn launcher(&self) -> Option<WindowId> {
        self.by_id
            .iter()
            .find_map(|(id, kind)| matches!(kind, WindowKind::Launcher).then_some(*id))
    }

    /// The Settings window, if it is open — what makes it single-instance: a second ask
    /// focuses this one instead of opening another.
    pub fn settings(&self) -> Option<WindowId> {
        self.by_id
            .iter()
            .find_map(|(id, kind)| matches!(kind, WindowKind::Settings).then_some(*id))
    }

    /// The window Settings is pinned above, if it is open.
    pub fn settings_owner(&self) -> Option<WindowId> {
        self.settings_owner
    }

    /// Whether `id` still names a live window.
    pub fn is_open(&self, id: WindowId) -> bool {
        self.by_id.contains_key(&id)
    }

    /// Every live window by id — for the questions the named accessors above don't cover: which
    /// project a child window's owner is showing now ([`crate::platform::owner`]), and whether a
    /// Configure or data-source editor window is already open on a given owner's def.
    pub fn by_id(&self) -> &HashMap<WindowId, WindowKind> {
        &self.by_id
    }

    /// Record which window Settings is pinned above (`None` when it closes).
    pub fn pin_settings(&mut self, owner: Option<WindowId>) {
        self.settings_owner = owner;
    }
}

/// The app-global live window registry — created in `main`, handed to every window root.
/// Global for the same reason the config store is: "is this project already open?" and "am
/// I the last window?" are machine-global questions no single window can answer.
pub type WindowRegistry = State<Windows>;

/// Create the app-global registry. Call **once**, in `main`, before `launch`.
pub fn create_global_windows() -> WindowRegistry {
    State::create_global(Windows::default())
}

/// Set while a quit is in flight. Read on the UI thread (`use_claim_open`'s drop) and
/// written from both the UI and the renderer's menu handler — which are the same thread,
/// but an atomic says "shared flag, not window state" and costs nothing.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Mark the app as quitting, so the windows closing behind it keep their entries in the
/// persisted open-set.
pub fn begin_quit() {
    QUITTING.store(true, Ordering::Relaxed);
}

/// Abandon a quit that a window vetoed and the user then cancelled.
///
/// **This must be called from every path that dismisses the close confirm**, or the flag
/// latches: a cancelled quit would leave the app "quitting" for the rest of the session,
/// which silently inverts both behaviours the flag gates (the launcher would stop taking
/// over from the last window, and closed projects would stay in the persisted open-set and
/// reopen next launch).
///
/// It is also where an update install is abandoned (UP-02). The install press is a quit with an
/// intent recorded in front of it, so a cancelled quit has to forget that intent — and this is
/// already the one call every dismissing path makes, which is cheaper to keep true than a rule
/// each dialog remembers. Nothing is lost by it: the staged bundle is a file on disk and the
/// status still reads `Ready`.
pub fn end_quit() {
    QUITTING.store(false, Ordering::Relaxed);
    abandon_install();
}

/// Whether a quit is in flight (see [`begin_quit`]).
pub fn is_quitting() -> bool {
    QUITTING.load(Ordering::Relaxed)
}

/// Declare this window to the app for as long as it lives: the registry, under whatever `kind`
/// reports, and the **menubar**, under `menu`. Call once in a window root.
///
/// The two are one call because they are one obligation — "say what this window is" — and because
/// the menubar half is easy to forget and expensive to get wrong. Configure and Export were both
/// added without it, which left the menubar pointed at the project window underneath: ⇧⌘W closed
/// the focused *panel* while naming the project, and Open… sat enabled with no listener to reach.
///
/// A window learns its own id from the renderer, so the first insert lands a beat after mount; the
/// task is scope-bound, so a window that dies before its id arrives never registers one.
///
/// `kind` is read **reactively**, because a project window can be re-rooted in place and an entry
/// naming the old project would answer both of the registry's questions wrongly. The `MenuScope` is
/// not, for the same reason it need not be: a re-root swaps which project an `OpenCtx` points at,
/// never whether there is one.
///
/// Returns this window's id once the renderer has answered. Handed back rather than kept private
/// because `Command::CycleWindow` asks [`Windows::cycle_from`] for the window after *this* one.
pub fn use_register_window(
    app: &AppCtx,
    kind: impl Fn() -> WindowKind + 'static,
    menu: MenuScope,
) -> State<Option<WindowId>> {
    use_file_menu(app, menu);
    let mut windows = app.windows;
    let mut id = use_state(|| None::<WindowId>);
    use_hook(move || {
        let platform = Platform::get();
        spawn(async move {
            if let Ok(window_id) = platform.post_callback(|window_id, _| window_id).await {
                id.set(Some(window_id));
            }
        });
    });
    use_side_effect(move || {
        let kind = kind();
        debug_assert_eq!(
            kind.is_workspace(),
            menu.is_workspace(),
            "window kind and menu scope disagree about {kind:?}"
        );
        let Some(window_id) = *id.read() else {
            return;
        };
        windows.write().by_id.insert(window_id, kind);
    });
    use_drop(move || {
        if let Some(window_id) = *id.peek() {
            windows.write().by_id.remove(&window_id);
        }
    });
    id
}

/// Record a window we just opened, without waiting for it to render.
///
/// `launch_window` hands the [`WindowId`] back at creation, whereas a window's own
/// [`use_register_window`] can only learn it a render + round trip later. Registering here
/// closes that gap, which is what stops two closes in quick succession from each seeing
/// "no launcher open" and opening one apiece.
pub(super) fn register(mut windows: WindowRegistry, id: WindowId, kind: WindowKind) {
    windows.write().by_id.insert(id, kind);
}

/// Open `root` in a **new** project window. A project that already has a window is focused
/// rather than opened twice.
///
/// This is *how* a window comes up, not *where* an open lands: a window that already has a
/// project routes through [`OpenCtx`](crate::platform::OpenCtx) first, which may re-root that
/// window in place instead. The launcher calls straight here — it has nothing to displace.
///
/// Returns once the window exists, so a caller that opens a project *instead of* itself
/// (the launcher) can close only after there is something to close in favour of.
///
/// `platform` is passed rather than read here on purpose: the caller takes it in its
/// component scope, so this can be awaited from anywhere — including after a modal folder
/// picker, where there is no longer a scope to read it from.
pub async fn open_project(platform: Platform, app: AppCtx, root: PathBuf) {
    let path = root.to_string_lossy().into_owned();
    let windows = app.windows;
    let focus_if_open = || match windows.peek().project(&path) {
        Some(id) => {
            platform.focus_window(Some(id));
            true
        }
        None => false,
    };
    if focus_if_open() {
        return;
    }
    let geometry = window_geometry(root.clone()).await;
    if focus_if_open() {
        return;
    }
    let id = platform
        .launch_window(ProjectApp::window(app.clone(), root, geometry))
        .await;
    register(windows, id, WindowKind::Project(path));
}

/// Open the launcher — or focus it if it is already up. Single-instance by construction:
/// the registry knows whether a launcher window exists.
pub async fn open_launcher(platform: Platform, app: AppCtx) {
    if let Some(id) = app.windows.peek().launcher() {
        platform.focus_window(Some(id));
        return;
    }
    let windows = app.windows;
    let id = platform.launch_window(LauncherApp::window(app)).await;
    register(windows, id, WindowKind::Launcher);
}

/// Close **this** window the way the red button does: when it is the app's last, the
/// launcher takes its place first, so closing a project lands on the welcome window rather
/// than quitting the app. During a quit there is nothing to take its place — the whole app
/// is going.
///
/// Every deliberate close of a project window funnels here (the ⇧⌘W command, the confirm
/// dialog's "Stop & exit", and the OS close once its veto has handed control back to the
/// UI), so the launcher rule lives in one place. Uses `close_current_window`, which
/// bypasses the `on_close` veto — this *is* the decided close.
pub async fn close_this_window(platform: Platform, app: AppCtx) {
    if !is_quitting() && app.windows.peek().is_last() {
        open_launcher(platform.clone(), app).await;
    }
    platform.close_current_window();
}

/// The native folder picker, resolved to a project folder. `None` when the user cancels, or
/// when the folder can't be resolved (reported, not silent).
///
/// Deliberately the **async** dialog: the blocking one spins its own run loop, which would
/// freeze every other window while it is up. Starts in the configured default project
/// directory when one is set (Settings ▸ System).
pub fn pick_project_folder(app: &AppCtx) -> impl Future<Output = Option<PathBuf>> {
    let start_dir = app.config.peek().settings.default_project_dir.clone();
    async move {
        let mut dialog = rfd::AsyncFileDialog::new().set_title("Open project");
        if !start_dir.is_empty() {
            dialog = dialog.set_directory(&start_dir);
        }
        let handle = dialog.pick_folder().await?;
        resolve_project_folder(handle.path())
    }
}

/// Canonicalize a picked / stored path into the project folder to open: picking the
/// project's own `.strata` directory means the folder that holds it (the same normalization
/// the recents migration applies to paths written by the pre-Freya app). `None` — reported —
/// when it no longer resolves, so a moved project doesn't open a window that can't load one.
pub fn resolve_project_folder(picked: &Path) -> Option<PathBuf> {
    let folder = project_folder(picked);
    match std::fs::canonicalize(&folder) {
        Ok(root) => Some(root),
        Err(e) => {
            tracing::error!("open project `{}`: {e}", folder.display());
            None
        }
    }
}

/// [`resolve_project_folder`] for a **stored recent** — the recents surfaces' shared open
/// step (the launcher's rows, the header switcher, the menubar's Open Recent), which adds
/// the rule that a project no longer on disk forfeits its entry: the failed resolve is
/// what proves the row dead, so it is dropped from the recents (waking every list that
/// renders them) rather than left to fail identically forever. Startup prunes the same way
/// (`AppConfig::prune_missing`); this covers a project deleted while the app is running.
///
/// Only an entry whose folder is actually *gone* is dropped — a resolve that failed for
/// any other reason (permissions, say) keeps its row, with the failure reported by
/// [`resolve_project_folder`] as usual.
pub fn resolve_recent(config: ConfigStation, path: &str) -> Option<PathBuf> {
    let root = resolve_project_folder(Path::new(path));
    if root.is_none() && !Path::new(path).exists() {
        write_config(config, &[ConfigChan::Recents], |cfg| {
            cfg.remove_recent(path);
        });
    }
    root
}

/// The project folder a picked path names: the `.strata` directory's parent, or the path
/// itself. Split out from [`resolve_project_folder`] so the rule is testable without a real
/// folder on disk to canonicalize.
fn project_folder(picked: &Path) -> PathBuf {
    match (picked.file_name(), picked.parent()) {
        (Some(name), Some(parent)) if name == STRATA_DIR => parent.to_path_buf(),
        _ => picked.to_path_buf(),
    }
}

/// Quit from the UI (⌘Q): mark the quit, then ask every window to close. Each window's
/// `on_close` hook still gets its say, so a running query can still raise the T2 confirm.
pub fn quit() {
    let platform = Platform::get();
    begin_quit();
    drop(platform.post_callback(|_, ctx| quit_windows(ctx)));
}

/// Quit from the renderer thread (the menubar's Quit item, which already holds the
/// [`RendererContext`]). Same two steps as [`quit`], without the hop.
pub fn quit_windows(ctx: &mut RendererContext) {
    begin_quit();
    for id in ctx.windows.keys().copied().collect::<Vec<_>>() {
        ctx.request_close_window(Some(id));
    }
}

#[cfg(test)]
mod tests {
    use super::{project_folder, WindowKind, Windows};
    use freya::winit::window::WindowId;
    use std::path::{Path, PathBuf};

    /// A registry holding the given kinds, under ids that sort in the order written — so a
    /// test can say what the cycling ring should be and read it back. `WindowId: From<u64>` is
    /// winit's own escape hatch for exactly this; the ids are otherwise opaque.
    fn registry(kinds: [WindowKind; 4]) -> Windows {
        let mut windows = Windows::default();
        for (n, kind) in kinds.into_iter().enumerate() {
            windows.by_id.insert(id(n as u64), kind);
        }
        windows
    }

    fn id(n: u64) -> WindowId {
        WindowId::from(n)
    }

    #[test]
    fn cycling_walks_the_workspace_windows_and_skips_the_panels() {
        let windows = registry([
            WindowKind::Project("/a".into()),
            WindowKind::Settings,
            WindowKind::Project("/b".into()),
            WindowKind::Export,
        ]);
        assert_eq!(windows.workspace_count(), 2);
        assert_eq!(windows.cycle_from(id(0)), Some(id(2)));
        assert_eq!(windows.cycle_from(id(2)), Some(id(0)));
    }

    #[test]
    fn cycling_declines_when_there_is_nowhere_to_go() {
        let lone = registry([
            WindowKind::Project("/a".into()),
            WindowKind::Settings,
            WindowKind::Export,
            WindowKind::Export,
        ]);
        assert_eq!(lone.cycle_from(id(0)), None);
        assert_eq!(lone.cycle_from(id(9)), None);
    }

    #[test]
    fn picking_the_strata_dir_opens_its_project() {
        assert_eq!(
            project_folder(Path::new("/data/sales/.strata")),
            PathBuf::from("/data/sales")
        );
        assert_eq!(
            project_folder(Path::new("/data/sales")),
            PathBuf::from("/data/sales")
        );
        assert_eq!(
            project_folder(Path::new("/data/.strata/nested")),
            PathBuf::from("/data/.strata/nested")
        );
    }
}
