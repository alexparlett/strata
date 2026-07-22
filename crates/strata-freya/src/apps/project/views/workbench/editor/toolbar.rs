use crate::apps::project::query::{QueryMode, QuerySpec, RunId, DEFAULT_PAGE_SIZE};
use crate::apps::project::state::{Chan, SessionState, TabId};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::run_button::{RunButton, RunState};
use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::use_radio;

/// The editor query toolbar, built to the comp. The bar itself only needs the editor surface (its
/// background) and the divider colour. The Run control is its own three-state `RunButton`; the rest
/// are outline [`Button`]s wrapping an icon (the rationalised button model — no bespoke IconButton).
///
/// Run / Explain / Analyze are wired: a press snapshots the tab's editor text into a fresh-nonce
/// [`QuerySpec`] in the workbench's `request` slot — the results pane's `use_query` picks it up
/// (state-arch §6). Run→Cancel while running, and the running / dirty / validation gates, land
/// with P2-15. The editing actions (Format / Clear / Save) are stubbed until their layers land.
#[derive(PartialEq)]
pub struct EditorToolbar {
    pub id: TabId,
    pub request: State<Option<QuerySpec>>,
}

impl Component for EditorToolbar {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        let theme = use_theme();
        let (bg, border) = {
            let t = theme.read();
            (t.colors.background, t.colors.border)
        };
        let radio = use_radio::<SessionState, Chan>(Chan::Tab(id));
        let mut request = self.request;

        // A press is an *action*: snapshot the editor text now, mint a fresh nonce, and
        // set it as the window's current execution. Blank buffers are a no-op (proper
        // disabled-state gating is P2-15). `read()` here is peek-equivalent: inside an
        // event handler there's no reactive context, so it cannot subscribe.
        let mut press = move |mode: QueryMode| {
            let sql = radio.read().tabs.get(&id).map(|t| t.text()).unwrap_or_default();
            if sql.trim().is_empty() {
                return;
            }
            request.set(Some(QuerySpec {
                tab: id,
                run: RunId::new(),
                sql,
                mode,
                page_size: DEFAULT_PAGE_SIZE,
            }));
        };

        // An outline icon button — `outline_button` variant with a centred icon. (Icon keeps its
        // resting tint on hover; Freya's Button doesn't cascade its hover colour into an SvgViewer.)
        let tool = move |icon: IconName| {
            Button::new()
                .height(Size::px(28.))
                .width(Size::px(28.))
                .child(Icon::new(icon).size(15.))
        };

        let row = rect()
            .width(Size::fill())
            .height(Size::px(38.))
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.)
            .padding((0., 10.))
            .background(bg)
            .child(
                RunButton::new(RunState::Idle).on_press(move |_| press(QueryMode::Run)),
            )
            .child(
                tool(IconName::Explain)
                    .on_press(move |_| press(QueryMode::Explain { analyze: false })),
            )
            .child(
                tool(IconName::Analyze)
                    .on_press(move |_| press(QueryMode::Explain { analyze: true })),
            )
            .child(Divider::vertical().length(Size::px(18.)).color(border))
            .child(tool(IconName::Format))
            .child(tool(IconName::Trash))
            .child(Divider::vertical().length(Size::px(18.)).color(border))
            .child(tool(IconName::Eye))
            .child(tool(IconName::Save));

        rect()
            .width(Size::fill())
            .vertical()
            .child(row)
            .child(Divider::horizontal().color(border))
    }
}
