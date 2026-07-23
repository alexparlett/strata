//! The results-pane footer (P2-08, comp `StatusBar.dc.html` / `data-rg="statusbar"`): a
//! state-toned dot + label + muted sub-label on the left, then the snapshot chip (clock +
//! relative age, live-ticking) and the accent selection aggregate (Rz3 — count over every
//! selected cell, Σ / avg / min / max over the numeric ones); the pager cluster — page-size
//! dropdown (opens upward) · 1px divider · first / prev / page-input "of M" / next / last —
//! pins right, grid state only.

use std::time::{Duration, Instant};

use async_io::Timer;
use freya::components::use_theme;
use freya::prelude::*;
use strata_model::Kind;

use super::datagrid::{GridData, PageRead};
use super::selection::Selection;
use super::ResultsState;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{InputTypography, Meta, Path};

define_theme!(
    %[component]
    pub StatusBar {
        %[fields]
        background: Color,
        color: Color,
        border_fill: Color,
        sub_color: Color,
        control_color: Color,
    }
);

/// The rows-per-page choices the page-size dropdown offers (matches the comp).
const PAGE_SIZES: [usize; 4] = [100, 250, 500, 1000];

/// The pager's slots in the footer: the 1-based page and the rows-per-page the results pane
/// owns (bumping either re-keys the pane's snapshot read), plus the snapshot total that bounds
/// them. A page-size pick resets to page 1 — the old page number indexes a different cut.
#[derive(Clone, Copy, PartialEq)]
pub struct Pager {
    pub page: State<usize>,
    pub page_size: State<usize>,
    pub total: usize,
}

impl Pager {
    fn pages(self) -> usize {
        let size = (*self.page_size.read()).max(1);
        if self.total == 0 { 1 } else { self.total.div_ceil(size) }
    }
}

/// The settled Run the grid state reports: row count + engine elapsed for the label, and the
/// settle instant the snapshot chip ages against.
#[derive(Clone, Copy, PartialEq)]
pub struct RunInfo {
    pub total: usize,
    pub elapsed_ms: u128,
    pub settled: Instant,
}

/// The results-pane footer — present in *every* state (empty · running · grid · plan · error).
///
/// Themed by `status_bar`. The state-dot / label colour is **semantic** — read from the palette,
/// not the component token — so it tracks the same success/warning/error slots the rest of the
/// app uses; the aggregate takes the palette's `primary` accent the same way.
#[derive(PartialEq)]
pub struct StatusBar {
    state: ResultsState,
    pager: Option<Pager>,
    info: Option<RunInfo>,
    /// Physical-operator count for the plan state's sub-label.
    plan_ops: Option<usize>,
    /// The resolved current page (grid state) — the selection aggregate reads its real cells.
    view: Option<PageRead>,
    pub theme: Option<StatusBarThemePartial>,
}

impl StatusBar {
    pub fn new(state: ResultsState) -> Self {
        Self { state, pager: None, info: None, plan_ops: None, view: None, theme: None }
    }

    /// Show the pager cluster (the grid state passes it; every other state passes nothing).
    pub fn pager(mut self, pager: Pager) -> Self {
        self.pager = Some(pager);
        self
    }

    /// The settled Run's readouts (grid state): row count, elapsed, snapshot age.
    pub fn info(mut self, info: RunInfo) -> Self {
        self.info = Some(info);
        self
    }

    /// Operator count for the plan state's sub-label.
    pub fn plan_ops(mut self, ops: usize) -> Self {
        self.plan_ops = Some(ops);
        self
    }

    /// The resolved page the selection aggregate reads (grid state).
    pub fn view(mut self, view: PageRead) -> Self {
        self.view = Some(view);
        self
    }
}

