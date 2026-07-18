//! The results area (R1). A persistent **toolbar** (grid/chart states only: find ·
//! Table/Chart toggle · refresh · download) over the state **body** (running spinner /
//! structured error / EXPLAIN plan / grid / chart placeholder / "no results" empty),
//! with a single **unified status bar** at the foot in *every* state: a state-coloured
//! dot + label + subtext, the snapshot clock chip (ticking), and the pager (grid-only,
//! pinned right). The old grid-only green-dot pager is gone — the status token is now
//! consistent across states. Reads this workspace's run from `crate::runs::RUNS`.

use std::time::Duration;

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::project::ProjectStoreExt;
use crate::runs::{ResultsView, Selection};
use crate::session::WorkspaceId;
use crate::ui::components::{
    Body, Button, ButtonVariant, DotStatus, Eyebrow, Icon, IconButton, IconButtonVariant, Meta,
    MonoValue, Pager, Path, Prose, RectAlign, SearchDialog, Segment, SegmentOption, Select,
    SelectOption, Spacer, StatusDot, Title,
};
use crate::ui::icons::{IconName, IconSize};

/// Results = optional toolbar (grid/chart) + the state body + the unified status bar.
/// The body is a mutually-exclusive state switch; the status bar is always present so
/// the status token stays consistent (no-run · running · failed · grid/chart · plan).
#[component]
pub fn Results(ws_id: WorkspaceId) -> Element {
    let (running, has_err, has_plan, has_result, view) = crate::runs::RUNS
        .resolve()
        .get(ws_id)
        .map(|e| {
            let r = e.read();
            (
                r.running,
                r.query_error.is_some(),
                r.plan.is_some(),
                r.result.is_some(),
                r.view,
            )
        })
        .unwrap_or((false, false, false, false, ResultsView::Grid));

    rsx! {
        // Toolbar — only in a result state (grid or chart); find is further grid-only.
        if has_result {
            ResultsToolbar { ws_id }
        }
        // Body — the state switch (each fills the remaining height).
        if running {
            Running { ws_id }
        } else if has_err {
            ErrorView { ws_id }
        } else if has_plan {
            super::plan_view::PlanView { ws_id }
        } else if has_result {
            if view == ResultsView::Chart {
                ChartPlaceholder {}
            } else {
                super::grid::ResultsGrid { ws_id }
            }
        } else {
            Empty { ws_id }
        }
        // Unified status bar — every state.
        StatusBar { ws_id }
    }
}

/// Results area while a query is in flight — a centred spinner. (Cancel is S14.)
#[component]
fn Running(ws_id: WorkspaceId) -> Element {
    let target = target_name(ws_id);
    rsx! {
        div { class: "res-state res-running",
            Icon { name: IconName::Spinner, size: IconSize::Px(30) }
            Title { class: "res-title", "Running query…" }
            Path { class: "res-sub mono", "scanning {target}" }
            Button {
                variant: ButtonVariant::Danger,
                small: true,
                icon: IconName::Stop, icon_size: IconSize::Xs,
                onclick: move |_| dispatch(Action::CancelQuery),
                "Cancel"
            }
        }
    }
}

/// Results area for the last failed query — an error banner, the message, an
/// optional code frame with a caret, and an optional hint. Dismiss clears it.
#[component]
fn ErrorView(ws_id: WorkspaceId) -> Element {
    let err = crate::runs::RUNS
        .resolve()
        .get(ws_id)
        .and_then(|e| e.read().query_error.clone());
    let Some(err) = err else {
        return rsx! { div {} };
    };
    let loc = err.loc.clone().unwrap_or_default();
    rsx! {
        div { class: "res-error ps-scroll",
            div { class: "err-banner",
                span { class: "err-ico", Icon { name: IconName::ErrCircle, size: IconSize::Sm } }
                MonoValue { class: "err-type", "{err.etype}" }
                if !loc.is_empty() {
                    Path { class: "err-loc", "{loc}" }
                }
                Spacer {}
                IconButton {
                    variant: IconButtonVariant::Ghost,
                    icon: IconName::Close,
                    title: "Dismiss",
                    onclick: move |_| dispatch(Action::DismissQueryError),
                }
            }
            div { class: "err-body",
                {super::errview::error_detail(&err)}
            }
        }
    }
}

