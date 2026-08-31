//! A [`Host`] over plain values and a real [`Engine`] — the vocabulary's test rig, and the
//! executable statement of what a host owes it.
//!
//! Public rather than `#[cfg(test)]` because the integration test speaks real MCP over the real
//! transport from `tests/`, where a `cfg(test)` item is invisible.
//!
//! The engine is **real**: a mock project registers actual tables and `run` actually executes, so
//! the happy path a test asserts is the one the engine produces. What is faked is only what a host
//! is — which projects exist, what the catalog says, and which query sessions are open for whom.
//! [`MockProject::settling`] is the one deliberate lever, making the next run settle with an engine
//! string of the test's choosing.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use strata_arrow::plan::as_explain;
use strata_engine::{Engine, EngineError, RunRows, RunTag, WsId};

use crate::error::AgentError;
use crate::host::{
    Agent, AgentId, CatalogEntry, Described, Host, Project, QuerySessionId, QuerySessionInfo,
    QuerySessionState, RunMode, RunSettle, Settled,
};

/// One query session the mock is holding, and whose it is.
struct MockSession {
    agent: AgentId,
    info: QuerySessionInfo,
}

/// One project the mock serves. Build it, register whatever tables the test needs on its
/// [`engine`](MockProject::engine), then hand it to [`MockHost::new`].
pub struct MockProject {
    /// The project's name.
    pub name: String,
    /// Its root, which is its identity.
    pub root: PathBuf,
    /// The engine it serves from.
    pub engine: Arc<Engine>,
    /// The catalog rows `list_tables` answers with.
    pub catalog: Vec<CatalogEntry>,
    /// What `describe_table` answers for, by name.
    pub described: Vec<Described>,
    sessions: Vec<MockSession>,
    settle: Option<EngineError>,
}

impl MockProject {
    /// A project with its own engine and an empty catalog.
    ///
    /// The engine is given the project directory, because every host that opens one does. A mock
    /// that skipped it would leave the engine's owned-storage fence with no `.strata/` to fence,
    /// and `export_result` would read a path inside it as an ordinary folder.
    pub fn new(name: &str, root: impl Into<PathBuf>) -> MockProject {
        let root = root.into();
        let engine = Engine::builder().with_data_dir(&root).build();
        MockProject {
            name: name.into(),
            root,
            engine,
            catalog: Vec::new(),
            described: Vec::new(),
            sessions: Vec::new(),
            settle: None,
        }
    }

    /// The project with `entries` as its catalog.
    pub fn with_catalog(mut self, entries: Vec<CatalogEntry>) -> MockProject {
        self.catalog = entries;
        self
    }

    /// The project with `described` added to what it can describe.
    pub fn with_described(mut self, described: Described) -> MockProject {
        self.described.push(described);
        self
    }

    /// Make every run in this project settle with `error` instead of executing.
    pub fn settling(mut self, error: EngineError) -> MockProject {
        self.settle = Some(error);
        self
    }
}

/// A [`Host`] over a fixed set of [`MockProject`]s.
pub struct MockHost {
    projects: Mutex<Vec<MockProject>>,
    page_size: AtomicUsize,
    runs: AtomicU64,
    /// Every [`Agent`] this host was introduced to, in order — the record a test asserts the
    /// seam's own contract against. `open_query_session` is where a host first learns an agent
    /// exists, so it is also the only place it learns whether the agent is the app's own
    /// (`Agent::in_app`), and a host that dropped that would make the rule untestable.
    opened: Mutex<Vec<Agent>>,
}

impl MockHost {
    /// A host serving `projects`.
    pub fn new(projects: Vec<MockProject>) -> Arc<MockHost> {
        Arc::new(MockHost {
            projects: Mutex::new(projects),
            page_size: AtomicUsize::new(100),
            runs: AtomicU64::new(0),
            opened: Mutex::new(Vec::new()),
        })
    }

    /// Swap a project's engine, keeping its root — what an engine restart does to a project
    /// window (P4-07 remounts the subtree at the *same* folder). The lever a test needs to
    /// reach anything that remembers a per-engine id across a rebuild.
    pub fn replace_engine(&self, root: &Path, engine: Arc<Engine>) {
        if let Some(project) = self
            .projects
            .lock()
            .unwrap()
            .iter_mut()
            .find(|p| p.root == root)
        {
            engine.set_data_dir(root);
            project.engine = engine;
        }
    }

    /// The agents this host has been introduced to, oldest first.
    pub fn opened(&self) -> Vec<Agent> {
        self.opened.lock().unwrap().clone()
    }

    /// What [`Host::default_page_size`] answers. Settable because the real one tracks a live
    /// setting, `0` among its legal values ("no limit"), and that zero is the reading a host
    /// is most likely to get wrong.
    pub fn set_default_page_size(&self, rows: usize) {
        self.page_size.store(rows, Ordering::Relaxed);
    }