impl Component for StatusBar {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&self.theme, StatusBarThemePreference, "status_bar");

        // Dot + label tone and the aggregate's accent are semantic palette slots, independent
        // of the `status_bar` token.
        let app_theme = use_theme();
        let (dot_color, accent) = {
            let c = &app_theme.read().colors;
            let dot = match self.state {
                ResultsState::Empty => c.text_placeholder,
                ResultsState::Running => c.warning,
                ResultsState::Grid => c.success,
                ResultsState::ExplainPlan => c.info,
                ResultsState::Error => c.error,
            };
            (dot, c.primary)
        };

        // Label + sub-label per state (comp `_statusVals`): the grid state leads with the real
        // row count and trails the engine elapsed; the plan state counts operators. The empty
        // state's run hint derives from the effective keymap (rebinds repaint it; unbound
        // drops the sub-label).
        let run_hint = crate::keymap::use_hint(strata_core::config::Command::RunQuery);
        let (label, sub): (String, Option<String>) = match self.state {
            ResultsState::Empty => (
                "No query run".into(),
                (!run_hint.is_empty()).then(|| format!("{run_hint} to run")),
            ),
            ResultsState::Running => ("Running…".into(), Some("scanning sources".into())),
            ResultsState::Grid => match &self.info {
                Some(info) => (
                    format!("{} rows", fmt_int(info.total)),
                    Some(format!("· {} ms", info.elapsed_ms)),
                ),
                None => ("Results".into(), None),
            },
            ResultsState::ExplainPlan => (
                "Query plan".into(),
                self.plan_ops.map(|n| {
                    format!("{n} operator{} · physical", if n == 1 { "" } else { "s" })
                }),
            ),
            ResultsState::Error => ("Query failed".into(), None),
        };

        // The live aggregate over the current selection's real cells (Rz3) — only when the
        // shown page has settled (a page in flight has no cells to sum).
        let sel = consume_context::<State<Selection>>();
        let agg = self
            .view
            .as_ref()
            .and_then(PageRead::ready)
            .and_then(|data| selection_agg(&sel.read(), data))
            .map(|a| a.label());

        rect()
            .width(Size::fill())
            .height(Size::px(40.))
            .min_height(Size::px(40.))
            .content(Content::Flex)
            .background(theme.background)
            // 1px top divider (Freya's `Border` is all-sides; a pinned 1px child is the local idiom
            // for a single edge), then the bar row fills the rest.
            .child(Divider::horizontal().color(theme.border_fill))
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .direction(Direction::Horizontal)
                    // Flex content so the info cluster below can flex — that's what pins the
                    // pager to the right edge (a `Size::flex` child needs a flex-content parent).
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    // padding 0 sp-4 · gap sp-4 (matches the comp's `statusbar` row).
                    .padding(Gaps::new(0., 12., 0., 12.))
                    .spacing(12.)
                    // The info cluster owns all the slack (which is also what pins the pager
                    // right) and clips at its own edge, so a narrow window never paints the
                    // readouts under the pager — the aggregate ellipsizes first (it takes the
                    // cluster's remaining width via `flex`).
                    .child(
                        rect()
                            .width(Size::flex(1.))
                            .height(Size::fill())
                            .direction(Direction::Horizontal)
                            .content(Content::Flex)
                            .cross_align(Alignment::Center)
                            .spacing(12.)
                            .overflow(Overflow::Clip)
                            .child(
                                rect()
                                    .width(Size::px(7.))
                                    .height(Size::px(7.))
                                    .corner_radius(3.5)
                                    .background(dot_color),
                            )
                            .child(Meta::new(label).color(dot_color))
                            .maybe_child(sub.map(|text| Path::new(text).color(theme.sub_color)))
                            .map(self.info, |el, info| {
                                el.child(SnapshotChip {
                                    settled: info.settled,
                                    color: theme.sub_color,
                                })
                            })
                            .maybe_child(agg.map(|text| {
                                Meta::new(text)
                                    .color(accent)
                                    .width(Size::flex(1.))
                                    .text_overflow(TextOverflow::Ellipsis)
                            })),
                    )
                    .map(self.pager, |el, pager| {
                        el.child(PagerCluster { pager, theme: theme.clone(), accent })
                    }),
            )
    }
}

// ── snapshot chip ──────────────────────────────────────────────────────────────────────────────

/// Clock + "snapshot 2m ago": how stale the grid's materialized snapshot is. The age re-derives
/// on a slow tick so it stays honest while the tab sits open; the tooltip spells the semantics
/// out (comp `title=`).
#[derive(PartialEq)]
struct SnapshotChip {
    settled: Instant,
    color: Color,
}

