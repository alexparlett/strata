//! The freya-query capabilities of the query round-trip (`docs/SNAPSHOT_SPEC.md` §6):
//!
//! - [`RunQuery`] — the **Run** (or Explain). Keyed by [`QuerySpec`], whose [`RunId`]
//!   nonce is the cache identity: a Run is an *action*, so every press executes (fresh
//!   nonce → fresh snapshot) and nothing else ever re-executes it (raw-SQL identity is
//!   never a cache key — same SQL ≠ same data).
//! - [`FetchSnapshotPage`] — a page read of one **immutable** snapshot. Keyed by
//!   [`PageSpec`] — `(snapshot, page, page_size, sort)` — which is sound to cache
//!   forever: a revisited page renders with zero engine traffic.
//!
//! Subscribe with `stale_time(Duration::MAX)` on both: freya-query re-runs *stale*
//! entries on resubscribe, and an uncontrolled re-execution would silently
//! re-materialize a snapshot out from under the cached pages. A Run's subscription is
//! always built through [`QuerySpec::query`] — a `Query`'s settings are part of its
//! **cache identity**, so a second call site constructing them by hand risks a silently
//! different entry, which is a duplicate execution. The default `clean_time` stands: a
//! superseded press's entry ages out five minutes after its last subscriber unmounts,
//! while the *current* request's entry is held live by its tab's keeper
//! (`views::keeper`) even when the tab is backgrounded.

use std::time::Duration;

use freya::query::{Captured, Query, QueryCapability};
use strata_arrow::plan::{as_explain, QueryPlan};
use strata_arrow::RecordBatch;
use strata_engine::{RunOutcome, RunTag, StatementReport};
use strata_model::{Cell, QueryOutput, SnapshotId};
use uuid::Uuid;

use crate::apps::project::contexts::EngineCtx;
use strata_model::TabId;

/// Rows per page for a Run's snapshot (page 1 rides in the Run's own `QueryOutput`; later
/// pages go through [`FetchSnapshotPage`]). Matches the Dioxus app's default.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// One Run press's identity — the cache key that makes a Run an action (§6). Fresh per
/// press; never derived from the SQL.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RunId(Uuid);

impl RunId {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl From<RunId> for RunTag {
    fn from(run: RunId) -> Self {
        RunTag(run.0.as_u128())
    }
}

/// What kind of execution a Run press asked for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum QueryMode {
    /// Execute + materialize a snapshot → rows.
    Run,
    /// `EXPLAIN [ANALYZE]` → a plan tree, no snapshot.
    Explain { analyze: bool },
}

/// One Run press: the tab it ran in, its nonce, and a snapshot of the editor text at
/// press time (editing after the press doesn't re-run — only a new press does).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct QuerySpec {
    pub tab: TabId,
    pub run: RunId,
    pub sql: String,
    pub mode: QueryMode,
    pub page_size: usize,
}

impl QuerySpec {
    /// The one way to subscribe this press: every `use_query` on a Run builds its `Query`
    /// here, because the settings below are part of the cache identity — a hand-built
    /// subscription with different settings would be a *different* entry, and mounting it
    /// would dispatch the same press a second time. `stale_time(MAX)` so a settled press
    /// never re-executes by itself (`SNAPSHOT_SPEC` §6); `clean_time` stays at the default
    /// so a superseded press's entry is garbage-collected once nothing subscribes it.
    pub fn query(&self, engine: &EngineCtx) -> Query<RunQuery> {
        Query::new(self.clone(), RunQuery(engine.captured())).stale_time(Duration::MAX)
    }
}

/// A settled Run: the snapshot handle + page 1 (`output`) and the page-1 batch (the
/// type-aware source for Copy/Export).
pub struct QueryPage {
    pub output: QueryOutput,
    pub batch: RecordBatch,
}

/// What a Run press settled to.
///
/// Three, not two, and the third is not a mode: `Explain` is something the *press* asked for,
/// while whether a Run produces rows or performs a statement is a property of what was typed
/// — the engine's router decides it (ED-02), and this enum carries its answer through.
pub enum QueryOutcome {
    Rows(QueryPage),
    Plan(QueryPlan),
    /// An intercepted statement's report — no rows and no snapshot handle, so the tab's
    /// previous result stays readable. The keeper folds its `StoreEffect` into the project
    /// (`state::statement`); the results pane renders the sentence.
    Statement(StatementReport),
}

/// The Run capability. The engine handle rides as [`Captured`] — invisible to cache
/// identity (`PartialEq` always-true, `Hash` no-op).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RunQuery(pub Captured<EngineCtx>);

impl QueryCapability for RunQuery {
    type Ok = QueryOutcome;
    type Err = String;
    type Keys = QuerySpec;

