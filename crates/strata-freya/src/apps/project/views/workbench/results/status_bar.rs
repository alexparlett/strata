//! The results-pane footer (P2-08, comp `StatusBar.dc.html` / `data-rg="statusbar"`): a
//! state-toned dot + label + muted sub-label on the left, then the snapshot chip (clock +
//! relative age, live-ticking) and the accent selection aggregate (Rz3 — count over every
//! selected cell, Σ / avg / min / max over the numeric ones); the pager cluster — page-size
//! dropdown (opens upward) · 1px divider · first / prev / page-input "of M" / next / last —
//! pins right, grid state only.

use std::time::{Duration, Instant};

use async_io::Timer;
use freya::prelude::*;
use strata_arrow::plan::PlanTab;
use strata_core::config::Command;
use strata_core::util::fmt_int;
use strata_engine::sql::StmtKind;
use strata_model::Kind;

use super::datagrid::{GridData, PageRead};
use super::selection::Selection;
use super::ResultsState;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_2, SP_3, SP_4};
use crate::components::tones::tones;
use crate::components::toolbar::{Toolbar, ToolbarAction, ToolbarItem};
use crate::components::typography::{InputTypography, Meta, Path};
use crate::keymap::use_hint;
use crate::theme::{use_roles, Role};

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

/// The bar's own height, and the state dot's.
const BAR_HEIGHT: f32 = 40.;
const DOT_SIZE: f32 = 7.;
/// What the two composite pager items cost the fold arithmetic. Stated because a `Custom` item
/// cannot be measured before it is laid out, and these are the widths the comp draws:
/// the `Select` at its longest label ("1000 / page"), and the page box plus its "of M".
const PAGE_SIZE_WIDTH: f32 = 110.;
const JUMP_INPUT_WIDTH: f32 = 44.;

/// What the jump box costs the fold arithmetic: its input, the gap, and the `"of {pages}"` beside
/// it — whose width **follows the page count**, which has no upper bound (`total.div_ceil(100)` at
/// the smallest page size runs to five digits and beyond).
///
/// `PAGE_SIZES` is a closed set so the dropdown can be a constant; this cannot. A fixed budget
/// under-charges a wide count, and since `Custom::width` only feeds `fold_plan` while the real
/// `JumpBox` renders at its natural width, the row would be judged to fit while the flex info
/// cluster beside it got squeezed by the difference.
///
/// An estimate, not a measurement — 11px mono is ~6.6px/char, and the fold budget only needs to
/// not be *under*.
fn jump_width(pages: usize) -> f32 {
    const CHAR_W: f32 = 6.6;
    let digits = count(pages).chars().count() as f32;
    JUMP_INPUT_WIDTH + 8. + ("of ".len() as f32 + digits) * CHAR_W
}

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
        if self.total == 0 {
            1
        } else {
            self.total.div_ceil(size)
        }
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

/// The results-pane footer — present in *every* state (empty · running · grid · plan ·
/// statement · error).
///
/// Themed by `status_bar`. The state-dot / label colour is **semantic** — read from the palette,
/// not the component token — so it tracks the same success/warning/error slots the rest of the
/// app uses; the aggregate takes the palette's `primary` accent the same way.
#[derive(PartialEq)]
pub struct StatusBar {
    state: ResultsState,
    pager: Option<Pager>,
    info: Option<RunInfo>,
    /// The plan state's sub-label: operator count of the shown tree + which tree it is.
    plan: Option<(usize, PlanTab)>,
    /// The statement state's readouts: what ran, and how long the engine took (ED-02).
    statement: Option<(StmtKind, u128)>,
    /// The resolved current page (grid state) — the selection aggregate reads its real cells.
    view: Option<PageRead>,
    pub theme: Option<StatusBarThemePartial>,
}