/// Results area before this workspace has produced any rows. An unrun EXPLAIN gets
/// a plan-specific hint. Also the grid's defensive fallback (see `grid`).
#[component]
pub fn Empty(ws_id: WorkspaceId) -> Element {
    let sql = crate::session::snapshot()
        .workspaces
        .iter()
        .find(|w| w.id == ws_id)
        .map(|w| w.sql.clone())
        .unwrap_or_default();
    let is_explain = crate::plan::is_explain(&sql);
    let (title, sub) = if is_explain {
        ("No plan yet", "Run the EXPLAIN to see the query plan.")
    } else {
        (
            "No results yet",
            "Run the query to load rows from your sources into the grid.",
        )
    };
    rsx! {
        div { class: "res-state res-empty",
            div { class: "res-empty-ico", Icon { name: IconName::Rows, size: IconSize::Px(22) } }
            Title { class: "res-title", "{title}" }
            Prose { class: "res-sub", "{sub}" }
        }
    }
}

/// Chart-view placeholder — R2 (the real canvas chart) isn't built yet; the toggle is
/// live so the layout + status token are exercised. Reflects the current snapshot.
#[component]
fn ChartPlaceholder() -> Element {
    rsx! {
        div { class: "res-state res-empty",
            div { class: "res-empty-ico", Icon { name: IconName::Chart, size: IconSize::Px(22) } }
            Title { class: "res-title", "Chart view coming soon" }
            Prose { class: "res-sub", "Visualising the result snapshot lands with R2. Switch back to Table to view rows." }
        }
    }
}

