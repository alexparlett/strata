//! The **service directory**: which project windows exist right now, from the server's
//! thread — and the app's [`Host`] impl over them.
//!
//! This is the one registry in the app that AGENTS.md §4's no-registry rule does *not*
//! govern, and the distinction is worth stating rather than assuming. That rule is about
//! **reactive UI state**: a `State<HashMap<TabId, …>>` threading every tab's data through one
//! value into every consumer, where context or a prop already expresses the relationship. This
//! is a **DI seam between threads** — the server has no scope, no context and no render pass,
//! so a directory is the only shape a lookup can take. It is the [`Windows`] registry's shape
//! (`platform::windows`) for the same reason, one thread further out.
//!
//! What a window lends it is exactly two things: its `Arc<Engine>` (the **data plane** — reads
//! that are engine-scoped and side-effect free go straight there) and one `mpsc::Sender`
//! (the **control plane** — everything that touches Radio state travels it as an
//! [`AgentAsk`]). Registration is per *mount* of the project subtree, not per window, which is
//! what makes a re-root and an engine restart deregister and re-register through the same
//! mount/drop path rather than needing a cleanup route of their own.
//!
//! [`Windows`]: crate::platform::Windows

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use strata_agent::{
    AgentError, CatalogEntry, Described, Host, Project, RunMode, RunSettle, TabInfo,
};
use strata_core::engine::Engine;
use strata_model::TabId;
use tokio::sync::{mpsc, oneshot};

use super::ask::AgentAsk;

/// How many asks may be queued for one window before a tool call waits its turn.
///
/// The driver is serial and never awaits a run's settle (it parks the reply), so this is a
/// burst buffer rather than a backlog — a bound at all only because an unbounded queue would
/// let a client that never reads its answers grow the window's memory without limit. A caller
/// that fills it simply waits, which is the honest backpressure.
const ASK_QUEUE: usize = 16;

/// One registration's identity, so a drop can only ever remove *its own* entry.
///
/// A counter rather than the project root, which looks like it would do: an engine restart
/// remounts the subtree at the **same** root, and although Freya orders the outgoing scope's
/// drop before the incoming mount, keying on the root would make that ordering load-bearing
/// for correctness rather than merely for tidiness — and the failure if it ever changed is
/// silent (the agent stops seeing a project that is right there).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RegId(u64);

/// One project window's lending: what it is called, where it lives, and the two channels the
/// server reaches it through.
struct Window {
    id: RegId,
    root: PathBuf,
    name: String,
    engine: Arc<Engine>,
    asks: mpsc::Sender<AgentAsk>,
}

/// Every project window an agent can address, plus the one app-wide number a tool needs
/// (`run`'s default page size).
///
/// Created once in `main` and shared by the server and every window (`super::AgentCtx`).
#[derive(Default)]
pub struct AgentDirectory {
    windows: Mutex<Vec<Window>>,
    /// Mirrors `Settings::row_limit` — see [`Host::default_page_size`], which is sync
    /// precisely because it must never become a question a window has to be awake to answer.
    page_size: AtomicUsize,
    next: AtomicU64,
}

impl AgentDirectory {
    /// Lend this mount's project to the server, and take back the id that ends the loan.
    pub fn register(
        &self,
        root: PathBuf,
        name: String,
        engine: Arc<Engine>,
    ) -> (RegId, mpsc::Receiver<AgentAsk>) {
        let (asks, rx) = mpsc::channel(ASK_QUEUE);
        let id = RegId(self.next.fetch_add(1, Ordering::Relaxed));
        self.windows.lock().unwrap().push(Window {
            id,
            root,
            name,
            engine,
            asks,
        });
        (id, rx)
    }

    /// End the loan. Idempotent, and matched on [`RegId`] rather than on the root.
    pub fn deregister(&self, id: RegId) {
        self.windows.lock().unwrap().retain(|w| w.id != id);
    }

    /// Mirror the app's default row limit — the number [`Host::default_page_size`] answers
    /// with. Written by the same effect that starts and stops the server, so a change in
    /// Settings lands without restarting anything.
    pub fn set_default_page_size(&self, rows: usize) {
        self.page_size.store(rows, Ordering::Relaxed);
    }

    /// The ask channel for `project`, or [`AgentError::WindowGone`].
    ///
    /// Deliberately clones the sender out from under the lock rather than handing back a
    /// borrow: the caller is about to `await`, and a `MutexGuard` held across an await is
    /// both a `!Send` future and a lock the UI thread could then block on.
    fn asks(&self, project: &Path) -> Result<mpsc::Sender<AgentAsk>, AgentError> {
        self.windows
            .lock()
            .unwrap()
            .iter()
            .find(|w| w.root == project)
            .map(|w| w.asks.clone())
            .ok_or(AgentError::WindowGone)
    }

