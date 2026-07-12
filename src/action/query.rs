//! Query / results / saved-query action handlers. Called from `action::dispatch`
//! (and from `catalog::menu_action` for the `SELECT *` / saved-query menu items).

use dioxus::prelude::*;

use crate::ddl::{self, Decision};
use crate::engine::Command;
use crate::state::{AppState, LogKind, SavedQuery};

/// Run the active tab's SQL (DDL-classified: run / capture-view / drop-view / block).
pub fn run(mut state: Signal<AppState>) {
    let sql = crate::session::active_sql();
    let trimmed = sql.trim().to_string();
    if trimmed.is_empty() {
        state.write().set_status(LogKind::Info, "Nothing to run");
        return;
    }
    // `EXPLAIN [ANALYZE]` takes a dedicated path: the engine runs it and returns
    // a parsed plan tree (S12) rather than a paged result snapshot.
    if crate::plan::is_explain(&trimmed) {
        explain(state, trimmed);
        return;
    }
    match ddl::classify(&trimmed) {
        Decision::Block { reason } => {
            let id = crate::session::active_id();
            if id != 0 {
                crate::runs::edit_existing(id, |run| {
                    run.running = false;
                    run.result = None;
                });
            }
            tracing::warn!("blocked statement: {reason}");
            state
                .write()
                .set_status(LogKind::Warn, format!("Blocked · {reason}"));
        }
        Decision::CaptureView { name, sql } => {
            let tx = state.read().cmd_tx.clone();
            if let Some(tx) = tx {
                let _ = tx.send(Command::CreateView { name, sql });
            }
            state.write().set_status(LogKind::Info, "Saving view…");
        }
        Decision::DropView { name } => {
            let tx = state.read().cmd_tx.clone();
            if let Some(tx) = tx {
                let _ = tx.send(Command::DropView { name });
            }
        }
        Decision::Query => {
            let req = {
                let mut s = state.write();
                let r = s.next_req;
                s.next_req += 1;
                r
            };
            let ws_id = crate::session::active_id();
            let page_size = crate::runs::RUNS
                .resolve()
                .get(ws_id)
                .map(|e| e.peek().page_size)
                .unwrap_or(100);
            crate::runs::edit(ws_id, |run| {
                run.running = true;
                run.query_error = None;
                run.plan = None;
                run.pending_req = Some(req);
                run.page = 1;
            });
            let tx = state.read().cmd_tx.clone();
            if let Some(tx) = tx {
                let _ = tx.send(Command::Query {
                    req_id: req,
                    ws_id,
                    sql: trimmed,
                    page_size,
                });
            }
            state.write().set_status(LogKind::Run, "Running…");
        }
    }
}

/// Send an already-built `EXPLAIN …` statement to the engine's explain path for the
/// active tab — the shared core of `run`'s EXPLAIN branch and `run_explain` (E4).
fn explain(mut state: Signal<AppState>, sql: String) {
    let req = {
        let mut s = state.write();
        let r = s.next_req;
        s.next_req += 1;
        r
    };
    let ws_id = crate::session::active_id();
    crate::runs::edit(ws_id, |run| {
        run.running = true;
        run.query_error = None;
        run.plan = None;
        run.pending_req = Some(req);
        run.page = 1;
    });
    let tx = state.read().cmd_tx.clone();
    if let Some(tx) = tx {
        let _ = tx.send(Command::Explain {
            req_id: req,
            ws_id,
            sql,
        });
    }
    state.write().set_status(LogKind::Run, "Explaining…");
}

/// Run an `EXPLAIN [ANALYZE]` of the active tab's SQL **without mutating the editor
/// buffer** (E4): wrap the current SQL with the prefix (stripping any existing one) and
/// route it through the engine's explain path. Like Save-as-view, the change lives in
/// the engine, not the editor — the user's query in the editor stays untouched.
pub fn run_explain(mut state: Signal<AppState>, analyze: bool) {
    let sql = crate::session::active_sql();
    if sql.trim().is_empty() {
        state
            .write()
            .set_status(LogKind::Info, "Nothing to explain");
        return;
    }
    explain(state, crate::plan::as_explain(&sql, analyze));
}