impl StatusBar {
    pub fn new(state: ResultsState) -> Self {
        Self {
            state,
            pager: None,
            info: None,
            plan: None,
            statement: None,
            view: None,
            theme: None,
        }
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

    /// The plan state's sub-label: the shown tree's operator count + which tree (P2-05 —
    /// tracks the view's Physical/Logical selection).
    pub fn plan(mut self, ops: usize, tab: PlanTab) -> Self {
        self.plan = Some((ops, tab));
        self
    }

    /// The statement state's readouts (ED-02): the statement's own SQL name, off the same
    /// `StmtKind::label` table the body's title reads, and the engine's elapsed.
    pub fn statement(mut self, kind: StmtKind, elapsed_ms: u128) -> Self {
        self.statement = Some((kind, elapsed_ms));
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

        let tones = tones();
        let roles = use_roles();
        let dot_color = match self.state {
            ResultsState::Empty => roles.get(Role::TextPlaceholder),
            ResultsState::Running => tones.warning,
            ResultsState::Grid | ResultsState::Chart | ResultsState::Statement => tones.ok,
            ResultsState::ExplainPlan => tones.info,
            ResultsState::Error => tones.error,
        };
        let accent = roles.get(Role::Accent);

        let run_hint = use_hint(Command::RunQuery);
        let (label, sub): (String, Option<String>) = match self.state {
            ResultsState::Empty => (
                "No query run".into(),
                (!run_hint.is_empty()).then(|| format!("{run_hint} to run")),
            ),
            ResultsState::Running => ("Running…".into(), Some("scanning sources".into())),
            ResultsState::Grid => match &self.info {
                Some(info) => (
                    format!("{} rows", count(info.total)),
                    Some(format!("· {} ms", info.elapsed_ms)),
                ),
                None => ("Results".into(), None),
            },
            ResultsState::Chart => match &self.info {
                Some(info) => (
                    format!("{} rows", count(info.total)),
                    Some("charting snapshot".into()),
                ),
                None => ("Results".into(), None),
            },
            ResultsState::ExplainPlan => (
                "Query plan".into(),
                self.plan.map(|(n, tab)| {
                    let tree = match tab {
                        PlanTab::Physical => "physical",
                        PlanTab::Logical => "logical",
                    };
                    format!("{n} operator{} · {tree}", if n == 1 { "" } else { "s" })
                }),
            ),
            ResultsState::Statement => match self.statement {
                Some((kind, elapsed_ms)) => {
                    (kind.label().into(), Some(format!("· {elapsed_ms} ms")))
                }
                None => ("Statement executed".into(), None),
            },
            ResultsState::Error => ("Query failed".into(), None),
        };

        let sel = consume_context::<State<Selection>>();
        let agg = self
            .view
            .as_ref()
            .and_then(PageRead::ready)
            .and_then(|data| selection_agg(&sel.read(), data))
            .map(|a| a.label());

        let info_cluster = rect()
            .width(Size::flex(1.))
            .height(Size::fill())
            .direction(Direction::Horizontal)
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(SP_4)
            .overflow(Overflow::Clip)
            .child(
                rect()
                    .width(Size::px(DOT_SIZE))
                    .height(Size::px(DOT_SIZE))
                    .corner_radius(DOT_SIZE / 2.)
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
            }));