/// Center-pane placeholder shown when no query tab is open (all tabs closed).
#[component]
pub fn EmptyState() -> Element {
    let has_closed = crate::session::has_closed();
    let saved: Vec<String> = crate::project::store()
        .saved_queries()
        .read()
        .iter()
        .take(4)
        .map(|q| q.name.clone())
        .collect();
    rsx! {
        div { class: "ws-empty",
            div { class: "ws-empty-ico", Icon { name: IconName::Database, size: IconSize::Px(26) } }
            Title { class: "ws-empty-title", "No query open" }
            Prose { class: "ws-empty-sub",
                "Open a new query tab to explore your data, or run "
                MonoValue { class: "mono hl", "SELECT *" }
                " on a table from the catalog."
            }
            div { class: "ws-empty-actions",
                Button {
                    variant: ButtonVariant::Primary,
                    icon: IconName::Plus, icon_size: IconSize::Sm,
                    kbd: crate::keymap::hint(crate::config::Command::NewTab),
                    onclick: move |_| dispatch(Action::NewTab),
                    "New query"
                }
                if has_closed {
                    Button {
                        variant: ButtonVariant::Secondary,
                        icon: IconName::Reopen, icon_size: IconSize::Sm,
                        onclick: move |_| dispatch(Action::ReopenTab),
                        "Reopen closed"
                    }
                }
            }
            if !saved.is_empty() {
                div { class: "ws-empty-saved",
                    Eyebrow { class: "lbl", "SAVED QUERIES" }
                    for name in saved {
                        {
                            let nm = name.clone();
                            rsx! {
                                div { class: "ws-empty-q",
                                    onclick: move |_| dispatch(Action::OpenSavedQuery(nm.clone())),
                                    Icon { name: IconName::Brackets, size: IconSize::Sm, color: "var(--purple)" }
                                    Body { class: "nm", "{name}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The results toolbar (grid/chart states): find-in-results (grid-only, live page
/// match count + clear) · Table/Chart toggle · Refresh (re-run) · Download (export).
#[component]
fn ResultsToolbar(ws_id: WorkspaceId) -> Element {
    let (search, view, matches, page_rows, find_open) = crate::runs::RUNS
        .resolve()
        .get(ws_id)
        .map(|e| {
            let r = e.read();
            let s = r.result_search.to_lowercase();
            let (matches, page_rows) = r
                .result
                .as_ref()
                .map(|res| {
                    let pr = res.rows.len();
                    let m = if s.is_empty() {
                        pr
                    } else {
                        res.rows
                            .iter()
                            .filter(|row| row.iter().any(|c| c.text.to_lowercase().contains(&s)))
                            .count()
                    };
                    (m, pr)
                })
                .unwrap_or((0, 0));
            (
                r.result_search.clone(),
                r.view,
                matches,
                page_rows,
                r.find_open,
            )
        })
        .unwrap_or_default();
    let grid = view == ResultsView::Grid;

    // v19 (U6): the find popover's open flag lives in this tab's `runs` (so ⌘F can reach
    // it), toggled via `Action::SetResultsFind`. While this is the active tab, claim the
    // `Find` command so ⌘F opens *this* toolbar's find (and nothing when no results show).
    use_effect(move || {
        if crate::session::active_id() == ws_id {
            crate::keymap::register(
                crate::keymap::CtxCommand::Find,
                crate::keymap::Responder::Toolbar(ws_id),
            );
        } else {
            crate::keymap::unregister_if(crate::keymap::CtxCommand::Find, ws_id);
        }
    });
    use_drop(move || crate::keymap::unregister_if(crate::keymap::CtxCommand::Find, ws_id));

    rsx! {
        div { class: "results-tb",
            // Table/Chart toggle (left) — text-only segmented (v19).
            Segment {
                value: if grid { "grid" } else { "chart" },
                compact: true,
                on_select: move |v: String| dispatch(Action::SetResultsView(
                    if v == "chart" { ResultsView::Chart } else { ResultsView::Grid },
                )),
                options: vec![
                    SegmentOption::new("grid", "Table"),
                    SegmentOption::new("chart", "Chart"),
                ],
            }
            Spacer {}
            // Right cluster (bordered, 28px): find (grid-only) · refresh · clear · export.
            if grid {
                SearchDialog {
                    trigger_class: if find_open { "ds-icon-btn toolbar compact on" } else { "ds-icon-btn toolbar compact" },
                    title: "Find in results",
                    open: find_open,
                    on_open: move |v| dispatch(Action::SetResultsFind { ws: ws_id, open: v }),
                    value: search.clone(),
                    placeholder: "Find in results",
                    width: 340,
                    oninput: move |v| dispatch(Action::SetResultSearch(v)),
                    trigger: rsx! { Icon { name: IconName::Search, size: IconSize::Md } },
                    trailing: rsx! {
                        if !search.is_empty() {
                            Meta { class: "res-find-count", "{matches} of {page_rows} on page" }
                        }
                    },
                }
            }
            IconButton { icon: IconName::Refresh,
                variant: IconButtonVariant::Toolbar,
                compact: true,
                title: "Refresh — re-run the query",
                onclick: move |_| dispatch(Action::RunQuery),
            }
            IconButton { icon: IconName::Trash,
                variant: IconButtonVariant::Toolbar,
                compact: true,
                class: "res-clear",
                title: "Clear results",
                onclick: move |_| dispatch(Action::ClearResults),
            }
            IconButton { icon: IconName::Download,
                variant: IconButtonVariant::Toolbar,
                compact: true,
                title: "Export results",
                onclick: move |_| crate::overlays::open_export(),
            }
        }
    }
}

/// The unified results status bar — a state dot + label + subtext in every state, the
/// snapshot clock chip (ticking every 15s), and the pager pinned right (grid-only).
#[component]
fn StatusBar(ws_id: WorkspaceId) -> Element {
    // Tick every 15s so the snapshot "Xm ago" label stays fresh.
    let mut tick = use_signal(|| 0u32);
    use_future(move || async move {
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            *tick.write() += 1;
        }
    });
    let _ = tick(); // subscribe → re-render on each tick

    let target = target_name(ws_id);
    let (dot, label, sub, snap, has_result, view, page, page_size, total) = crate::runs::RUNS
        .resolve()
        .get(ws_id)
        .map(|e| {
            let r = e.read();
            let has_result = r.result.is_some();
            let (total, elapsed) = r
                .result
                .as_ref()
                .map(|o| (o.total, o.elapsed_ms))
                .unwrap_or((0, 0));
            let (dot, label, sub) = if r.running {
                ("run", "Running…".to_string(), format!("scanning {target}"))
            } else if let Some(qe) = &r.query_error {
                ("err", "Query failed".to_string(), qe.etype.clone())
            } else if let Some(p) = &r.plan {
                let logical = matches!(r.plan_tab, crate::plan::PlanTab::Logical);
                let ops = if logical {
                    p.logical.len()
                } else {
                    p.physical.len()
                };
                let mode = if logical {
                    "logical"
                } else if p.analyze {
                    "measured"
                } else {
                    "physical"
                };
                (
                    "plan",
                    "Query plan".to_string(),
                    format!("{ops} operators · {mode}"),
                )
            } else if has_result {
                (
                    "ok",
                    format!("{} rows", fmt_int(total as u64)),
                    format!("{elapsed} ms"),
                )
            } else {
                ("idle", "No query run".to_string(), format!("{} to run", crate::keymap::hint(crate::config::Command::RunQuery)))
            };
            // Snapshot chip only once the tab has actually produced a result.
            let snap = if has_result {
                r.ran_at.map(|t| ago_label(t.elapsed()))
            } else {
                None
            };
            (
                dot,
                label,
                sub,
                snap,
                has_result,
                r.view,
                r.page,
                r.page_size,
                total,
            )
        })
        .unwrap_or_else(|| {
            (
                "idle",
                "No query run".to_string(),
                format!("{} to run", crate::keymap::hint(crate::config::Command::RunQuery)),
                None,
                false,
                ResultsView::Grid,
                1,
                100,
                0,
            )
        });

    rsx! {
        div { class: "res-statusbar",
            StatusDot {
                status: match dot {
                    "run" => DotStatus::Run,
                    "err" => DotStatus::Err,
                    "plan" => DotStatus::Plan,
                    "ok" => DotStatus::Ok,
                    _ => DotStatus::Idle,
                },
            }
            Meta { class: "res-stat", "{label}" }
            if !sub.is_empty() {
                Path { class: "res-stat-sub", "· {sub}" }
            }
            if let Some(ago) = snap {
                Path { class: "res-snap", Icon { name: IconName::Clock, size: IconSize::Xs } "snapshot {ago}" }
            }
            if let Some(agg) = selection_agg(ws_id) {
                {agg_token(agg)}
            }
            Spacer {}
            if has_result && view == ResultsView::Grid {
                {pager_controls(total, page, page_size)}
            }
        }
    }
}

/// Live aggregate over the current grid selection (Rz3): cell count, plus Σ / avg / min /
/// max over the selected **numeric** cells. Page-local — mirrors the grid's filtered
/// visible rows. `None` when nothing is selected.
struct AggView {
    cells: usize,
    numeric: usize,
    sum: f64,
    min: f64,
    max: f64,
}

fn selection_agg(ws_id: WorkspaceId) -> Option<AggView> {
    let entry = crate::runs::RUNS.resolve().get(ws_id)?;
    let run = entry.read();
    let sel = run.sel.clone()?;
    let result = run.result.as_ref()?;
    let search = run.result_search.to_lowercase();
    // Same filtered visible page the grid renders (selection indexes into this).
    let rows: Vec<&Vec<crate::model::Cell>> = result
        .rows
        .iter()
        .filter(|r| search.is_empty() || r.iter().any(|c| c.text.to_lowercase().contains(&search)))
        .collect();
    let numeric_col: Vec<bool> = result
        .columns
        .iter()
        .map(|c| c.kind.cell_class() == "num")
        .collect();
    let ncols = result.columns.len();

    let mut coords: Vec<(usize, usize)> = Vec::new();
    match &sel {
        Selection::Cell { .. } => {
            if let Some((minr, maxr, minc, maxc)) = sel.cell_bounds() {
                for r in minr..=maxr {
                    for c in minc..=maxc {
                        coords.push((r, c));
                    }
                }
            }
        }
        Selection::Rows(rs) => {
            for &r in rs {
                for c in 0..ncols {
                    coords.push((r, c));
                }
            }
        }
        Selection::Cols(cs) => {
            for r in 0..rows.len() {
                for &c in cs {
                    coords.push((r, c));
                }
            }
        }
    }

    let mut cells = 0usize;
    let mut numeric = 0usize;
    let mut sum = 0.0f64;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for (r, c) in coords {
        let Some(cell) = rows.get(r).and_then(|row| row.get(c)) else {
            continue;
        };
        cells += 1;
        if numeric_col.get(c).copied().unwrap_or(false) && !cell.null {
            if let Ok(v) = cell.text.replace(',', "").trim().parse::<f64>() {
                numeric += 1;
                sum += v;
                min = min.min(v);
                max = max.max(v);
            }
        }
    }
    (cells > 0).then_some(AggView {
        cells,
        numeric,
        sum,
        min,
        max,
    })
}

/// Compact number for the aggregate strip — up to 4 dp, trailing zeros trimmed.
fn fmt_num(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn agg_token(a: AggView) -> Element {
    let mut parts = vec![format!("{} cells", fmt_int(a.cells as u64))];
    if a.numeric > 0 {
        let avg = a.sum / a.numeric as f64;
        parts.push(format!("Σ {}", fmt_num(a.sum)));
        parts.push(format!("avg {}", fmt_num(avg)));
        parts.push(format!("min {}", fmt_num(a.min)));
        parts.push(format!("max {}", fmt_num(a.max)));
    }
    let text = parts.join("  ·  ");
    rsx! { Meta { class: "res-agg", "{text}" } }
}

/// The pager cluster on the right of the status bar (grid-only): page-size dropdown
/// (opens upward) + first/prev/page-input `of M`/next/last.
fn pager_controls(total: usize, page: usize, page_size: usize) -> Element {
    let page_count = ((total as f64) / (page_size as f64)).ceil().max(1.0) as usize;
    rsx! {
        Select {
            value: page_size.to_string(),
            width: 128,
            align: RectAlign::TOP_START,
            options: vec![
                SelectOption::new("100", "100 / page"),
                SelectOption::new("500", "500 / page"),
                SelectOption::new("1000", "1,000 / page"),
                SelectOption::new("5000", "5,000 / page"),
                SelectOption::new("10000", "10,000 / page"),
            ],
            on_select: move |v: String| {
                if let Ok(n) = v.parse::<usize>() { dispatch(Action::SetPageSize(n)); }
            },
        }
        div { style: "width:1px;height:18px;background:var(--line);" }
        Pager {
            page: page as u32,
            page_count: page_count as u32,
            on_jump: move |n: u32| dispatch(Action::FetchPage(n as usize)),
        }
    }
}

/// The workspace (tab) name for `ws_id`, else a neutral `sources` fallback.
fn target_name(ws_id: WorkspaceId) -> String {
    crate::session::snapshot()
        .workspaces
        .iter()
        .find(|w| w.id == ws_id)
        .map(|w| w.name.clone())
        .unwrap_or_else(|| "sources".into())
}

/// Humanise a snapshot age for the clock chip: `just now` / `Nm ago` / `Nh ago`.
fn ago_label(d: Duration) -> String {
    let s = d.as_secs();
    if s < 45 {
        "just now".into()
    } else if s < 90 {
        "1m ago".into()
    } else if s < 3600 {
        format!("{}m ago", (s + 30) / 60)
    } else if s < 5400 {
        "1h ago".into()
    } else if s < 86_400 {
        format!("{}h ago", (s + 1800) / 3600)
    } else {
        format!("{}d ago", s / 86_400)
    }
}

/// Thousands-separated integer (e.g. `48,213`) for the row count.
fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}