/// Clear the active tab's results back to the empty state (Rz8): drop the result / plan /
/// error and the find query. No-op mid-run, or when there's nothing to clear.
pub fn clear_results(mut state: Signal<AppState>) {
    let id = crate::session::active_id();
    if id == 0 {
        return;
    }
    let mut cleared = false;
    crate::runs::edit_existing(id, |run| {
        if run.running || (run.result.is_none() && run.query_error.is_none() && run.plan.is_none())
        {
            return;
        }
        run.result = None;
        run.page_batch = None;
        run.query_error = None;
        run.plan = None;
        run.result_search = String::new();
        run.sel = None;
        run.sel_anchor = None;
        run.sort = None;
        run.col_widths.clear();
        cleared = true;
    });
    if cleared {
        state.write().set_status(LogKind::Info, "Cleared results");
    }
}

/// Open/close a tab's results find popover (U6). Closing clears its find query so a
/// stale filter never lingers. No-op if the tab has no run yet.
pub fn set_results_find(ws: crate::session::WorkspaceId, open: bool) {
    crate::runs::edit_existing(ws, |r| {
        r.find_open = open;
        if !open {
            r.result_search.clear();
        }
    });
}

/// Cancel the active tab's in-flight query / explain (S14). No-op if nothing is
/// running. Scoped to the current `pending_req`, so a stale Esc/click can't abort a
/// just-started newer run.
pub fn cancel(state: Signal<AppState>) {
    let ws_id = crate::session::active_id();
    let req = crate::runs::RUNS.resolve().get(ws_id).and_then(|e| {
        let r = e.peek();
        r.running.then_some(r.pending_req).flatten()
    });
    let Some(req_id) = req else {
        return;
    };
    if let Some(tx) = state.read().cmd_tx.clone() {
        let _ = tx.send(Command::Cancel { ws_id, req_id });
    }
}

/// Dismiss the results-pane error view (falls back to the grid if a prior
/// result is still loaded, otherwise the "no results yet" empty state).
pub fn dismiss_error(_state: Signal<AppState>) {
    let id = crate::session::active_id();
    if id != 0 {
        crate::runs::edit_existing(id, |run| run.query_error = None);
    }
}

/// Switch the EXPLAIN plan view between the physical and logical trees.
pub fn set_plan_tab(_state: Signal<AppState>, tab: crate::plan::PlanTab) {
    let id = crate::session::active_id();
    if id != 0 {
        crate::runs::edit_existing(id, |run| run.plan_tab = tab);
    }
}

/// Toggle the EXPLAIN plan view between the operator-card tree and raw text.
pub fn toggle_plan_raw(_state: Signal<AppState>) {
    let id = crate::session::active_id();
    if id != 0 {
        crate::runs::edit_existing(id, |run| run.plan_raw = !run.plan_raw);
    }
}

/// Fetch a specific page from the active workspace's snapshot (bounded LIMIT/OFFSET).
pub fn fetch_page(state: Signal<AppState>, page: usize) {
    let ws_id = crate::session::active_id();
    // Resolve the active sort (if any) to `(column name, ascending)` so the engine can
    // ORDER BY it over the snapshot; the name comes from the result schema at that index.
    let (page_size, has_result, sort) = crate::runs::RUNS
        .resolve()
        .get(ws_id)
        .map(|e| {
            let run = e.peek();
            let sort = run.sort.and_then(|s| {
                run.result
                    .as_ref()
                    .and_then(|r| r.columns.get(s.col))
                    .map(|c| (c.name.clone(), s.asc))
            });
            (run.page_size, run.result.is_some(), sort)
        })
        .unwrap_or((100, false, None));
    if !has_result {
        return;
    }
    // Selection is page-local — dropping it avoids stale indices highlighting the new page.
    crate::runs::edit(ws_id, |run| {
        run.page = page;
        run.sel = None;
        run.sel_anchor = None;
    });
    let tx = state.read().cmd_tx.clone();
    if let Some(tx) = tx {
        let _ = tx.send(Command::FetchPage {
            ws_id,
            page,
            page_size,
            sort,
        });
    }
}