        let bar = Toolbar::new()
            .height(BAR_HEIGHT - 1.)
            .padding(SP_4)
            .spacing(SP_4)
            .leading(info_cluster, 0.)
            .map(self.pager, |bar, pager| {
                let pages = pager.pages();
                let current = *pager.page.read();
                let nav = |icon, label: &'static str, enabled, target: usize| {
                    ToolbarAction::new(icon, label)
                        .enabled(enabled)
                        .on_press(move |_| jump(sel, pager.page, target))
                };

                bar.item(
                    ToolbarItem::Custom {
                        width: PAGE_SIZE_WIDTH,
                        inline: PageSize {
                            pager,
                            theme: theme.clone(),
                            accent,
                        }
                        .into_element(),
                        folded: None,
                    }
                    .rank(0),
                )
                .item(ToolbarItem::Separator.rank(0))
                .item(nav(IconName::First, "First page", current > 1, 1).rank(2))
                .item(
                    nav(
                        IconName::ChevronLeft,
                        "Previous",
                        current > 1,
                        current.saturating_sub(1).max(1),
                    )
                    .rank(3),
                )
                .item(
                    ToolbarItem::Custom {
                        width: jump_width(pages),
                        inline: JumpBox {
                            pager,
                            color: theme.sub_color,
                        }
                        .into_element(),
                        folded: None,
                    }
                    .rank(1),
                )
                .item(
                    nav(
                        IconName::ChevronRight,
                        "Next",
                        current < pages,
                        (current + 1).min(pages),
                    )
                    .rank(3),
                )
                .item(nav(IconName::Last, "Last page", current < pages, pages).rank(2))
            });

        rect()
            .width(Size::fill())
            .height(Size::px(BAR_HEIGHT))
            .min_height(Size::px(BAR_HEIGHT))
            .content(Content::Flex)
            .background(theme.background)
            .child(Divider::horizontal().color(theme.border_fill))
            .child(bar)
    }
}

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

        TooltipContainer::new(Tooltip::new_text(
            "Results are a snapshot taken when the query last ran — not live files. Refresh to re-run.",
        ))
        .position(AttachedPosition::Top)
        .child(
            rect()
                .direction(Direction::Horizontal)
                .cross_align(Alignment::Center)
                .spacing(SP_2)
                .color(self.color)
                .child(Icon::new(IconName::Clock).size(12.))
                .child(Path::new(format!("snapshot {ago}")).color(self.color)),
        )
    }
}

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
    /// Fold one selected coordinate in. Out-of-range coordinates are *skipped*, not an error:
    /// the selection is page-local and a page can shrink under it (a filter, a shorter last
    /// page) between the write and this read.
    ///
    /// Numeric = a [`Kind::Num`] column's non-null cell whose text parses — the engine formats
    /// numbers with thousands separators, so the commas come back out first. A null counts as
    /// a selected *cell* but never as a numeric one.
    fn add(&mut self, data: &GridData, r: usize, c: usize) {
        let Some(cell) = data.rows.get(r).and_then(|row| row.get(c)) else {
            return;
        };
        self.cells += 1;
        let numeric_col = data.columns.get(c).is_some_and(|col| col.kind == Kind::Num);
        if numeric_col && !cell.null {
            if let Ok(v) = cell.text.replace(',', "").trim().parse::<f64>() {
                self.numeric += 1;
                self.sum += v;
                self.min = self.min.min(v);
                self.max = self.max.max(v);
            }
        }
    }

    /// The accent strip's text: "N cells · Σ x · avg x · min x · max x" (count only when the
    /// selection holds no numeric cells).
    fn label(&self) -> String {
        let mut parts = vec![if self.cells == 1 {
            "1 cell".to_string()
        } else {
            format!("{} cells", count(self.cells))
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

/// Aggregate the selection over the page's real cells — the Dioxus `selection_agg`. The view
/// handed in is the *find-filtered* page (P2-09), which the page-local selection indexes
/// into, so the aggregate stays honest under an active filter.
///
/// The selection is walked and folded in one pass — Select-All over the pager's largest cut is
/// 1000 × N coordinates, and this runs on **every** selection change (a drag-paint fires it per
/// pointer move), so materializing them first would allocate the whole grid to read it.
fn selection_agg(sel: &Selection, data: &GridData) -> Option<AggView> {
    let ncols = data.columns.len();
    let nrows = data.rows.len();

    let mut agg = AggView {
        cells: 0,
        numeric: 0,
        sum: 0.0,
        min: f64::INFINITY,
        max: f64::NEG_INFINITY,
    };
    match sel {
        Selection::None => return None,
        Selection::Cell { .. } => {
            let (minr, maxr, minc, maxc) = sel.cell_bounds()?;
            for r in minr..=maxr {
                for c in minc..=maxc {
                    agg.add(data, r, c);
                }
            }
        }
        Selection::Rows(rows) => {
            for &r in rows {
                for c in 0..ncols {
                    agg.add(data, r, c);
                }
            }
        }
        Selection::Cols(cols) => {
            for r in 0..nrows {
                for &c in cols {
                    agg.add(data, r, c);
                }
            }
        }
    }
    (agg.cells > 0).then_some(agg)
}

/// Jump to `target` (already clamped): clear the page-local selection, then bump the page.
///
/// Every jump clears the selection — its indices would silently point at *different* cells on the
/// new page, and the aggregate would lie.
fn jump(mut sel: State<Selection>, mut page: State<usize>, target: usize) {
    sel.set(Selection::None);
    page.set(target);
}

/// The page-size dropdown: the app-standard [`Select`], pinned `open_up` because the bar sits on
/// the window's bottom edge. A pick is a new cut of the snapshot, so it returns to page 1 — the
/// old page number indexes a different cut.
///
/// Its own component so its width is the one thing the toolbar has to know about it.
#[derive(PartialEq)]
struct PageSize {
    pager: Pager,
    theme: StatusBarTheme,
    accent: Color,
}

impl Component for PageSize {
    fn render(&self) -> impl IntoElement {
        let pager = self.pager;
        let mut page_size = pager.page_size;
        let size = *page_size.read();
        let sel = consume_context::<State<Selection>>();
        let accent = self.accent;
        let control_color = self.theme.control_color;

        Select::new()
            .open_up()
            .selected_item(Meta::new(format!("{size} / page")).color(control_color))
            .children(PAGE_SIZES.iter().map(|&n| {
                MenuItem::new()
                    .selected(n == size)
                    .on_press(move |_| {
                        jump(sel, pager.page, 1);
                        page_size.set(n);
                    })
                    .child({
                        let label = Meta::new(format!("{n} / page"));
                        if n == size {
                            label.color(accent)
                        } else {
                            label
                        }
                    })
            }))
    }
}

/// The page-number box and its "of M". Its own component because the echo state and the effect
/// that keeps it following the page are hooks, and hooks cannot live behind the `Option<Pager>`
/// the status bar renders from.
///
/// Commits on submit only: each report is a snapshot page read, so per-keystroke would load a page
/// per digit. Garbage snaps back to the shown page.
#[derive(PartialEq)]
struct JumpBox {
    pager: Pager,
    color: Color,
}

impl Component for JumpBox {
    fn render(&self) -> impl IntoElement {
        let pager = self.pager;
        let page = pager.page;
        let pages = pager.pages();
        let current = *page.read();
        let sel = consume_context::<State<Selection>>();

        let mut text = use_state(move || current.to_string());
        use_side_effect(move || {
            let p = *page.read();
            text.set_if_modified(p.to_string());
        });

        rect()
            .direction(Direction::Horizontal)
            .cross_align(Alignment::Center)
            .spacing(SP_3)
            .child(InputTypography::mono(
                Input::new(text)
                    .compact()
                    .width(Size::px(JUMP_INPUT_WIDTH))
                    .text_align(TextAlign::Center)
                    .on_submit(move |v: String| match v.trim().parse::<usize>() {
                        Ok(n) => {
                            let target = n.clamp(1, pages);
                            jump(sel, page, target);
                            text.set(target.to_string());
                        }
                        Err(_) => text.set((*page.peek()).to_string()),
                    }),
            ))
            .child(Path::new(format!("of {}", count(pages))).color(self.color))
    }
}

/// Thousands-separated count ("12847" → "12,847") — [`strata_core::util::fmt_int`] over a
/// `usize`, so the footer's counts read exactly like the plan view's metrics and the column
/// inspector's row counts.
fn count(n: usize) -> String {
    fmt_int(n as u64)
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
    use std::sync::Arc;

    use datafusion::arrow::datatypes::{DataType, Field};
    use strata_arrow::{column_info, RecordBatch, Schema};
    use strata_model::Cell;

    use super::*;

    /// The jump box's fold budget follows the page count, which is unbounded.
    ///
    /// A fixed budget under-charged a wide count, and because `Custom::width` only feeds
    /// `fold_plan` while the box renders at its natural width, the row was judged to fit while the
    /// flex info cluster beside it absorbed the difference.
    #[test]
    fn the_jump_box_budget_grows_with_the_page_count() {
        let one = jump_width(1);
        let many = jump_width(12_345);

        assert!(
            many > one,
            "a five-digit count costs more than a one-digit one: {many} vs {one}"
        );
        assert!(
            many - one >= 5. * 6.,
            "and by roughly the characters it added: {}",
            many - one
        );
        assert!(one > JUMP_INPUT_WIDTH, "the label is always charged too");
    }

    #[test]
    fn ints_group_by_thousands() {
        assert_eq!(count(0), "0");
        assert_eq!(count(12_847), "12,847");
        assert_eq!(count(1_234_567), "1,234,567");
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
        let col = |name: &str, dtype: DataType| column_info(&Field::new(name, dtype, true));
        let cell = |text: &str| Cell {
            text: text.into(),
            null: false,
        };
        GridData {
            columns: vec![col("n", DataType::Int64), col("s", DataType::Utf8)],
            rows: vec![
                vec![cell("1,000"), cell("a")],
                vec![
                    Cell {
                        text: String::new(),
                        null: true,
                    },
                    cell("b"),
                ],
                vec![cell("2.5"), cell("c")],
            ],
            batch: RecordBatch::new_empty(Arc::new(Schema::empty())),
        }
    }

    #[test]
    fn cell_rectangle_aggregates_numeric_cells_only() {
        let data = grid();
        let sel = Selection::Cell {
            ar: 0,
            ac: 0,
            fr: 2,
            fc: 1,
        };
        let agg = selection_agg(&sel, &data).expect("cells selected");
        assert_eq!(agg.cells, 6);
        assert_eq!(agg.numeric, 2);
        assert_eq!(agg.sum, 1002.5);
        assert_eq!(agg.min, 2.5);
        assert_eq!(agg.max, 1000.0);
        assert_eq!(
            agg.label(),
            "6 cells  ·  Σ 1002.5  ·  avg 501.25  ·  min 2.5  ·  max 1000"
        );
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
        let sel = Selection::Cell {
            ar: 0,
            ac: 1,
            fr: 0,
            fc: 1,
        };
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