    /// Put one question to `project` and wait for its answer.
    ///
    /// Every way this can fail is the same fact — the window went — and there are three of
    /// them: it was never in the directory, the driver's receiver has already dropped, or the
    /// reply channel dropped with the scope that held it. A re-root and a close are
    /// indistinguishable here and should be: what the agent needs to know is that the window
    /// it was addressing is not the one in front of the user any more.
    async fn ask<T>(
        &self,
        project: &Path,
        build: impl FnOnce(oneshot::Sender<T>) -> AgentAsk,
    ) -> Result<T, AgentError> {
        let asks = self.asks(project)?;
        let (tx, rx) = oneshot::channel();
        asks.send(build(tx))
            .await
            .map_err(|_| AgentError::WindowGone)?;
        rx.await.map_err(|_| AgentError::WindowGone)
    }
}

impl Host for AgentDirectory {
    async fn projects(&self) -> Vec<Project> {
        self.windows
            .lock()
            .unwrap()
            .iter()
            .map(|w| Project {
                name: w.name.clone(),
                root: w.root.clone(),
            })
            .collect()
    }

    fn default_page_size(&self) -> usize {
        self.page_size.load(Ordering::Relaxed)
    }

    async fn engine(&self, project: &Path) -> Result<Arc<Engine>, AgentError> {
        self.windows
            .lock()
            .unwrap()
            .iter()
            .find(|w| w.root == project)
            .map(|w| Arc::clone(&w.engine))
            .ok_or(AgentError::WindowGone)
    }

    async fn catalog(&self, project: &Path) -> Result<Vec<CatalogEntry>, AgentError> {
        self.ask(project, AgentAsk::Catalog).await
    }

    async fn describe(&self, project: &Path, name: &str) -> Result<Described, AgentError> {
        let name = name.to_string();
        self.ask(project, |reply| AgentAsk::Describe { name, reply })
            .await?
    }

    async fn tabs(&self, project: &Path) -> Result<Vec<TabInfo>, AgentError> {
        self.ask(project, AgentAsk::Tabs).await
    }

    async fn open_tab(&self, project: &Path) -> Result<TabId, AgentError> {
        self.ask(project, AgentAsk::OpenTab).await
    }

    async fn close_tab(&self, project: &Path, tab: TabId) -> Result<(), AgentError> {
        self.ask(project, |reply| AgentAsk::CloseTab { tab, reply })
            .await?
    }

