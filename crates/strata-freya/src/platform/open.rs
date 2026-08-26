//! **Which window an open lands in** — the [`OpenPref`] question, and the one path that
//! answers it.
//!
//! [`windows::open_project`] is *how* a project window comes up; this is *where*. The two
//! are deliberately apart, because the question only exists for a window that already has a
//! project: the launcher has nothing to displace (it stands down behind the project it
//! opened), so it keeps calling `open_project` directly.
//!
//! A **project** window routes every open — ⌘O, File ▸ Open…, File ▸ Open Recent, the header
//! switcher's rows — through [`OpenCtx`], which resolves the setting:
//!
//! * [`OpenPref::New`] — a window of its own ([`windows::open_project`]).
//! * [`OpenPref::This`] — **in place**: the window's project root is a `State` and its whole
//!   project subtree is keyed on that root, so setting it unmounts the old project (stores,
//!   engine, autosave, catalog) and stands the new one up exactly as launch does.
//! * [`OpenPref::Ask`] — the This/New prompt (`views::dialogs::OpenPrompt`), whose
//!   "Remember, don't ask again" writes the answer back as the pref.
//!
//! **A project already open in another window is focused, whatever the pref says.** Two
//! windows on one project would both autosave over the same `session.json`, so that rule
//! outranks the preference rather than being one of its outcomes — the same rule
//! [`windows::open_project`] has always applied before launching a window.
//!
//! **Opening in place asks before it destroys work.** The remount drops the outgoing
//! project's engine, which aborts every query executing in it — the same loss ⇧⌘W, the red
//! button and ⌘Q all stop and ask about. So [`OpenCtx::reroot`] goes through that very
//! dialog ([`CloseTarget::Reroot`]) rather than being the one destructive path that doesn't.
//!
//! Deciding and acting are split ([`OpenTarget`]) because the two callers hold different
//! handles: a window has a [`Platform`], while the menubar's event handler runs on the
//! renderer with a `RendererContext` and no `Platform` to get. One set of rules, two
//! executors.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use freya::prelude::*;
use freya::winit::window::WindowId;
use strata_core::config::OpenPref;

use crate::apps::project::{CloseGuard, CloseTarget, EngineRestart};
use crate::platform::windows;
use crate::state::{write_config, AppCtx, ConfigChan};

/// A project window's open path: the project it currently shows, and the This/New question
/// it is asking (if any).
///
/// Both are the window's own `State` slots — the window root owns them, provides this into
/// its tree for the header switcher, and parks it in [`FocusedOpen`] while the window is
/// focused so the menubar's Open Recent can reach it too. `Copy`, so every surface that can
/// trigger an open holds one by value.
#[derive(Clone, Copy, PartialEq)]
pub struct OpenCtx {
    /// The open project's folder. **Setting it re-roots the window** — see the module doc.
    pub root: State<PathBuf>,
    /// The folder a press picked, held while the prompt asks which window it belongs in.
    pub prompt: State<Option<PathBuf>>,
    /// This window's engine in-flight flag — the close-while-running gate's other half. In a
    /// `State` slot purely so this struct stays `Copy`, the same reason `TabCloser` holds its
    /// engine that way. It is the *window's* guard, handed over from engine to engine, so it
    /// stays correct across a re-root.
    pub guard: State<Arc<CloseGuard>>,
    /// The window's confirm-dialog slot, so a re-root that would abort running queries raises
    /// the same T2 question closing the window does.
    pub confirm: State<Option<CloseTarget>>,
    /// Whether the window currently shows the load-fault arm rather than an open project —
    /// set and cleared by the fault arm itself. It changes what "already showing it" means:
    /// naming this window's own project is normally a no-op, but on a faulted window the
    /// user plainly means "load it again", so [`apply`](Self::apply) retries instead.
    pub faulted: State<bool>,
    /// Whether the window's **project subtree is up** — set and cleared by the loaded arm
    /// itself, the same mount/drop shape as [`faulted`](Self::faulted) above.
    ///
    /// Not the complement of `faulted`: the loading arm is neither, and the distinction is the
    /// point. It exists because a handful of commands have their listeners *inside* the
    /// subtree (New Query and Save Query, in the workbench) while the window-level ones —
    /// Close Project, Open…, Settings… — are mounted in every arm. The menubar is built in the
    /// window root, above the subtree, so without this it cannot tell the two apart and would
    /// offer New Query on a window whose project failed to load (`menu::MenuScope`).
    pub loaded: State<bool>,
    /// The window's engine generation, which is how a retry re-runs the load: the bump
    /// remounts the keyed project subtree — the same mechanism the fault dialog's own Try
    /// again uses.
    pub restart: EngineRestart,
}