impl Component for SnapshotChip {
    fn render(&self) -> impl IntoElement {
        // Ticks the age label along (10s ≪ the 45s "just now" window, so no visible jump is
        // missed). Scope-bound: leaving the grid state unmounts it.
        let mut now = use_state(Instant::now);
        use_hook(move || {
            spawn(async move {
                loop {
                    Timer::after(Duration::from_secs(10)).await;
                    now.set(Instant::now());
                }
            });
        });
        let ago = ago_label(now().saturating_duration_since(self.settled));

        TooltipContainer::new(Tooltip::new(
            "Results are a snapshot taken when the query last ran — not live files. Refresh to re-run.",
        ))
        .position(AttachedPosition::Top)
        .child(
            rect()
                .direction(Direction::Horizontal)
                .cross_align(Alignment::Center)
                .spacing(4.)
                .color(self.color)
                .child(Icon::new(IconName::Clock).size(12.))
                .child(Path::new(format!("snapshot {ago}")).color(self.color)),
        )
    }
}

// ── selection aggregate (Rz3) ─────────────────────────────────────────────────────────────────

/// Live aggregate over the current grid selection: cell count, plus Σ / avg / min / max over the
/// selected **numeric** cells. Page-local — the selection indexes into the shown page.
struct AggView {
    cells: usize,
    numeric: usize,
    sum: f64,
    min: f64,
    max: f64,
}

impl AggView {
    /// The accent strip's text: "N cells · Σ x · avg x · min x · max x" (count only when the
    /// selection holds no numeric cells).
    fn label(&self) -> String {
        let mut parts = vec![if self.cells == 1 {
            "1 cell".to_string()
        } else {
            format!("{} cells", fmt_int(self.cells))
        }];
        if self.numeric > 0 {
            let avg = self.sum / self.numeric as f64;
            parts.push(format!("Σ {}", fmt_num(self.sum)));
            parts.push(format!("avg {}", fmt_num(avg)));
            parts.push(format!("min {}", fmt_num(self.min)));
            parts.push(format!("max {}", fmt_num(self.max)));
        }
        parts.join("  ·  ")
    }
}

/// Aggregate the selection over the page's real cells — the Dioxus `selection_agg`, minus the
/// find-in-results filter (P2-09). Numeric = a `Kind::Num` column's non-null cell; the engine
/// formats numbers with thousands separators, so parsing strips the commas back out.
fn selection_agg(sel: &Selection, data: &GridData) -> Option<AggView> {
    let ncols = data.columns.len();
    let nrows = data.rows.len();

    let mut coords: Vec<(usize, usize)> = Vec::new();
    match sel {
        Selection::None => return None,
        Selection::Cell { .. } => {
            let (minr, maxr, minc, maxc) = sel.cell_bounds()?;
            for r in minr..=maxr {
                for c in minc..=maxc {
                    coords.push((r, c));
                }
            }
        }
        Selection::Rows(rows) => {
            for &r in rows {
                for c in 0..ncols {
                    coords.push((r, c));
                }
            }
        }
        Selection::Cols(cols) => {
            for r in 0..nrows {
                for &c in cols {
                    coords.push((r, c));
                }
            }
        }
    }

    let mut agg = AggView { cells: 0, numeric: 0, sum: 0.0, min: f64::INFINITY, max: f64::NEG_INFINITY };
    for (r, c) in coords {
        let Some(cell) = data.rows.get(r).and_then(|row| row.get(c)) else {
            continue;
        };
        agg.cells += 1;
        let numeric_col = data.columns.get(c).is_some_and(|col| col.kind == Kind::Num);
        if numeric_col && !cell.null {
            if let Ok(v) = cell.text.replace(',', "").trim().parse::<f64>() {
                agg.numeric += 1;
                agg.sum += v;
                agg.min = agg.min.min(v);
                agg.max = agg.max.max(v);
            }
        }
    }
    (agg.cells > 0).then_some(agg)
}

// ── pager cluster ─────────────────────────────────────────────────────────────────────────────

/// The grid state's right cluster, to the comp: page-size dropdown (opens upward) · 1px divider ·
/// first / prev / page-number input ("of M") / next / last as standard 28×28 flat `Button`s.
/// Every jump clears the selection — its indices would silently point at *different* cells on
/// the new page, and the aggregate would lie.
#[derive(PartialEq)]
struct PagerCluster {
    pager: Pager,
    theme: StatusBarTheme,
    accent: Color,
}