    async fn run(
        &self,
        project: &Path,
        tab: TabId,
        sql: String,
        mode: RunMode,
        page_size: usize,
    ) -> Result<RunSettle, AgentError> {
        self.ask(project, |reply| AgentAsk::Run {
            tab,
            sql,
            mode,
            page_size,
            reply,
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use futures::executor::block_on;
    use futures::future::join;
    use strata_agent::TabState;

    use super::*;

    /// The directory's futures are executor-agnostic — that is the whole property the bridge
    /// rests on — so a plain `block_on` stands in for both the server's Tokio runtime and
    /// Freya's UI executor, and a hand-rolled responder stands in for the driver.
    fn window(directory: &AgentDirectory, name: &str, root: &str) -> mpsc::Receiver<AgentAsk> {
        let (_, rx) = directory.register(
            PathBuf::from(root),
            name.into(),
            Arc::new(Engine::new(BTreeMap::new())),
        );
        rx
    }

    #[test]
    fn an_ask_reaches_the_registered_window_and_its_answer_comes_back() {
        let directory = AgentDirectory::default();
        let mut rx = window(&directory, "sales", "/w/sales");
        let answered = TabId::new();

        let (opened, _) = block_on(join(directory.open_tab(Path::new("/w/sales")), async {
            let Some(AgentAsk::OpenTab(reply)) = rx.recv().await else {
                panic!("expected an open-tab ask");
            };
            let _ = reply.send(answered);
        }));

        assert_eq!(opened.unwrap(), answered);
    }

    /// A window that answers can still refuse: the ask's own `Result` is what carries "no such
    /// tab", as against the directory's "no such window".
    #[test]
    fn a_windows_own_refusal_travels_back_as_itself() {
        let directory = AgentDirectory::default();
        let mut rx = window(&directory, "sales", "/w/sales");
        let tab = TabId::new();

        let (closed, _) = block_on(join(
            directory.close_tab(Path::new("/w/sales"), tab),
            async {
                let Some(AgentAsk::CloseTab { reply, .. }) = rx.recv().await else {
                    panic!("expected a close-tab ask");
                };
                let _ = reply.send(Err(AgentError::NotFound("No open tab.".into())));
            },
        ));

        assert!(matches!(closed, Err(AgentError::NotFound(_))));
    }

    /// **Three ways to lose a window, one answer.** Never registered, deregistered (the
    /// `use_drop` a close or a re-root runs), and registered but with its driver gone — an
    /// agent needs to know the window it was addressing is not the one in front of the user,
    /// and which of the three it was is a distinction with nothing behind it.
    #[test]
    fn every_way_of_losing_a_window_answers_the_same_thing() {
        let directory = AgentDirectory::default();
        assert!(matches!(
            block_on(directory.catalog(Path::new("/w/nope"))),
            Err(AgentError::WindowGone)
        ));

        let (id, rx) = directory.register(
            PathBuf::from("/w/sales"),
            "sales".into(),
            Arc::new(Engine::new(BTreeMap::new())),
        );
        // The driver's end goes, the entry does not: a send into a closed channel is the
        // same fact one step later.
        drop(rx);
        assert!(matches!(
            block_on(directory.catalog(Path::new("/w/sales"))),
            Err(AgentError::WindowGone)
        ));

        directory.deregister(id);
        assert!(matches!(
            block_on(directory.tabs(Path::new("/w/sales"))),
            Err(AgentError::WindowGone)
        ));
        assert!(block_on(directory.projects()).is_empty());
    }

    /// A reply channel that drops without answering — the parked run whose window went while
    /// its query was in flight — is the same answer again, rather than a hang.
    #[test]
    fn a_reply_dropped_unanswered_is_a_window_gone_and_not_a_hang() {
        let directory = AgentDirectory::default();
        let mut rx = window(&directory, "sales", "/w/sales");

        let (settled, _) = block_on(join(
            directory.run(
                Path::new("/w/sales"),
                TabId::new(),
                "SELECT 1".into(),
                RunMode::Run,
                100,
            ),
            async {
                // Taken, parked, and then dropped with the scope that held it.
                drop(rx.recv().await);
            },
        ));

        assert!(matches!(settled, Err(AgentError::WindowGone)));
    }

    /// The root is the identity and the name is a label, which is what lets `host::resolve`
    /// report a colliding name rather than guess.
    #[test]
    fn projects_are_listed_by_root_and_name() {
        let directory = AgentDirectory::default();
        let _a = window(&directory, "data", "/a/data");
        let _b = window(&directory, "data", "/b/data");

        let listed = block_on(directory.projects());
        assert_eq!(
            listed.iter().map(|p| p.root.clone()).collect::<Vec<_>>(),
            vec![PathBuf::from("/a/data"), PathBuf::from("/b/data")]
        );
        assert!(listed.iter().all(|p| p.name == "data"));
    }

    /// `run`'s default page size is a number a `Host` answers **synchronously**, so it is
    /// mirrored here rather than asked of a window. Zero is the app's own "no limit" and is
    /// passed through untouched — resolving it is the tool layer's, once.
    #[test]
    fn the_default_page_size_is_mirrored_verbatim() {
        let directory = AgentDirectory::default();
        assert_eq!(directory.default_page_size(), 0);
        directory.set_default_page_size(250);
        assert_eq!(directory.default_page_size(), 250);
    }

    /// Nothing in the vocabulary reports a tab state the directory invents: `TabState` comes
    /// back from the window verbatim. Pinned here because the enum is the one shape a
    /// well-meaning "unknown" arm could be added to.
    #[test]
    fn a_tab_state_is_the_windows_answer_verbatim() {
        let directory = AgentDirectory::default();
        let mut rx = window(&directory, "sales", "/w/sales");
        let tab = TabId::new();

        let (listed, _) = block_on(join(directory.tabs(Path::new("/w/sales")), async {
            let Some(AgentAsk::Tabs(reply)) = rx.recv().await else {
                panic!("expected a tabs ask");
            };
            let _ = reply.send(vec![TabInfo {
                tab,
                title: "findings".into(),
                state: TabState::Running,
            }]);
        }));

        let listed = listed.unwrap();
        assert_eq!(listed[0].tab, tab);
        assert!(matches!(listed[0].state, TabState::Running));
    }
}
