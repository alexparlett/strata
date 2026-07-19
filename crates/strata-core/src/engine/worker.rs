//! The engine worker: [`engine_loop`] owns the `SessionContext` and drains `Command`s
//! on the engine thread, dispatching each to the sibling modules (`query`, `explain`,
//! `export`, `catalog`). Also builds the context from the `datafusion.*` overrides.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;

use datafusion::prelude::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::catalog::{column_info, plan_deps, rebuild_listing, register_external, run_profile};
use super::explain::run_explain;
use super::export::{run_export, ExportArgs};
use super::query::{fetch_page, run_and_snapshot, snapshot_dir, snapshot_file, snapshot_name, CellFormat};
use super::{Command, Event};

/// The catalog + schema **we own**. DataFusion's `"datafusion"` / `"public"` are only
/// *defaults*: `datafusion.catalog.default_*` renames them from the user's engine
/// config, and `SessionConfig::from_env` would inherit `DATAFUSION_*` env vars too (we
/// build from `new()`, so those can't reach us today — but that's a construction
/// detail, not a guarantee). Either would move our tables out from under
/// [`Command::RefreshCatalog`], which looks them up by name. Naming them ourselves is
/// free: the DataFusion catalog is never persisted — every window rebuilds it by
/// re-registering the project's tables on open.
const CATALOG: &str = "strata";
const SCHEMA: &str = "public";

/// Build a `SessionContext` honouring the engine config `overrides`: the
/// `ConfigOptions` keys go on the `SessionConfig`; the `datafusion.runtime.*` keys
/// build a `RuntimeEnv` (parsed via `parse_capacity_limit`). Bad values are logged
/// and skipped rather than failing the whole engine.
fn build_context(overrides: &BTreeMap<String, String>) -> SessionContext {
    let mut config = SessionConfig::new();
    for (key, value) in overrides {
        if key.starts_with("datafusion.runtime.") {
            continue; // runtime.* live on the RuntimeEnv, not ConfigOptions
        }
        if crate::engine::config::is_owned_key(key) {
            continue; // ours (see below) — a stale saved override must not apply
        }
        if let Err(e) = config.options_mut().set(key, value) {
            tracing::warn!("engine config: skipping {key}={value}: {e}");
        }
    }
    // Name the catalog/schema ourselves. The context creates them on construction
    // (`create_default_catalog_and_schema` defaults on) and they stay the resolution
    // default for bare table names. Note this alone wouldn't hold: `SetEngineConfig`
    // re-asserts the overrides over the built config at runtime, which is why
    // `is_owned_key` fences them out of *both* apply paths, not just this one.
    let config = config.with_default_catalog_and_schema(CATALOG, SCHEMA);
    match build_runtime(overrides) {
        Ok(Some(rt)) => SessionContext::new_with_config_rt(config, rt),
        Ok(None) => SessionContext::new_with_config(config),
        Err(e) => {
            tracing::warn!("engine runtime config invalid ({e}); using defaults");
            SessionContext::new_with_config(config)
        }
    }
}

