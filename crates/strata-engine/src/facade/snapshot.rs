//! Every read of one immutable snapshot.
//!
//! `docs/SNAPSHOT_SPEC.md` §5's growth rule lives here: a feature is a new read on this handle,
//! always a read of the fixed set. Nothing here re-runs the query and nothing mutates the
//! snapshot, which is what makes every read safely cacheable by its arguments.

use strata_model::{ChartData, ChartQuery, SnapshotId, Trend};

use crate::query::{self, CellFormat};
use crate::{
    chart, export, Engine, EngineError, ExportHold, SnapshotPage, SnapshotPin, StopReason,
};

/// The reads of one immutable snapshot, from [`Engine::snapshot`].
///
/// Addressed by id rather than by a settled result, because a snapshot outlives the workspace's
/// current text: an export belongs to a result, not to a tab.
#[derive(Clone, Copy)]
pub struct SnapshotReads<'a> {
    pub(super) engine: &'a Engine,
    pub(super) snapshot: SnapshotId,
}

impl SnapshotReads<'_> {
    /// Read one page — `sort` = `(column, ascending)` applied as an `ORDER BY` over the whole
    /// snapshot before the page window (Rz6). Reads are snapshot-scoped and side-effect free:
    /// safely cacheable by `(snapshot, page, page_size, sort)`.
    pub async fn page(
        self,
        page: usize,
        page_size: usize,
        sort: Option<(String, bool)>,
    ) -> Result<SnapshotPage, EngineError> {
        let snapshot = self.snapshot;
        let ctx = self.engine.ctx.clone();
        let fmt = CellFormat::new(&self.engine.overrides.lock().unwrap());
        let ord = self.engine.ordinal(snapshot);
        let (rows, batch) = self
            .engine
            .rt()
            .spawn(async move {
                query::fetch_page(&ctx, snapshot, page, page_size, sort, ord, &fmt).await
            })
            .await
            .map_err(|e| EngineError::task("page", e))??;
        Ok(SnapshotPage { rows, batch })
    }

    /// Read the snapshot as a chart (Rz2, `docs/CHART_SPEC.md` §5) — the
    /// renderer-first read `q` asks for: a projected, ordinal-ordered, capped read plus a
    /// long→wide pivot (`Rows`), raw points (`Raw`), or the one computed mark
    /// (`Histogram`). No aggregation, no bucketing, no imposed order — the withdrawn
    /// pipeline's grouped reads must not come back here.
    ///
    /// Snapshot-scoped and side-effect free like [`page`](Self::page). Cache
    /// identity is `(snapshot, q)` **plus the engine's display config**: axis labels render
    /// through the live `datafusion.format.*` overrides, which
    /// [`Engine::set_config`] changes without a restart — so a UI cache keyed on
    /// `(snapshot, q)` alone serves stale labels after a Settings change, and the chart
    /// surface must re-render (not merely re-key) when those overrides move, exactly as the
    /// grid's pages do. Deliberately no lifecycle bookkeeping and no confirm in front of it —
    /// a projected, capped read of a local snapshot is [`page`](Self::page)-tier work, not
    /// [`Catalog::profile`](crate::Catalog::profile)'s tier.
    ///
    /// The chart never re-reads the source files: it charts the result the grid is paging,
    /// which is what makes the two agree when the data underneath has since moved.
    pub async fn chart(self, q: ChartQuery) -> Result<ChartData, EngineError> {
        let _reading = self.pin();
        let snapshot = self.snapshot;
        let ctx = self.engine.ctx.clone();
        let fmt = CellFormat::new(&self.engine.overrides.lock().unwrap());
        let ord = self.engine.ordinal(snapshot);
        self.engine
            .rt()
            .spawn(async move { chart::run_chart(&ctx, snapshot, &q, &fmt, ord.as_deref()).await })
            .await
            .map_err(|e| EngineError::task("chart", e))?
            .map_err(EngineError::from)
    }

    /// The least-squares fit over the snapshot's finite `(x, y)` pairs (Chart 11) — the
    /// scatter's trendline, and the one computed *overlay* `docs/CHART_SPEC.md` §10
    /// sanctions. Engine-side because the overlay is a function of the encoding, not of the
    /// query: templating it into SQL would rewrite the user's query on every encoder gesture.
    ///
    /// [`chart`](Self::chart)'s tier exactly — snapshot-scoped, side-effect free, pinned
    /// for the length of the call — and deliberately **not** a [`ChartQuery`] arm, so a UI
    /// cache can key the fit by the two columns alone and toggling the overlay never
    /// re-reads the points. `Ok(None)` is a fit the data cannot support (fewer than two
    /// pairs, or no x-variance): the overlay simply does not draw, never an error the user
    /// must dismiss.
    pub async fn trend(self, x: String, y: String) -> Result<Option<Trend>, EngineError> {
        let _reading = self.pin();
        let snapshot = self.snapshot;
        let ctx = self.engine.ctx.clone();
        self.engine
            .rt()
            .spawn(async move { chart::run_trend(&ctx, snapshot, &x, &y).await })
            .await
            .map_err(|e| EngineError::task("trend", e))?
            .map_err(EngineError::from)
    }

    /// Write the snapshot to disk per `spec` — one file, or a Hive directory when the
    /// spec carries partition columns.
    ///
    /// **The snapshot is the source, not the SQL.** An export never re-runs the query: it
    /// streams the very table the grid is paging, in the sort the grid is showing, so the
    /// file matches what was on screen even if the underlying data has since moved. That is
    /// the whole reason snapshots exist (`docs/SNAPSHOT_SPEC.md`), and it is why this hangs
    /// off a [`SnapshotId`] rather than a workspace: an export belongs to a *result*, not to
    /// a tab, and the result outlives the tab's current text.
    ///
    /// Unlike [`Workspace::query`](crate::Workspace::query) there is no dispatch nonce and no
    /// supersede: two exports are two files, and neither invalidates the other. The
    /// bookkeeping is the in-flight count, which keeps the close confirm honest while a file
    /// is half-written, and a [`pin`](Self::pin) for the duration — so a re-run in the owning
    /// tab can't deregister the table this `COPY` is streaming. The export window holds a pin
    /// of its own for its whole life; this one makes the call correct on its own terms, for a
    /// caller that has no window.
    pub async fn export(
        self,
        spec: export::ExportSpec,
    ) -> Result<export::ExportReport, EngineError> {
        let snapshot = self.snapshot;
        let holding = ExportHold::new(self.engine, snapshot);
        let stats = self
            .engine
            .lifecycle
            .lock()
            .unwrap()
            .stats
            .get(&snapshot)
            .cloned()
            .unwrap_or_default();
        let task = {
            let ctx = self.engine.ctx.clone();
            self.engine.rt().spawn(async move {
                let _holding = holding;
                export::run_export(&ctx, snapshot, spec, &stats).await
            })
        };

        let joined = task.await;

        let (path, rows) = match joined {
            Ok(res) => res?,
            Err(join) if join.is_cancelled() => {
                return Err(EngineError::Stopped(StopReason::Cancelled))
            }
            Err(join) => return Err(EngineError::task("export", join)),
        };
        Ok(export::ExportReport::of(path, rows))
    }

    /// Write the snapshot to a **caller-named** file — [`export`](Self::export)'s funnel
    /// for a caller with no file dialog in front of it, and the agent vocabulary's one write.
    ///
    /// A third gesture into the export the window and the typed `COPY` already make, never a third
    /// implementation: this composes the spec that has no options to get wrong (the whole result,
    /// snapshot order, one file) and hands it to [`export`](Self::export), which keeps the pin,
    /// the background count and the ordinal's exclusion exactly as it does for the window.
    ///
    /// What is new is the destination, because the caller named it with nothing in front of it to
    /// say no — so [`export::check_destination`] is the fence, and it is the whole fence: the data
    /// itself is already readable page by page by whoever can call this.
    pub async fn export_to(
        self,
        path: String,
        format: export::Format,
    ) -> Result<export::ExportReport, EngineError> {
        let root = self.engine.data_root.lock().unwrap().clone();
        export::check_destination(&path, root.as_deref())?;
        self.export(export::ExportSpec {
            path,
            scope: export::Scope::All,
            sort: None,
            format,
            partition: export::Partition::default(),
        })
        .await
    }

    /// Hold the snapshot open for as long as the returned [`SnapshotPin`] lives: while a pin is
    /// out, retiring the snapshot is **deferred**, not skipped.
    ///
    /// A snapshot is owned by its workspace and retired the moment that workspace dispatches
    /// another run (`docs/SNAPSHOT_SPEC.md` §4) — which is right for the grid, whose pages
    /// follow the tab, and wrong for anything that outlives one press. The export window is
    /// the first such reader: it is opened *on a result*, may sit there while the user goes
    /// back and re-runs the query, and must still write the rows that were on screen when it
    /// was opened. Without a pin a re-run deregisters the table mid-`COPY` and truncates the
    /// file, or — the quieter failure — makes a later Export report no results at all when
    /// there are plainly some on screen.
    ///
    /// **RAII rather than a pin/unpin pair**, deliberately: this is the same rule freya-query
    /// cache entries live by (lifetime is a held handle, never imperative bookkeeping), so a
    /// caller expresses "I still need this" by keeping the guard, and an early return, a
    /// panic or a dropped window all release it.
    pub fn pin(self) -> SnapshotPin {
        let mut lc = self.engine.lifecycle.lock().unwrap();
        *lc.pins.entry(self.snapshot).or_insert(0) += 1;
        drop(lc);
        SnapshotPin {
            engine: self.engine.owned(),
            snapshot: self.snapshot,
        }
    }

    /// Does the snapshot still exist to be read?
    ///
    /// The one honest way to tell "your result was replaced" from a real read failure. A
    /// retired snapshot's table is deregistered, so [`page`](Self::page)
    /// answers with DataFusion's own "table not found" prose — and matching that prose at a
    /// call site is exactly the copy-of-a-rule this crate keeps refusing to hand out
    /// ([`EngineError::Stopped`](crate::EngineError::Stopped) is the same lesson). A reader that
    /// outlived its snapshot asks **after** its read fails, so the answer cannot race the
    /// dispatch that retired it.
    ///
    /// `Lifecycle::stats` is the register consulted because it has exactly a snapshot's
    /// lifetime by construction — inserted when the write pass settles, removed by
    /// `Engine::retire_now`, which every retire of a handed-out snapshot goes
    /// through. A snapshot whose retire is **deferred** behind a pin is still there to read,
    /// and still answers `true`, which is the same fact from the reader's side.
    pub fn live(self) -> bool {
        self.engine
            .lifecycle
            .lock()
            .unwrap()
            .stats
            .contains_key(&self.snapshot)
    }
}