/// Cycle the active tab's column sort (Rz6): unsorted → `ci` asc → `ci` desc → clear, then
/// re-fetch page 1. Sort is applied over the whole snapshot at page-read time.
pub fn sort_column(state: Signal<AppState>, ci: usize) {
    let ws_id = crate::session::active_id();
    if ws_id == 0 {
        return;
    }
    crate::runs::edit(ws_id, |run| {
        run.sort = match run.sort {
            Some(crate::runs::ColSort { col, asc: true }) if col == ci => {
                Some(crate::runs::ColSort { col: ci, asc: false })
            }
            Some(crate::runs::ColSort { col, asc: false }) if col == ci => None,
            _ => Some(crate::runs::ColSort { col: ci, asc: true }),
        };
    });
    fetch_page(state, 1);
}

/// Update the find-in-results query.
pub fn set_result_search(_state: Signal<AppState>, q: String) {
    let id = crate::session::active_id();
    if id != 0 {
        crate::runs::edit(id, |run| run.result_search = q);
    }
}

/// Switch the active tab's result view (grid ↔ chart). Per result-set.
pub fn set_results_view(v: crate::runs::ResultsView) {
    let id = crate::session::active_id();
    if id != 0 {
        crate::runs::edit(id, |run| run.view = v);
    }
}

/// Toggle the page-size dropdown.
pub fn toggle_page_size_menu(mut state: Signal<AppState>) {
    let mut s = state.write();
    s.page_size_open = !s.page_size_open;
}

/// Set the page size and reload the first page.
pub fn set_page_size(mut state: Signal<AppState>, size: usize) {
    state.write().page_size_open = false;
    let id = crate::session::active_id();
    if id != 0 {
        crate::runs::edit(id, |run| run.page_size = size);
    }
    fetch_page(state, 1);
}

/// Pretty-print the active tab's SQL in place.
pub fn format(_state: Signal<AppState>) {
    let cur = crate::session::active_sql();
    let out = sqlformat::format(
        &cur,
        &sqlformat::QueryParams::None,
        &sqlformat::FormatOptions::default(),
    );
    crate::session::set_sql(crate::session::active_id(), out);
}

/// Clear the active tab's SQL.
pub fn clear(_state: Signal<AppState>) {
    crate::session::set_sql(crate::session::active_id(), String::new());
}

/// Save the active SELECT as a named catalog view (auto-named `saved_view_N`).
pub fn save_as_view(mut state: Signal<AppState>) {
    let sql = crate::session::active_sql();
    let n = state.read().project.views.len() + 1;
    let name = format!("saved_view_{n}");
    // The tab is now bound to (and in sync with) this view.
    crate::session::set_origin(
        crate::session::active_id(),
        crate::state::Origin::View(name.clone()),
    );
    let tx = state.read().cmd_tx.clone();
    if let Some(tx) = tx {
        let _ = tx.send(Command::CreateView { name, sql });
    }
    state.write().set_status(LogKind::Info, "Saving view…");
}

/// Load `SELECT * FROM t LIMIT <row_limit>` into the active tab (does not run).
/// The LIMIT comes from the "Default row limit" setting (0 = no limit).
pub fn select_star(mut state: Signal<AppState>, table: &str) {
    let limit = crate::settings::SETTINGS.resolve().peek().row_limit;
    let sql = if limit > 0 {
        format!("SELECT *\nFROM {table}\nLIMIT {limit};")
    } else {
        format!("SELECT *\nFROM {table};")
    };
    crate::session::open_named(table, sql, crate::state::Origin::Scratch);
    state.write().set_status(
        LogKind::Info,
        format!("Loaded SELECT * for '{table}' — press ⌘/Ctrl+Enter to run"),
    );
}

