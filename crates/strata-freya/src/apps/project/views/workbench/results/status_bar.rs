use super::ResultsState;
use crate::apps::project::views::workbench::results::selection::Selection;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Meta;
use freya::components::use_theme;
use freya::prelude::*;

define_theme!(
    %[component]
    pub StatusBar {
        %[fields]
        background: Color,
        color: Color,
        border_fill: Color,
        hover_background: Color,
    }
);

/// The grid's pager slot in the footer: the 1-based page `State` the grid reads (prev/next bump
/// it — each bump re-keys the grid's snapshot read), plus the snapshot totals that bound it.
#[derive(Clone, Copy, PartialEq)]
pub struct Pager {
    pub page: State<usize>,
    pub total: usize,
    pub page_size: usize,
}

impl Pager {
    fn pages(self) -> usize {
        if self.total == 0 { 1 } else { self.total.div_ceil(self.page_size) }
    }
}

/// The results-pane footer — present in *every* state (empty · running · grid). A state-coloured
/// dot + a state label sit on the left; the pager (grid state only) sits on the right. The rest of
/// the right region (snapshot chip · selection aggregate · elapsed) fills out with P2-08.
///
/// Themed by `status_bar` (background · label colour · top divider · future hover). The state-dot
/// colour is **semantic** — read from the palette, not the component token — so it tracks the same
/// success/warning/error slots the rest of the app uses.
#[derive(PartialEq)]
pub struct StatusBar {
    state: ResultsState,
    pager: Option<Pager>,
    pub theme: Option<StatusBarThemePartial>,
}

impl StatusBar {
    pub fn new(state: ResultsState) -> Self {
        Self { state, pager: None, theme: None }
    }

    /// Show the pager cluster (the grid state passes it; every other state passes `None`).
    pub fn pager(mut self, pager: Option<Pager>) -> Self {
        self.pager = pager;
        self
    }
}

impl Component for StatusBar {
    fn render(&self) -> impl IntoElement {

        // `hover_background` is themed for the (coming) pager buttons but not painted yet — the
        // theme file owns the whole token regardless.
        let StatusBarTheme { background, color, border_fill, .. } =
            get_theme!(&self.theme, StatusBarThemePreference, "status_bar");

        // The dot colour is a semantic palette slot, independent of the `status_bar` token.
        let theme = use_theme();
        let dot_color = {
            let c = &theme.read().colors;
            match self.state {
                ResultsState::Empty => c.text_placeholder,
                ResultsState::Running => c.warning,
                ResultsState::Grid => c.success,
                ResultsState::ExplainPlan => c.info,
                ResultsState::Error => c.error,
            }
        };
        let sel = consume_context::<State<Selection>>();

        // The snapshot chip · selection aggregate · elapsed readouts land with P2-08; until then
        // the footer is the coarse state + the pager.
        let label_text = match self.state {
            ResultsState::Empty => "No query run",
            ResultsState::Running => "Running…",
            ResultsState::Grid => "Results",
            ResultsState::ExplainPlan => "Query plan",
            ResultsState::Error => "Query failed",
        };

        let selection = sel.read().clone();
        let sel_text = match selection {
            Selection::None => None,
            Selection::Cell { ar, ac, fc, fr } => {
                Some(format!("{}:{} ({}:{})", ac, ar, fc, fr))
            }
            Selection::Rows(rows) => {
                Some(format!("{} rows", rows.len()))
            }
            Selection::Cols(cols) => {
                Some(format!("{} cols", cols.len()))
            }
        };

        rect()
            .width(Size::fill())
            .height(Size::px(40.))
            .min_height(Size::px(40.))
            .content(Content::Flex)
            .background(background)
            // 1px top divider (Freya's `Border` is all-sides; a pinned 1px child is the local idiom
            // for a single edge), then the bar row fills the rest.
            .child(Divider::horizontal().color(border_fill))
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .direction(Direction::Horizontal)
                    // Flex content so the spacer below can flex — that's what pins the pager
                    // to the right edge (a `Size::flex` child needs a flex-content parent).
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    // padding: 0 sp-3 0 sp-4 · gap sp-3 (matches `.res-statusbar`).
                    .padding(Gaps::new(0., 8., 0., 12.))
                    .spacing(8.)
                    .child(
                        rect()
                            .width(Size::px(7.))
                            .height(Size::px(7.))
                            .corner_radius(3.5)
                            .background(dot_color),
                    )
                    .child(Meta::new(label_text).color(color))
                    .maybe_child(sel_text.map(|text| Meta::new(text).color(color)))
                    // Spacer pins the pager to the bar's right edge.
                    .child(rect().width(Size::flex(1.)))
                    .map(self.pager, |el, pager| el.child(PagerCluster { pager, color })),
            )
    }
}

/// Prev/next + the visible row range — the minimal working pager (P2-08 dresses it to the comp:
/// `hover_background`, snapshot chip, aggregates). Prev/next bump the shared page `State`; the
/// grid's snapshot read re-keys off it, so a revisited page settles from the freya-query cache.
#[derive(PartialEq)]
struct PagerCluster {
    pager: Pager,
    color: Color,
}

impl Component for PagerCluster {
    fn render(&self) -> impl IntoElement {
        let pager = self.pager;
        let mut page = pager.page;
        let current = *page.read();
        let pages = pager.pages();

        let range = if pager.total == 0 {
            "0 rows".to_string()
        } else {
            let start = (current - 1) * pager.page_size + 1;
            let end = (current * pager.page_size).min(pager.total);
            format!("{start}–{end} of {}", pager.total)
        };

        let nav = |icon: IconName| {
            Button::new()
                .flat()
                .width(Size::px(22.))
                .height(Size::px(22.))
                .child(Icon::new(icon).size(12.))
        };

        rect()
            .direction(Direction::Horizontal)
            .cross_align(Alignment::Center)
            .spacing(4.)
            .child(nav(IconName::ChevronLeft).on_press(move |_| {
                let p = *page.peek();
                if p > 1 {
                    page.set(p - 1);
                }
            }))
            .child(Meta::new(range).color(self.color))
            .child(nav(IconName::ChevronRight).on_press(move |_| {
                let p = *page.peek();
                if p < pages {
                    page.set(p + 1);
                }
            }))
    }
}
