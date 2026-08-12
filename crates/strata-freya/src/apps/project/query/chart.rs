//! **The chart read** as a freya-query capability (Rz2, `docs/CHART_SPEC.md` §5) — the
//! results Chart body's data, shaped exactly like the grid's page read
//! ([`FetchSnapshotPage`](super::FetchSnapshotPage)) because it is the same tier of work: a
//! projected, capped read of one immutable local snapshot, with no confirm in front of it.
//!
//! ## What the key has to carry
//!
//! `(snapshot, query)` is not the whole identity. The axis labels come out of the engine's
//! **display config** — `datafusion.format.*`, which Settings ▸ Engine ▸ Properties changes on
//! a live engine with no restart and no new snapshot ([`Engine::chart`]'s own note) — so an
//! entry keyed on the pair alone would keep serving labels rendered under a format the user
//! has since changed. [`ChartSpec::display`] carries that subset, which makes the change a
//! *new entry* rather than a stale one, and keeps `stale_time(MAX)` honest: given a fixed
//! snapshot, a fixed request and a fixed display config, the answer never changes.
//!
//! The subset comes from the **app config** (the store `use_engine_config` drives the engine
//! from), not from the engine's own copy, because that is the reactive source: a window
//! re-renders on a settings write, and Freya's runner drains a write's dirty scopes before it
//! polls the tasks queued alongside them, so the driver's `set_config` has landed by the time
//! this capability runs.
//!
//! [`Engine::chart`]: strata_core::engine::Engine::chart

use std::collections::BTreeMap;
use std::time::Duration;

use freya::query::{Captured, Query, QueryCapability};
use strata_model::{ChartData, ChartQuery, SnapshotId, Trend};

use crate::apps::project::contexts::EngineCtx;

/// One chart read of one immutable snapshot: what to read, and the display config its labels
/// render through.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ChartSpec {
    pub snapshot: SnapshotId,
    pub query: ChartQuery,
    /// The engine's `datafusion.format.*` overrides — see the module note. Built with
    /// [`strata_core::engine::config::display_subset`], never assembled by hand.
    pub display: BTreeMap<String, String>,
}

impl ChartSpec {
    /// The one way to subscribe this read — the same contract as
    /// [`PageSpec::query`](super::PageSpec::query): the settings below are part of the cache
    /// identity, so a second call site building them by hand would fork the entry into a
    /// duplicate read. `enabled` is the one legitimate per-site variable (the read stays
    /// disabled until a Run has settled a snapshot to read).
    pub fn query(&self, engine: &EngineCtx, enabled: bool) -> Query<FetchChart> {
        Query::new(self.clone(), FetchChart(engine.captured()))
            .stale_time(Duration::MAX)
            .enable(enabled)
    }
}

/// The chart-read capability. The engine handle rides as [`Captured`] — invisible to cache
/// identity.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct FetchChart(pub Captured<EngineCtx>);

impl QueryCapability for FetchChart {
    type Ok = ChartData;
    type Err = String;
    type Keys = ChartSpec;

    async fn run(&self, spec: &ChartSpec) -> Result<ChartData, String> {
        self.0.chart(spec.snapshot, spec.query.clone()).await
    }
}

/// The scatter's **trendline read** (Chart 11): the least-squares fit over the same snapshot
/// [`ChartSpec`] reads, keyed by the two columns the scatter currently plots.
///
/// **Numbers only, so no display config in the key** — nothing here renders through
/// `datafusion.format.*`. And deliberately not an extension of [`ChartQuery`]: the fit is its
/// own entry, which is what makes toggling the overlay a repaint of data already in hand plus
/// one cheap aggregate, never a re-read of the points.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct TrendSpec {
    pub snapshot: SnapshotId,
    pub x: String,
    pub y: String,
}

impl TrendSpec {
    /// The one way to subscribe the fit — [`ChartSpec::query`]'s contract: `stale_time(MAX)`
    /// because a fixed snapshot and two fixed columns never answer differently, and `enabled`
    /// is the per-site variable (a scatter with the toggle on, over a settled result).
    pub fn query(&self, engine: &EngineCtx, enabled: bool) -> Query<FetchTrend> {
        Query::new(self.clone(), FetchTrend(engine.captured()))
            .stale_time(Duration::MAX)
            .enable(enabled)
    }
}

/// The trendline capability. `Ok(None)` is a fit the data cannot support — the overlay simply
/// does not draw.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct FetchTrend(pub Captured<EngineCtx>);

impl QueryCapability for FetchTrend {
    type Ok = Option<Trend>;
    type Err = String;
    type Keys = TrendSpec;

    async fn run(&self, spec: &TrendSpec) -> Result<Option<Trend>, String> {
        self.0
            .trend(spec.snapshot, spec.x.clone(), spec.y.clone())
            .await
    }
}

/// The round trip through the capability layer, driven headlessly — the same shape
/// [`run_query`](super::run_query)'s tests use (`block_on` stands in for the UI executor).
#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use strata_model::{ChartSeries, QueryOutput};

    use super::*;
    use crate::apps::project::query::{QueryMode, QueryOutcome, QuerySpec, RunId, RunQuery};
    use strata_model::TabId;

    /// A settled Run's snapshot, so the chart read has something real to read.
    fn snapshot(engine: &EngineCtx, sql: &str) -> QueryOutput {
        let run = RunQuery(engine.captured());
        let spec = QuerySpec {
            tab: TabId::new(),
            run: RunId::new(),
            sql: sql.into(),
            mode: QueryMode::Run,
            page_size: 10,
        };
        let QueryOutcome::Rows(page) = block_on(run.run(&spec)).expect("run") else {
            panic!("mode Run settles to rows");
        };
        page.output
    }

    #[test]
    fn a_chart_read_answers_the_run_s_own_rows_in_order() {
        let engine = EngineCtx::default();
        let output = snapshot(
            &engine,
            "SELECT * FROM (VALUES ('b', 2), ('a', 1), ('c', 3)) AS t(name, n)",
        );

        let charts = FetchChart(engine.captured());
        let spec = ChartSpec {
            snapshot: output.snapshot.expect("snapshot handle"),
            query: ChartQuery::Rows {
                x: Some("name".into()),
                ys: vec!["n".into()],
                series: None,
                cap: 1_000,
            },
            display: BTreeMap::new(),
        };
        let ChartData::Table { axis, series } = block_on(charts.run(&spec)).expect("chart") else {
            panic!("a Rows read answers a table");
        };
        // The order the query produced them in, not sorted (CHART_SPEC §1.6).
        assert_eq!(axis.labels, ["b", "a", "c"]);
        assert_eq!(
            series,
            [ChartSeries {
                name: "n".into(),
                values: vec![Some(2.), Some(1.), Some(3.)],
            }]
        );
    }

    /// The trendline rides the same round trip, keyed by its two columns alone.
    #[test]
    fn a_trend_read_answers_the_fit_over_the_run_s_snapshot() {
        let engine = EngineCtx::default();
        let output = snapshot(
            &engine,
            "SELECT * FROM (VALUES (1.0, 3.0), (2.0, 5.0), (3.0, 7.0)) AS t(x, y)",
        );

        let trends = FetchTrend(engine.captured());
        let spec = TrendSpec {
            snapshot: output.snapshot.expect("snapshot handle"),
            x: "x".into(),
            y: "y".into(),
        };
        let fit = block_on(trends.run(&spec))
            .expect("trend")
            .expect("three clean pairs fit a line");
        assert!((fit.slope - 2.).abs() < 1e-9, "{fit:?}");
        assert!((fit.intercept - 1.).abs() < 1e-9, "{fit:?}");
    }
}