    async fn run(&self, spec: &QuerySpec) -> Result<QueryOutcome, String> {
        let engine = &self.0;
        match spec.mode {
            QueryMode::Run => engine
                .run(
                    spec.tab.into(),
                    spec.run.into(),
                    spec.sql.clone(),
                    spec.page_size,
                )
                .await
                .map(|outcome| match outcome {
                    RunOutcome::Rows(output, batch) => {
                        QueryOutcome::Rows(QueryPage { output, batch })
                    }
                    RunOutcome::Statement(report) => QueryOutcome::Statement(report),
                }),
            QueryMode::Explain { analyze } => engine
                .explain(
                    spec.tab.into(),
                    spec.run.into(),
                    as_explain(&spec.sql, analyze),
                )
                .await
                .map(QueryOutcome::Plan),
        }
    }
}

/// One page read of one immutable snapshot — the sound cache key for paging, sort (and
/// filter, when it lands): reads of a fixed set never go stale.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PageSpec {
    pub snapshot: SnapshotId,
    pub page: usize,
    pub page_size: usize,
    /// `(column name, ascending)` — an `ORDER BY` over the whole snapshot before the
    /// page window; `None` = snapshot order.
    pub sort: Option<(String, bool)>,
}

impl PageSpec {
    /// The one way to subscribe this page read — the same contract as [`QuerySpec::query`]:
    /// the settings are cache identity, so a second call site building them by hand would
    /// fork the entry into a duplicate fetch. `enabled` is the one legitimate per-site
    /// variable (a read stays disabled until its Run settles rows).
    pub fn query(&self, engine: &EngineCtx, enabled: bool) -> Query<FetchSnapshotPage> {
        Query::new(self.clone(), FetchSnapshotPage(engine.captured()))
            .stale_time(Duration::MAX)
            .enable(enabled)
    }
}

/// A settled page read: display rows + the page batch.
pub struct SnapshotPage {
    pub rows: Vec<Vec<Cell>>,
    pub batch: RecordBatch,
}

/// The page-read capability.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct FetchSnapshotPage(pub Captured<EngineCtx>);

impl QueryCapability for FetchSnapshotPage {
    type Ok = SnapshotPage;
    type Err = String;
    type Keys = PageSpec;

    async fn run(&self, spec: &PageSpec) -> Result<SnapshotPage, String> {
        self.0
            .fetch_page(spec.snapshot, spec.page, spec.page_size, spec.sort.clone())
            .await
            .map(|(rows, batch)| SnapshotPage { rows, batch })
    }
}

/// The round trip through the capability layer, driven headlessly: `block_on` stands in
/// for the UI executor (the engine's `JoinHandle`s are executor-agnostic — the same
/// await `use_query` performs).
#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use strata_engine::sql::Blocked;

    use super::*;

    const SQL: &str = "SELECT * FROM (VALUES (2, 'b'), (1, 'a'), (3, 'c')) AS t";

    fn spec(engine: &EngineCtx, mode: QueryMode) -> (RunQuery, QuerySpec) {
        (
            RunQuery(engine.captured()),
            QuerySpec {
                tab: TabId::new(),
                run: RunId::new(),
                sql: SQL.into(),
                mode,
                page_size: 2,
            },
        )
    }

    #[test]
    fn run_then_page_through_the_capabilities() {
        let engine = EngineCtx::default();
        let (run, spec) = spec(&engine, QueryMode::Run);

        let QueryOutcome::Rows(page) = block_on(run.run(&spec)).expect("run") else {
            panic!("mode Run settles to rows");
        };
        assert_eq!(page.output.total, 3);
        assert_eq!(page.output.rows.len(), 2);
        let snapshot = page.output.snapshot.expect("snapshot handle");

        let pages = FetchSnapshotPage(engine.captured());
        let read = PageSpec {
            snapshot,
            page: 2,
            page_size: 2,
            sort: None,
        };
        let tail = block_on(pages.run(&read)).expect("page 2");
        assert_eq!(tail.rows.len(), 1);

        let sorted = PageSpec {
            snapshot,
            page: 1,
            page_size: 2,
            sort: Some(("column1".into(), false)),
        };
        let sorted = block_on(pages.run(&sorted)).expect("sorted page");
        assert_eq!(sorted.rows[0][0].text, "3");
    }

    /// A Run press now goes through the **router** (ED-02), so a statement the editor refuses
    /// settles the capability `Err` with the words the squiggle showed — not a DataFusion
    /// error about a policy that is ours, and not rows.
    #[test]
    fn a_refused_statement_settles_err_through_the_capability() {
        let engine = EngineCtx::default();
        let (run, mut spec) = spec(&engine, QueryMode::Run);
        spec.sql = "CREATE DATABASE d".into();

        let err = block_on(run.run(&spec)).err().expect("refused");
        assert_eq!(err, Blocked::CreateDatabase.editor_message());
    }

    #[test]
    fn explain_settles_to_a_plan() {
        let engine = EngineCtx::default();
        let (run, spec) = spec(&engine, QueryMode::Explain { analyze: false });
        let QueryOutcome::Plan(plan) = block_on(run.run(&spec)).expect("explain") else {
            panic!("mode Explain settles to a plan");
        };
        assert!(!plan.physical.is_empty());
    }
}