/// Where an open lands, once [`OpenCtx::decide`] has applied the rules. Returned rather than
/// acted on, so each caller carries it out with the window handle it actually has.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum OpenTarget {
    /// Nothing to do — this window already shows that project.
    Nothing,
    /// Another window already has it: focus that one.
    Focus(WindowId),
    /// Open it in a window of its own.
    NewWindow(PathBuf),
    /// Re-root this window in place ([`OpenCtx::reroot`]).
    ThisWindow(PathBuf),
    /// Raise the This/New prompt for it ([`OpenCtx::ask`]).
    Ask(PathBuf),
}

impl OpenCtx {
    /// Resolve where `root` should open. Pure — see the module doc on why acting is the
    /// caller's.
    pub fn decide(self, app: &AppCtx, root: PathBuf) -> OpenTarget {
        let current = self.root.peek();
        let already_open = app.windows.peek().project(&root.to_string_lossy());
        let pref = app.config.peek().settings.open_pref;
        decide_target(&current, already_open, pref, root)
    }

    /// Carry out a decision with a window's [`Platform`] handle — the in-window executor.
    pub fn apply(self, platform: Platform, app: AppCtx, target: OpenTarget) {
        match target {
            OpenTarget::Nothing => {
                if *self.faulted.peek() {
                    self.restart.restart();
                }
            }
            OpenTarget::Focus(id) => platform.focus_window(Some(id)),
            OpenTarget::NewWindow(root) => {
                spawn_forever(windows::open_project(platform, app, root));
            }
            OpenTarget::ThisWindow(root) => self.reroot(&app, root),
            OpenTarget::Ask(root) => self.ask(root),
        }
    }

    /// Decide and act, for a caller inside a window — the common path.
    pub fn request(self, platform: Platform, app: AppCtx, root: PathBuf) {
        let target = self.decide(&app, root);
        self.apply(platform, app, target);
    }

    /// Pick a project folder and route it — ⌘O / File ▸ Open… / the switcher's **Open…**,
    /// from a window that already has a project. `spawn_forever` for the reason in
    /// [`OpenCtx::apply`].
    pub fn pick(self, platform: Platform, app: AppCtx) {
        let pick = windows::pick_project_folder(&app);
        spawn_forever(async move {
            if let Some(root) = pick.await {
                self.request(platform, app, root);
            }
        });
    }

    /// Answer the prompt — its two actions. `remember` writes the answer back as the pref,
    /// so the question isn't asked again.
    pub fn choose(self, platform: Platform, app: AppCtx, new: bool, remember: bool) {
        let target = self.prompt.peek().clone();
        self.dismiss();
        let Some(root) = target else {
            return;
        };
        if remember {
            let pref = if new { OpenPref::New } else { OpenPref::This };
            write_config(app.config, &[ConfigChan::Settings], |cfg| {
                cfg.settings.open_pref = pref;
            });
        }
        let target = if new {
            OpenTarget::NewWindow(root)
        } else {
            OpenTarget::ThisWindow(root)
        };
        self.apply(platform, app, target);
    }

    /// Raise the This/New prompt for `root`.
    pub fn ask(self, root: PathBuf) {
        let mut prompt = self.prompt;
        prompt.set(Some(root));
    }

    /// Dismiss the prompt without opening anything — Cancel, Esc, the backdrop.
    pub fn dismiss(self) {
        let mut prompt = self.prompt;
        prompt.set(None);
    }