    /// Run one closure against a named project, or answer [`AgentError::WindowGone`] — which
    /// is what a host says when the project it was asked about is no longer there to answer.
    fn with<T>(
        &self,
        root: &Path,
        f: impl FnOnce(&mut MockProject) -> Result<T, AgentError>,
    ) -> Result<T, AgentError> {
        let mut projects = self.projects.lock().unwrap();
        match projects.iter_mut().find(|p| p.root == root) {
            Some(project) => f(project),
            None => Err(AgentError::WindowGone),
        }
    }
}

impl Host for MockHost {
    async fn projects(&self) -> Vec<Project> {
        self.projects
            .lock()
            .unwrap()
            .iter()
            .map(|p| Project {
                name: p.name.clone(),
                root: p.root.clone(),
            })
            .collect()
    }

    fn default_page_size(&self) -> usize {
        self.page_size.load(Ordering::Relaxed)
    }

    async fn engine(&self, project: &Path) -> Result<Arc<Engine>, AgentError> {
        self.with(project, |p| Ok(Arc::clone(&p.engine)))
    }

    async fn catalog(&self, project: &Path) -> Result<Vec<CatalogEntry>, AgentError> {
        self.with(project, |p| Ok(p.catalog.clone()))
    }

    async fn describe(&self, project: &Path, name: &str) -> Result<Described, AgentError> {
        self.with(project, |p| {
            p.described
                .iter()
                .find(|d| d.name().eq_ignore_ascii_case(name))
                .cloned()
                .ok_or_else(|| AgentError::NotFound(format!("Table or view '{name}' not found.")))
        })
    }

    /// **This agent's** sessions only — the contract's central scoping rule, kept here so a
    /// host that leaked another agent's work would fail the vocabulary's own tests.
    async fn query_sessions(
        &self,
        project: &Path,
        agent: AgentId,
    ) -> Result<Vec<QuerySessionInfo>, AgentError> {
        self.with(project, |p| {
            Ok(p.sessions
                .iter()
                .filter(|s| s.agent == agent)
                .map(|s| s.info.clone())
                .collect())
        })
    }

    async fn open_query_session(
        &self,
        project: &Path,
        agent: &Agent,
    ) -> Result<QuerySessionId, AgentError> {
        self.opened.lock().unwrap().push(agent.clone());
        self.with(project, |p| {
            let session = QuerySessionId::new();
            p.sessions.push(MockSession {
                agent: agent.id,
                info: QuerySessionInfo {
                    session,
                    state: QuerySessionState::Empty,
                },
            });
            Ok(session)
        })
    }

    async fn close_query_session(
        &self,
        project: &Path,
        agent: AgentId,
        session: QuerySessionId,
    ) -> Result<(), AgentError> {
        self.with(project, |p| {
            match p
                .sessions
                .iter()
                .position(|s| s.agent == agent && s.info.session == session)
            {
                Some(at) => {
                    p.sessions.remove(at);
                    Ok(())
                }
                None => Err(AgentError::no_such_query_session(session)),
            }
        })
    }

    async fn run(
        &self,
        project: &Path,
        agent: AgentId,
        session: QuerySessionId,
        sql: String,
        mode: RunMode,
        page_size: usize,
    ) -> Result<RunSettle, AgentError> {
        let (engine, settle) = self.with(project, |p| {
            if !p
                .sessions
                .iter()
                .any(|s| s.agent == agent && s.info.session == session)
            {
                return Err(AgentError::no_such_query_session(session));
            }
            Ok((Arc::clone(&p.engine), p.settle.clone()))
        })?;
        if let Some(error) = settle {
            return Ok(Err(error));
        }

        let ws = WsId::from(session);
        let run = RunTag(self.runs.fetch_add(1, Ordering::Relaxed) as u128);
        let settled = match mode {
            RunMode::Run => engine
                .ws(ws)
                .query(run, sql, page_size)
                .await
                .map(|RunRows { output, .. }| Settled::Rows(output)),
            RunMode::Explain => engine
                .ws(ws)
                .explain(run, as_explain(&sql, false))
                .await
                .map(Settled::Plan),
        };
        self.with(project, |p| {
            if let Some(s) = p
                .sessions
                .iter_mut()
                .find(|s| s.agent == agent && s.info.session == session)
            {
                s.info.state = QuerySessionState::Settled;
            }
            Ok(())
        })?;
        Ok(settled)
    }

    /// The connection ended, so its sessions go — in **every** project, since an agent may
    /// hold sessions in several and none of them outlives it.
    fn agent_gone(&self, agent: AgentId) {
        for project in self.projects.lock().unwrap().iter_mut() {
            project.sessions.retain(|s| s.agent != agent);
        }
    }
}
