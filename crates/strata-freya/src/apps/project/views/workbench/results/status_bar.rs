use super::ResultsState;
use crate::apps::project::views::workbench::results::selection::Selection;
use crate::components::divider::Divider;
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

/// The results-pane footer — present in *every* state (empty · running · grid). A state-coloured
/// dot + a state label sit on the left; the right region (snapshot chip · selection aggregate ·
/// pager) grows in with the query runtime + grid, so this is a real 40px bar now, not a stub.
///
/// Themed by `status_bar` (background · label colour · top divider · future hover). The state-dot
/// colour is **semantic** — read from the palette, not the component token — so it tracks the same
/// success/warning/error slots the rest of the app uses.
#[derive(PartialEq)]
pub struct StatusBar {
    state: ResultsState,
    pub theme: Option<StatusBarThemePartial>,
}

impl StatusBar {
    pub fn new(state: ResultsState) -> Self {
        Self { state, theme: None }
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
            }
        };
        let sel = consume_context::<State<Selection>>();

        // TODO: derive the label (+ subtext · snapshot · selection agg · pager) from the runs store
        // once the runtime layer lands. Until then the footer reflects the coarse view state.
        let label_text = match self.state {
            ResultsState::Empty => "No query run",
            ResultsState::Running => "Running…",
            ResultsState::Grid => "Results",
            ResultsState::ExplainPlan => "Query plan",
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
                    .maybe_child(sel_text.map(|text| Meta::new(text).color(color))),
            )
    }
}