    /// Open in **this** window — asking first when it would destroy work in flight.
    ///
    /// The remount drops the outgoing project's engine, and `Engine::drop` aborts everything
    /// executing in it. That is the same loss the window's own close paths (⇧⌘W, the red
    /// button, ⌘Q) stop and ask about, so this goes through the very same dialog rather than
    /// being the one destructive action that doesn't — [`CloseTarget::Reroot`] carries the
    /// folder, and answering it calls [`reroot_confirmed`](Self::reroot_confirmed).
    pub fn reroot(self, app: &AppCtx, root: PathBuf) {
        let running = self.guard.peek().running();
        if running && app.config.peek().settings.confirm_close_running {
            let mut confirm = self.confirm;
            confirm.set(Some(CloseTarget::Reroot(root)));
        } else {
            self.reroot_confirmed(root);
        }
    }

    /// Swap the root the window's project subtree is keyed on — the re-root itself, once
    /// nothing is left to ask. Everything else follows from the remount: the outgoing
    /// project's stores, engine and autosave are dropped (its session flushed on the way out,
    /// its entry dropped from the open-set) and the new project stands up exactly as it does
    /// at launch.
    pub fn reroot_confirmed(self, root: PathBuf) {
        let mut current = self.root;
        current.set(root);
    }
}

/// The rule itself, over plain values: what `root` resolves to, given the project the window
/// already shows, the window (if any) that already has `root`, and the preference. Split out
/// from [`OpenCtx::decide`] so the two rules that **outrank** the preference are testable
/// without a window to hold the state.
fn decide_target(
    current: &Path,
    already_open: Option<WindowId>,
    pref: OpenPref,
    root: PathBuf,
) -> OpenTarget {
    if root == current {
        return OpenTarget::Nothing;
    }
    if let Some(id) = already_open {
        return OpenTarget::Focus(id);
    }
    match pref {
        OpenPref::Ask => OpenTarget::Ask(root),
        OpenPref::This => OpenTarget::ThisWindow(root),
        OpenPref::New => OpenTarget::NewWindow(root),
    }
}

/// The **focused** project window's open path, for the one menubar item that carries data
/// rather than synthesizing a chord.
///
/// File ▸ Open… reaches the focused window through the keyboard pipeline
/// (`send_key_press`), like every other menu command; **Open Recent** can't — it carries a
/// path. So the focused window parks its [`OpenCtx`] here, exactly as it points the File
/// menu's recents and Close Project at itself ([`use_file_menu`](crate::menu::use_file_menu)).
/// The launcher parks `None`: it has no project to open *into*, so a recent there opens a
/// window and the launcher stands down, as it always did.
pub type FocusedOpen = State<Option<OpenCtx>>;

/// Create the slot. Call **once**, in `main`, before `launch` — not a hook.
pub fn create_global_open() -> FocusedOpen {
    State::create_global(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sales() -> PathBuf {
        PathBuf::from("/data/sales")
    }
    fn features() -> PathBuf {
        PathBuf::from("/data/ml_features")
    }
    const EVERY_PREF: [OpenPref; 3] = [OpenPref::Ask, OpenPref::This, OpenPref::New];

    /// The project the window already shows is a no-op under **every** preference: opening
    /// it "in this window" would tear the project down and stand the same one back up, and
    /// "in a new window" would give one project two windows fighting over its session.
    #[test]
    fn the_window_s_own_project_is_a_no_op() {
        for pref in EVERY_PREF {
            assert_eq!(
                decide_target(&sales(), None, pref, sales()),
                OpenTarget::Nothing
            );
        }
    }

    /// A project another window already has is focused under every preference, for the same
    /// reason: two windows on one project would both autosave over its `session.json`.
    #[test]
    fn a_project_that_already_has_a_window_is_focused() {
        let id = WindowId::from(7u64);
        for pref in EVERY_PREF {
            assert_eq!(
                decide_target(&sales(), Some(id), pref, features()),
                OpenTarget::Focus(id)
            );
        }
    }

    /// Only once neither of those applies does the preference get to decide.
    #[test]
    fn otherwise_the_preference_decides() {
        let decide = |pref| decide_target(&sales(), None, pref, features());
        assert_eq!(decide(OpenPref::Ask), OpenTarget::Ask(features()));
        assert_eq!(decide(OpenPref::This), OpenTarget::ThisWindow(features()));
        assert_eq!(decide(OpenPref::New), OpenTarget::NewWindow(features()));
    }
}
