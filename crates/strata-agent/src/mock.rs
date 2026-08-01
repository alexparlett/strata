//! A [`Host`] over plain values and a real [`Engine`] — the vocabulary's test rig, and the
//! executable statement of what a host owes it.
//!
//! Public rather than `#[cfg(test)]` for two reasons. The integration test speaks real MCP
//! over the real transport and lives in `tests/`, where a `cfg(test)` item is invisible; and
//! the two hosts that follow (AA-03's bridge, AA-05's headless) are written against this
//! contract, so having one worked example beside it is cheaper than reading the trait twice.
//!
//! The engine is **real**: a mock project registers actual tables and `run` actually
//! executes, so the happy path a test asserts is the one the engine produces. What is faked
//! is only what a host is: which projects exist, what the catalog says, and which query
//! sessions are open for whom. [`MockProject::settling`] is the one deliberate lever — it
//! makes the next run settle with an engine string of the test's choosing, which is how "the
//! user cancelled this" becomes assertable without racing a real cancel.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use strata_core::engine::plan::as_explain;
use strata_core::engine::{Engine, RunTag, WsId};

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
    pub name: String,
    pub root: PathBuf,
    pub engine: Arc<Engine>,
    pub catalog: Vec<CatalogEntry>,
    pub described: Vec<Described>,
    sessions: Vec<MockSession>,
    settle: Option<String>,
}

impl MockProject {
    /// A project with its own engine and an empty catalog.
    pub fn new(name: &str, root: impl Into<PathBuf>) -> MockProject {
        MockProject {
            name: name.into(),
            root: root.into(),
            engine: Arc::new(Engine::new(BTreeMap::new())),
            catalog: Vec::new(),
            described: Vec::new(),
            sessions: Vec::new(),
            settle: None,
        }
    }

    pub fn with_catalog(mut self, entries: Vec<CatalogEntry>) -> MockProject {
        self.catalog = entries;
        self
    }

    pub fn with_described(mut self, described: Described) -> MockProject {
        self.described.push(described);
        self
    }

    /// Make every run in this project settle with `error` instead of executing — the engine
    /// strings a cancel or a supersede produces.
    pub fn settling(mut self, error: &str) -> MockProject {
        self.settle = Some(error.into());
        self
    }
}

/// A [`Host`] over a fixed set of [`MockProject`]s.
pub struct MockHost {
    projects: Mutex<Vec<MockProject>>,
    page_size: AtomicUsize,
    runs: AtomicU64,
}

impl MockHost {
    pub fn new(projects: Vec<MockProject>) -> Arc<MockHost> {
        Arc::new(MockHost {
            projects: Mutex::new(projects),
            page_size: AtomicUsize::new(100),
            runs: AtomicU64::new(0),
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
            project.engine = engine;
        }
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
                .find(|d| described_name(d).eq_ignore_ascii_case(name))
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
        // Everything that needs the lock, taken before the await — a host answers its own
        // questions and then waits on the engine, never the other way round.
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
                .query(ws, run, sql, page_size)
                .await
                .map(|(output, _)| Settled::Rows(output)),
            // The host wraps, exactly as the app's Run capability does: `RunMode::Explain`
            // means "plan this statement", not "the caller already wrote EXPLAIN".
            RunMode::Explain => engine
                .explain(ws, run, as_explain(&sql, false))
                .await
                .map(Settled::Plan),
        };
        // Settled either way. A run that failed is still a run that finished — the dispatch
        // happened and the previous snapshot went with it. Reporting it as `Empty` would tell
        // an agent nothing had ever run there.
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

fn described_name(described: &Described) -> &str {
    match described {
        Described::Table { name, .. }
        | Described::View { name, .. }
        | Described::Failed { name, .. }
        | Described::Pending { name } => name,
    }
}