/// A `RuntimeEnv` from the `datafusion.runtime.*` overrides, or `None` when none are
/// set (default runtime). Sizes ("2G", "100G") parse via `parse_capacity_limit`.
fn build_runtime(
    overrides: &BTreeMap<String, String>,
) -> Result<Option<Arc<datafusion::execution::runtime_env::RuntimeEnv>>, String> {
    use datafusion::execution::runtime_env::RuntimeEnvBuilder;
    let val = |k: &str| {
        overrides
            .get(k)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let mem = val("datafusion.runtime.memory_limit");
    let tmp = val("datafusion.runtime.max_temp_directory_size");
    if mem.is_none() && tmp.is_none() {
        return Ok(None);
    }
    let mut b = RuntimeEnvBuilder::new();
    if let Some(m) = mem {
        let bytes = SessionContext::parse_capacity_limit("datafusion.runtime.memory_limit", &m)
            .map_err(|e| e.to_string())?;
        b = b.with_memory_limit(bytes, 1.0);
    }
    if let Some(t) = tmp {
        let bytes =
            SessionContext::parse_capacity_limit("datafusion.runtime.max_temp_directory_size", &t)
                .map_err(|e| e.to_string())?;
        b = b.with_max_temp_directory_size(bytes as u64);
    }
    b.build_arc().map(Some).map_err(|e| e.to_string())
}

/// The two `datafusion.runtime.*` values (memory limit, spill dir cap), for detecting
/// a change that the running engine can't apply live.
fn runtime_keys(o: &BTreeMap<String, String>) -> (Option<String>, Option<String>) {
    (
        o.get("datafusion.runtime.memory_limit").cloned(),
        o.get("datafusion.runtime.max_temp_directory_size").cloned(),
    )
}

/// An in-flight Query/Explain task for one workspace (S14). Keyed by `ws_id` in
/// `engine_loop`'s registry so a re-run or a `Cancel` can abort it; aborting drops
/// the DataFusion stream, cancelling execution cooperatively.
struct InFlight {
    req_id: u64,
    start: Instant,
    abort: tokio::task::AbortHandle,
}

pub async fn engine_loop(
    mut cmd_rx: UnboundedReceiver<Command>,
    evt_tx: UnboundedSender<Event>,
    engine_id: u64,
    mut overrides: BTreeMap<String, String>,
) {
    // Leftover snapshots from a previous run are cleared once at process start
    // (`purge_snapshot_root`), not here — wiping the shared root at runtime would
    // clobber other windows' engines.
    let ctx = build_context(&overrides);
    // The runtime.* config this engine's `RuntimeEnv` was built with — a later Save
    // that leaves the saved runtime.* differing from this needs a window restart.
    let built_runtime = runtime_keys(&overrides);
    // Enumerate the full function registry (built-ins + any UDFs) once, so the UI's
    // SQL language service (S26/S7/S25) can offer + validate real function names.
    // `udafs`/`udwfs` name-set enumerators are DataFusion 54 (part of why A9 gates S26).
    {
        use datafusion::execution::registry::FunctionRegistry;
        let mut scalar: Vec<String> = ctx.udfs().into_iter().collect();
        let mut aggregate: Vec<String> = ctx.udafs().into_iter().collect();
        let mut window: Vec<String> = ctx.udwfs().into_iter().collect();
        scalar.sort();
        aggregate.sort();
        window.sort();
        let _ = evt_tx.send(Event::Functions {
            scalar,
            aggregate,
            window,
        });
    }
    // In-flight Query/Explain tasks, keyed by ws_id (S14). Owned by this single loop
    // task (no locking); the spawned tasks only send events back.
    let mut inflight: HashMap<u64, InFlight> = HashMap::new();
    // In-flight profiles, keyed by **table** — a profile belongs to a table the way a
    // query belongs to a tab, so several can run at once and none blocks the loop (D4).
    let mut profiling: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            Command::Register(spec) => {
                let path = spec.paths.first().cloned().unwrap_or_default();
                // Re-registering replaces the data under any scan of this table: its
                // result is about to be thrown away by `end_profile`, so stop paying
                // for it.
                if let Some(h) = profiling.remove(&spec.name) {
                    h.abort();
                }
                let result = register_external(&ctx, &spec).await;
                let _ = evt_tx.send(Event::Registered {
                    table: spec.name,
                    path,
                    result,
                });
            }
            Command::Profile { table } => {
                // Spawned, never awaited here: a profile is a full scan and can run for
                // minutes — awaiting it in this loop would stall every other command
                // (queries included) behind it. Keyed by table, exactly as a query is
                // keyed by tab, so profiles of different tables run concurrently.
                // Reap finished scans first — a completed handle left in the map would
                // block its own table's re-scan forever.
                profiling.retain(|_, h| !h.is_finished());
                if profiling.contains_key(&table) {
                    continue; // already scanning this table — a second pass adds nothing
                }
                let ctx = ctx.clone();
                let tx = evt_tx.clone();
                let name = table.clone();
                let task = tokio::spawn(async move {
                    let result = run_profile(&ctx, &name).await;
                    let _ = tx.send(Event::Profiled { table: name, result });
                });
                profiling.insert(table, task.abort_handle());
            }
            Command::CancelProfile { table } => {
                // No event: the action layer clears the row itself, since an abort
                // means no `Profiled` is coming.
                if let Some(h) = profiling.remove(&table) {
                    h.abort();
                }
            }
            Command::RefreshCatalog => {
                // A refresh re-infers every table, so every in-flight scan is about to
                // be describing superseded data — abort the lot rather than let them
                // run to a result nothing will keep.
                for (_, h) in profiling.drain() {
                    h.abort();
                }
                // Re-infer each user table's schema in place, straight from what's in
                // the context — no retained specs. Query snapshots (`__snap_*`) are
                // skipped by name; SQL views by not being `ListingTable`s. Files, rows,
                // and partition values are already live (each scan re-`LIST`s — we run
                // no `ListFilesCache`), so this only refreshes the inferred schema.
                if let Some(schema) = ctx.catalog(CATALOG).and_then(|c| c.schema(SCHEMA)) {
                    for name in schema.table_names() {
                        if name.starts_with("__snap_") {
                            continue;
                        }
                        let Ok(Some(provider)) = schema.table(&name).await else {
                            continue;
                        };
                        // Only `ListingTable`s back on-disk data; grab its own paths +
                        // options (drops the borrow before we await). `TableProvider`
                        // has `Any` as a supertrait in DF 54 (no more `as_any`), and
                        // `downcast_ref` is inherent on `dyn TableProvider` — auto-deref
                        // through the `Arc` targets the provider, not the `Arc`.
                        let paths_opts = {
                            use datafusion::datasource::listing::ListingTable;
                            provider
                                .downcast_ref::<ListingTable>()
                                .map(|lt| (lt.table_paths().clone(), lt.options().clone()))
                        };
                        let Some((paths, opts)) = paths_opts else {
                            continue;
                        };
                        let result = rebuild_listing(&ctx, &name, paths, opts).await;
                        let _ = evt_tx.send(Event::Registered {
                            table: name,
                            path: String::new(),
                            result,
                        });
                    }
                }
            }
            Command::Deregister { table } => {
                // The table's going away — a scan of it is now pure waste.
                if let Some(h) = profiling.remove(&table) {
                    h.abort();
                }
                let _ = ctx.deregister_table(table.as_str());
                let _ = evt_tx.send(Event::Deregistered { table });
            }
            Command::CreateView { name, sql } => {
                // Redefining the view replaces the question any in-flight scan is
                // answering — `end_profile` will discard its result, so stop paying.
                if let Some(h) = profiling.remove(&name) {
                    h.abort();
                }
                let stmt = format!("CREATE OR REPLACE VIEW {name} AS {sql}");
                let (mut deps, mut aliases) = (Vec::new(), Vec::new());
                let result = match ctx.sql(&stmt).await {
                    Ok(df) => {
                        let _ = df.collect().await;
                        match ctx.table(name.as_str()).await {
                            Ok(t) => {
                                // The same `DataFrame` gives the columns and what the
                                // view reads (D10) — the planner has already resolved it,
                                // so we never parse the SQL ourselves.
                                let d = plan_deps(t.logical_plan());
                                deps = d.tables;
                                aliases = d.aliases;
                                Ok(t.schema().fields().iter().map(|f| column_info(f)).collect())
                            }
                            Err(e) => Err(e.to_string()),
                        }
                    }
                    Err(e) => Err(e.to_string()),
                };
                let _ = evt_tx.send(Event::ViewChanged {
                    name,
                    sql,
                    dropped: false,
                    deps,
                    aliases,
                    result,
                });
            }
            Command::DropView { name } => {
                // The view's going away — a scan of it is now pure waste.
                if let Some(h) = profiling.remove(&name) {
                    h.abort();
                }
                let result = ctx
                    .sql(&format!("DROP VIEW IF EXISTS {name}"))
                    .await
                    .map(|_| Vec::new())
                    .map_err(|e| e.to_string());
                let _ = evt_tx.send(Event::ViewChanged {
                    name,
                    sql: String::new(),
                    dropped: true,
                    deps: Vec::new(),
                    aliases: Vec::new(),
                    result,
                });
            }
            Command::Query {
                req_id,
                ws_id,
                sql,
                page_size,
            } => {
                // A re-run supersedes the tab's previous query — abort it (saves CPU;
                // today its stale result is merely dropped by `is_pending`).
                if let Some(f) = inflight.remove(&ws_id) {
                    f.abort.abort();
                }
                let ctx = ctx.clone();
                let tx = evt_tx.clone();
                let fmt = CellFormat::new(&overrides);
                let task = tokio::spawn(async move {
                    let result =
                        run_and_snapshot(&ctx, engine_id, ws_id, &sql, page_size, &fmt).await;
                    let _ = tx.send(Event::QueryResult {
                        req_id,
                        ws_id,
                        result,
                    });
                });
                inflight.insert(
                    ws_id,
                    InFlight {
                        req_id,
                        start: Instant::now(),
                        abort: task.abort_handle(),
                    },
                );
            }
            Command::Explain { req_id, ws_id, sql } => {
                // An explain supersedes the tab's previous run too (mutually exclusive).
                if let Some(f) = inflight.remove(&ws_id) {
                    f.abort.abort();
                }
                let ctx = ctx.clone();
                let tx = evt_tx.clone();
                let task = tokio::spawn(async move {
                    let result = run_explain(&ctx, &sql).await;
                    let _ = tx.send(Event::ExplainResult {
                        req_id,
                        ws_id,
                        result,
                    });
                });
                inflight.insert(
                    ws_id,
                    InFlight {
                        req_id,
                        start: Instant::now(),
                        abort: task.abort_handle(),
                    },
                );
            }
            Command::Cancel { ws_id, req_id } => {
                // Abort only if the tab's in-flight run is still this request.
                if inflight.get(&ws_id).map(|f| f.req_id) == Some(req_id) {
                    let f = inflight.remove(&ws_id).unwrap();
                    let elapsed_ms = f.start.elapsed().as_millis();
                    f.abort.abort();
                    // Clear the partial snapshot the aborted task may have left.
                    let _ = ctx.deregister_table(snapshot_name(ws_id).as_str());
                    let _ = std::fs::remove_file(snapshot_file(engine_id, ws_id));
                    let _ = evt_tx.send(Event::QueryCancelled {
                        req_id,
                        ws_id,
                        elapsed_ms,
                    });
                }
            }
            Command::FetchPage {
                ws_id,
                page,
                page_size,
                sort,
            } => {
                let fmt = CellFormat::new(&overrides);
                let result = fetch_page(&ctx, ws_id, page, page_size, sort, &fmt).await;
                let _ = evt_tx.send(Event::PageResult {
                    ws_id,
                    page,
                    result,
                });
            }
            Command::CleanupWorkspace { ws_id } => {
                // Abort a still-running query first so it can't re-register a
                // snapshot for a tab we're tearing down.
                if let Some(f) = inflight.remove(&ws_id) {
                    f.abort.abort();
                }
                let _ = ctx.deregister_table(snapshot_name(ws_id).as_str());
                let _ = std::fs::remove_file(snapshot_file(engine_id, ws_id));
            }
            Command::CleanupAll => {
                for (_, f) in inflight.drain() {
                    f.abort.abort();
                }
                // Only this engine's (this window's) snapshots.
                let _ = std::fs::remove_dir_all(snapshot_dir(engine_id));
            }
            Command::SetEngineConfig(new_overrides) => {
                let old_rt = runtime_keys(&overrides);
                let new_rt = runtime_keys(&new_overrides);
                // Set every live-settable option to its effective value (so a cleared
                // override resets to the default), through the shared state so
                // registered tables survive. runtime.* handled by the restart prompt.
                // Collect any DataFusion rejections (a value that slipped past the UI
                // validator, e.g. a hand-edited config) to surface below.
                let mut rejected = Vec::new();
                {
                    let state = ctx.state_ref();
                    let mut w = state.write();
                    let opts = w.config_mut().options_mut();
                    // Known keys: set each to its override, else reset to the default so a
                    // cleared override reverts. runtime.* is RuntimeEnv-level (restart).
                    for e in crate::engine::config::ENGINE_KEYS {
                        if crate::engine::config::is_restart_key(e.key) {
                            continue;
                        }
                        let val = new_overrides
                            .get(e.key)
                            .map(String::as_str)
                            .unwrap_or(e.default);
                        if let Err(err) = opts.set(e.key, val) {
                            tracing::warn!("engine config: {}={val}: {err}", e.key);
                            rejected.push(format!("{} ({val})", e.key));
                        }
                    }
                    // Custom (non-catalog) overrides: best-effort — DataFusion rejects any
                    // key it doesn't recognise. `is_owned_key` is *not* in `ENGINE_KEYS`,
                    // so without the guard a hand-typed `catalog.default_*` would land
                    // here and re-point resolution away from our own catalog.
                    for (k, val) in &new_overrides {
                        if crate::engine::config::is_restart_key(k)
                            || crate::engine::config::is_owned_key(k)
                            || crate::engine::config::key_def(k).is_some()
                        {
                            continue;
                        }
                        if let Err(err) = opts.set(k, val) {
                            tracing::warn!("engine config: {k}={val}: {err}");
                            rejected.push(format!("{k} ({val})"));
                        }
                    }
                }
                overrides = new_overrides;
                for label in rejected {
                    let _ = evt_tx.send(Event::Notice(format!(
                        "Engine setting ignored — invalid value for {label}."
                    )));
                }
                // Runtime.* changed *and* now differs from what this engine was built
                // with → the running engine is stale; ask the UI to offer a restart.
                if new_rt != old_rt && new_rt != built_runtime {
                    let _ = evt_tx.send(Event::EngineRestartRequired);
                }
            }
            Command::Export {
                ws_id,
                path,
                format,
                all,
                page,
                page_size,
                csv_delimiter,
                csv_header,
                csv_null,
                pq_compression,
                pq_level,
                partition_cols,
                keep_partition,
            } => {
                let result = run_export(
                    &ctx,
                    ws_id,
                    ExportArgs {
                        path,
                        format,
                        all,
                        page,
                        page_size,
                        csv_delimiter,
                        csv_header,
                        csv_null,
                        pq_compression,
                        pq_level,
                        partition_cols,
                        keep_partition,
                    },
                )
                    .await;
                let _ = evt_tx.send(Event::Exported { result });
            }
        }
    }

    // Command channel closed → this window's engine is done → tidy its snapshots
    // (belt-and-suspenders with the `CleanupAll` from `use_drop`; the startup
    // `purge_snapshot_root` covers an abrupt app exit that skips both).
    let _ = std::fs::remove_dir_all(snapshot_dir(engine_id));
}