impl PagerCluster {
    /// Jump to `target` (already clamped): clear the page-local selection, then bump the page.
    fn jump(mut sel: State<Selection>, mut page: State<usize>, target: usize) {
        sel.set(Selection::None);
        page.set(target);
    }
}

impl Component for PagerCluster {
    fn render(&self) -> impl IntoElement {
        let pager = self.pager;
        let mut page_size = pager.page_size;
        let page = pager.page;
        let pages = pager.pages();
        let current = *page.read();
        let size = *page_size.read();
        let sel = consume_context::<State<Selection>>();

        // The page input's text, following the page state (the chevrons and a size pick move it
        // too). Submit parses + clamps; garbage snaps back to the shown page.
        let mut text = use_state(move || current.to_string());
        use_side_effect(move || {
            let p = *page.read();
            text.set_if_modified(p.to_string());
        });

        // ── page-size dropdown ────────────────────────────────────────────────────────────
        // The app-standard `Select`: the input-shell trigger + item menu the comp draws
        // (`data-hv="input"`), themed by `select` / `menu_item`. Pinned `open_up` — the bar
        // sits on the window's bottom edge, so the comp always opens the menu upward. A pick
        // is a new cut of the snapshot — back to page 1 of it (the old page number is
        // meaningless).
        let accent = self.accent;
        let dropdown = Select::new()
            .open_up()
            .selected_item(Meta::new(format!("{size} / page")).color(self.theme.control_color))
            .children(PAGE_SIZES.iter().map(|&n| {
                MenuItem::new()
                    .selected(n == size)
                    .on_press(move |_| {
                        Self::jump(sel, page, 1);
                        page_size.set(n);
                    })
                    .child({
                        let label = Meta::new(format!("{n} / page"));
                        if n == size { label.color(accent) } else { label }
                    })
                    .into()
            }));

        // ── nav buttons + page input ──────────────────────────────────────────────────────
        // The standard flat ("ghost") `Button`, 28×28 like every other icon-button cluster in
        // the app — the `flat_button` theme carries the whole dress, ghost hover and
        // `disabled_*` at-a-bound tint included.
        let nav = move |icon: IconName, enabled: bool, target: usize| {
            Button::new()
                .flat()
                .enabled(enabled)
                .width(Size::px(28.))
                .height(Size::px(28.))
                .on_press(move |_| Self::jump(sel, page, target))
                .child(Icon::new(icon).size(15.))
        };

        let jump_input = rect()
            .direction(Direction::Horizontal)
            .cross_align(Alignment::Center)
            .spacing(8.)
            .padding((0., 8.))
            .child(InputTypography::mono(
                Input::new(text)
                    .compact()
                    .width(Size::px(44.))
                    .text_align(TextAlign::Center)
                    .on_submit(move |v: String| {
                        match v.trim().parse::<usize>() {
                            Ok(n) => {
                                let target = n.clamp(1, pages);
                                Self::jump(sel, page, target);
                                // Re-echo even when the page didn't move (e.g. "999" at the end).
                                text.set(target.to_string());
                            }
                            Err(_) => text.set((*page.peek()).to_string()),
                        }
                    }),
            ))
            .child(Path::new(format!("of {}", fmt_int(pages))).color(self.theme.sub_color));

        rect()
            .direction(Direction::Horizontal)
            .cross_align(Alignment::Center)
            .spacing(12.)
            .child(dropdown)
            .child(rect().width(Size::px(1.)).height(Size::px(18.)).background(self.theme.border_fill))
            .child(
                rect()
                    .direction(Direction::Horizontal)
                    .cross_align(Alignment::Center)
                    .spacing(2.)
                    .child(nav(IconName::First, current > 1, 1))
                    .child(nav(IconName::ChevronLeft, current > 1, current.saturating_sub(1).max(1)))
                    .child(jump_input)
                    .child(nav(IconName::ChevronRight, current < pages, (current + 1).min(pages)))
                    .child(nav(IconName::Last, current < pages, pages)),
            )
    }
}

// ── formatting ────────────────────────────────────────────────────────────────────────────────