/// Save the active tab's SQL to the project under the tab's name (upsert by name,
/// case-insensitive). Bound to ⌘S.
pub fn save(mut state: Signal<AppState>) {
    let Some(w) = crate::session::active() else {
        return;
    };
    let name = w.name.trim().to_string();
    if name.is_empty() {
        return;
    }
    let sql = w.sql.clone();
    let meta = crate::runs::RUNS
        .resolve()
        .get(w.id)
        .and_then(|e| {
            e.peek()
                .result
                .as_ref()
                .map(|r| format!("{} rows", r.total))
        })
        .unwrap_or_else(|| "—".to_string());
    let mut s = state.write();
    let updated = if let Some(q) = s
        .project
        .saved_queries
        .iter_mut()
        .find(|q| q.name.eq_ignore_ascii_case(&name))
    {
        q.sql = sql;
        q.meta = meta;
        true
    } else {
        s.project.saved_queries.push(SavedQuery {
            name: name.clone(),
            sql,
            meta,
        });
        false
    };
    let verb = if updated { "Updated" } else { "Saved" };
    s.push_log(LogKind::Ok, format!("{verb} query '{name}' to project"));
    s.set_status(LogKind::Ok, format!("{verb} query '{name}'"));
    drop(s);
    // The tab is now bound to (and in sync with) this saved query.
    crate::session::set_origin(w.id, crate::state::Origin::SavedQuery(name.clone()));
}

/// Open a saved query: reuse a tab already named after it, else open a new tab.
pub fn open_saved(state: Signal<AppState>, name: &str) {
    let sql = state
        .read()
        .project
        .saved_queries
        .iter()
        .find(|q| q.name == name)
        .map(|q| q.sql.clone());
    let Some(sql) = sql else {
        return;
    };
    crate::session::open_named(
        name,
        sql,
        crate::state::Origin::SavedQuery(name.to_string()),
    );
}

/// Delete a saved query from the project (immediate — no confirm dialog).
pub fn delete_saved(mut state: Signal<AppState>, name: &str) {
    let mut s = state.write();
    s.project.saved_queries.retain(|q| q.name != name);
    s.push_log(LogKind::Info, format!("Deleted query '{name}'"));
    s.set_status(LogKind::Info, format!("Deleted query '{name}'"));
}

/// `Action::RunExport` — pick a destination (native save dialog, or a folder for a
/// partitioned export) and export the snapshot to a file via the engine's `COPY … TO`.
/// Copying results to the clipboard is handled entirely by the grid's Copy (bounded to
/// the current page), so export is file-only and streams to disk — safe at any size.
pub fn run_export(state: Signal<AppState>, ex: crate::state::ExportForm) {
    let (ws_id, page, page_size, tx) = {
        let s = state.read();
        let ws_id = crate::session::active_id();
        let (page, page_size) = crate::runs::RUNS
            .resolve()
            .get(ws_id)
            .map(|e| {
                let run = e.peek();
                (run.page, run.page_size)
            })
            .unwrap_or((1, 100));
        (ws_id, page, page_size, s.cmd_tx.clone())
    };

    let ext = match ex.format.as_str() {
        "json" => "json",
        "parquet" => "parquet",
        "arrow" => "arrow",
        _ => "csv",
    };
    let partitioned = !ex.partition_cols.is_empty();
    let default_name = format!("{}.{ext}", ex.name);

    spawn(async move {
        // Partitioned export writes a *directory* of `col=value/` parts → pick a
        // folder (a subfolder named after the export holds the tree); otherwise
        // pick a file and force the extension so DataFusion writes one file.
        let dest = if partitioned {
            rfd::AsyncFileDialog::new()
                .pick_folder()
                .await
                .map(|h| h.path().join(&ex.name).to_string_lossy().into_owned())
        } else {
            rfd::AsyncFileDialog::new()
                .set_file_name(&default_name)
                .save_file()
                .await
                .map(|h| {
                    let mut p = h.path().to_string_lossy().into_owned();
                    let want = format!(".{ext}");
                    if !p.to_lowercase().ends_with(&want) {
                        p.push_str(&want);
                    }
                    p
                })
        };
        if let (Some(path), Some(tx)) = (dest, tx) {
            let _ = tx.send(Command::Export {
                ws_id,
                path,
                format: ex.format,
                all: ex.scope != "page",
                page,
                page_size,
                csv_delimiter: delim_char(&ex.csv_delim),
                csv_header: ex.csv_header,
                csv_null: ex.csv_null,
                pq_compression: ex.pq_compression,
                pq_level: ex.pq_level,
                partition_cols: ex.partition_cols,
                keep_partition: ex.keep_partition,
            });
        }
        crate::overlays::close_export();
    });
}

