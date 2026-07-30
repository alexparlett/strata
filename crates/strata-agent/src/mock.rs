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
//! is only what a host is: which projects exist, what the catalog says, and which tabs are
//! open. [`MockProject::settling`] is the one deliberate lever — it makes the next run settle
//! with an engine string of the test's choosing, which is how "the user cancelled this"
//! becomes assertable without racing a real cancel.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use strata_core::engine::plan::as_explain;
use strata_core::engine::{Engine, RunTag, WsId};
use strata_model::TabId;

use crate::error::AgentError;
use crate::host::{
    CatalogEntry, Described, Host, Project, RunMode, RunSettle, Settled, TabInfo, TabState,
};

/// One project the mock serves. Build it, register whatever tables the test needs on its
/// [`engine`](MockProject::engine), then hand it to [`MockHost::new`].
pub struct MockProject {
    pub name: String,
    pub root: PathBuf,
    pub engine: Arc<Engine>,
    pub catalog: Vec<CatalogEntry>,
    pub described: Vec<Described>,
    tabs: Vec<TabInfo>,
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
            tabs: Vec::new(),
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
    page_size: usize,
    runs: AtomicU64,
}

impl MockHost {
    pub fn new(projects: Vec<MockProject>) -> Arc<MockHost> {
        Arc::new(MockHost {
            projects: Mutex::new(projects),
            page_size: 100,
            runs: AtomicU64::new(0),
        })
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
        self.page_size
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

    async fn tabs(&self, project: &Path) -> Result<Vec<TabInfo>, AgentError> {
        self.with(project, |p| Ok(p.tabs.clone()))
    }

    async fn open_tab(&self, project: &Path) -> Result<TabId, AgentError> {
        self.with(project, |p| {
            let tab = TabId::new();
            p.tabs.push(TabInfo {
                tab,
                title: format!("Query {}", p.tabs.len() + 1),
                state: TabState::Empty,
            });
            Ok(tab)
        })
    }

    async fn close_tab(&self, project: &Path, tab: TabId) -> Result<(), AgentError> {
        self.with(project, |p| {
            match p.tabs.iter().position(|t| t.tab == tab) {
                Some(at) => {
                    p.tabs.remove(at);
                    Ok(())
                }
                None => Err(AgentError::NotFound(format!("No open tab '{}'.", tab.0))),
            }
        })
    }

    async fn run(
        &self,
        project: &Path,
        tab: TabId,
        sql: String,
        mode: RunMode,
        page_size: usize,
    ) -> Result<RunSettle, AgentError> {
        // Everything that needs the lock, taken before the await — a host answers UI-side
        // questions and then waits on the engine, never the other way round.
        let (engine, settle) = self.with(project, |p| {
            if !p.tabs.iter().any(|t| t.tab == tab) {
                return Err(AgentError::NotFound(format!("No open tab '{}'.", tab.0)));
            }
            Ok((Arc::clone(&p.engine), p.settle.clone()))
        })?;
        if let Some(error) = settle {
            return Ok(Err(error));
        }

        let ws = WsId::from(tab);
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
        let state = if settled.is_ok() {
            TabState::Settled
        } else {
            TabState::Empty
        };
        self.with(project, |p| {
            if let Some(t) = p.tabs.iter_mut().find(|t| t.tab == tab) {
                t.state = state;
            }
            Ok(())
        })?;
        Ok(settled)
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