/// Thousands-separated integer ("12847" → "12,847").
fn fmt_int(n: usize) -> String {
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

/// Compact number for the aggregate strip — up to 4 dp, trailing zeros trimmed.
fn fmt_num(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Coarse relative age for the snapshot chip ("just now" → "3m ago" → "2h ago" → "1d+ ago").
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
        "1d+ ago".into()
    }
}

#[cfg(test)]
mod tests {
    use strata_model::{Cell, ColumnInfo};

    use super::*;

    #[test]
    fn ints_group_by_thousands() {
        assert_eq!(fmt_int(0), "0");
        assert_eq!(fmt_int(999), "999");
        assert_eq!(fmt_int(1000), "1,000");
        assert_eq!(fmt_int(12_847), "12,847");
        assert_eq!(fmt_int(1_234_567), "1,234,567");
    }

    #[test]
    fn nums_trim_to_four_dp() {
        assert_eq!(fmt_num(3.0), "3");
        assert_eq!(fmt_num(3.5), "3.5");
        assert_eq!(fmt_num(1.0 / 3.0), "0.3333");
        assert_eq!(fmt_num(-2.50), "-2.5");
    }

    #[test]
    fn ago_coarsens_with_age() {
        assert_eq!(ago_label(Duration::from_secs(10)), "just now");
        assert_eq!(ago_label(Duration::from_secs(60)), "1m ago");
        assert_eq!(ago_label(Duration::from_secs(150)), "3m ago");
        assert_eq!(ago_label(Duration::from_secs(7200)), "2h ago");
        assert_eq!(ago_label(Duration::from_secs(100_000)), "1d+ ago");
    }

    fn grid() -> GridData {
        let col = |name: &str, kind: Kind| ColumnInfo {
            name: name.into(),
            dtype: "t".into(),
            kind,
            nullable: true,
            children: Vec::new(),
            stats: Vec::new(),
        };
        let cell = |text: &str| Cell { text: text.into(), null: false };
        GridData {
            columns: vec![col("n", Kind::Num), col("s", Kind::Str)],
            rows: vec![
                vec![cell("1,000"), cell("a")],
                vec![Cell { text: "".into(), null: true }, cell("b")],
                vec![cell("2.5"), cell("c")],
            ],
        }
    }

    #[test]
    fn cell_rectangle_aggregates_numeric_cells_only() {
        let data = grid();
        // Whole grid: 6 cells, numeric column has 1,000 and 2.5 (one null skipped).
        let sel = Selection::Cell { ar: 0, ac: 0, fr: 2, fc: 1 };
        let agg = selection_agg(&sel, &data).expect("cells selected");
        assert_eq!(agg.cells, 6);
        assert_eq!(agg.numeric, 2);
        assert_eq!(agg.sum, 1002.5);
        assert_eq!(agg.min, 2.5);
        assert_eq!(agg.max, 1000.0);
        assert_eq!(agg.label(), "6 cells  ·  Σ 1002.5  ·  avg 501.25  ·  min 2.5  ·  max 1000");
    }

    #[test]
    fn non_numeric_selection_shows_count_only() {
        let data = grid();
        let sel = Selection::Cols(vec![1]);
        let agg = selection_agg(&sel, &data).expect("column selected");
        assert_eq!(agg.cells, 3);
        assert_eq!(agg.numeric, 0);
        assert_eq!(agg.label(), "3 cells");
    }

    #[test]
    fn single_cell_reads_singular() {
        let data = grid();
        let sel = Selection::Cell { ar: 0, ac: 1, fr: 0, fc: 1 };
        let agg = selection_agg(&sel, &data).expect("cell selected");
        assert_eq!(agg.label(), "1 cell");
    }

    #[test]
    fn rows_selection_spans_every_column() {
        let data = grid();
        let sel = Selection::Rows(vec![0, 2]);
        let agg = selection_agg(&sel, &data).expect("rows selected");
        assert_eq!(agg.cells, 4);
        assert_eq!(agg.numeric, 2);
        assert_eq!(agg.sum, 1002.5);
    }

    #[test]
    fn empty_selection_has_no_aggregate() {
        assert!(selection_agg(&Selection::None, &grid()).is_none());
    }
}