fn delim_char(d: &str) -> char {
    match d {
        "tab" => '\t',
        "semicolon" => ';',
        "pipe" => '|',
        _ => ',',
    }
}

/// Rz4 — copy the current grid selection to the clipboard. Builds a sub-`RecordBatch` (project
/// the selected columns + take the selected rows over the page batch), then serializes it via
/// [`crate::serialize`] into a [`ClipboardWriter`]. `Tsv` (default: ⌘C) carries no header; the
/// rest do. Page-local: selection indices are into the grid's filtered visible page.
pub fn copy_selection(mut state: Signal<AppState>, fmt: crate::serialize::TextFormat) {
    use datafusion::arrow::array::{ArrayRef, RecordBatch, UInt32Array};

    let ws_id = crate::session::active_id();
    let sub: Option<(RecordBatch, usize, usize)> =
        crate::runs::RUNS.resolve().get(ws_id).and_then(|e| {
            let run = e.peek();
            let sel = run.sel.clone()?;
            let result = run.result.as_ref()?;
            let batch = run.page_batch.as_ref()?;
            let search = run.result_search.to_lowercase();
            // Map each filtered *display* row index → its page-`batch` row index (the grid's
            // search filter is page-local; `result.rows` and the batch share row order).
            let filtered_to_batch: Vec<usize> = result
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    search.is_empty() || r.iter().any(|c| c.text.to_lowercase().contains(&search))
                })
                .map(|(oi, _)| oi)
                .collect();
            let ncols = result.columns.len();
            let (mut frows, mut cols): (Vec<usize>, Vec<usize>) = match &sel {
                crate::runs::Selection::Cell { .. } => {
                    let (minr, maxr, minc, maxc) = sel.cell_bounds()?;
                    ((minr..=maxr).collect(), (minc..=maxc).collect())
                }
                crate::runs::Selection::Rows(rs) => (rs.clone(), (0..ncols).collect()),
                crate::runs::Selection::Cols(cs) => {
                    ((0..filtered_to_batch.len()).collect(), cs.clone())
                }
            };
            frows.sort_unstable();
            frows.dedup();
            frows.retain(|&r| r < filtered_to_batch.len());
            cols.sort_unstable();
            cols.dedup();
            cols.retain(|&c| c < ncols && c < batch.num_columns());
            if frows.is_empty() || cols.is_empty() {
                return None;
            }
            let batch_rows: Vec<u32> = frows.iter().map(|&i| filtered_to_batch[i] as u32).collect();
            let projected = batch.project(&cols).ok()?;
            let indices = UInt32Array::from(batch_rows);
            let taken: Vec<ArrayRef> = projected
                .columns()
                .iter()
                .map(|c| datafusion::arrow::compute::take(&**c, &indices, None))
                .collect::<Result<_, _>>()
                .ok()?;
            let sub = RecordBatch::try_new(projected.schema(), taken).ok()?;
            Some((sub, frows.len(), cols.len()))
        });

    let mut s = state.write();
    match sub {
        None => s.set_status(LogKind::Warn, "Nothing selected to copy"),
        Some((batch, r, c)) => {
            // All formats carry a header row / keys — consistent across TSV/CSV/JSON/Markdown so
            // a copied selection is always self-describing.
            let header = true;
            let mut clip = crate::serialize::ClipboardWriter::new();
            let res = crate::serialize::write_batch(fmt, &batch, header, &mut clip)
                .map_err(|e| e.to_string())
                .and_then(|_| clip.commit());
            match res {
                Ok(()) => s.set_status(LogKind::Ok, format!("Copied {r}×{c} to clipboard")),
                Err(e) => s.set_status(LogKind::Error, format!("Clipboard failed · {e}")),
            }
        }
    }
}
