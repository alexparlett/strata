use crate::apps::project::contexts::EngineCtx;
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
/// Run / Explain / Analyze are wired (P2-15): a press snapshots the tab's editor text into a
/// fresh-nonce [`QuerySpec`] in the workbench's `request` slot — the results pane's `use_query`
/// picks it up (state-arch §6). While that press is in flight (the `running` mirror holds its
/// nonce) Run wears its Cancel dress — pressing it aborts engine-side and drops the trigger,
/// the same action as the Running body's control. A blank buffer disables Run. The editing
/// actions (Format / Clear / Save) are stubbed until their layers land (P2-16), along with the
/// dirty / validation gates that come with them.
#[derive(PartialEq)]
pub struct EditorToolbar {
    pub id: TabId,
    pub request: State<Option<QuerySpec>>,
    /// The in-flight press's nonce, mirrored from the results body's query lifecycle (see
    /// `ResultsBody` — the toolbar must not subscribe the query itself).
    pub running: State<Option<RunId>>,
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
        let engine = use_consume::<EngineCtx>();
        let mut request = self.request;

        // This tab's press while it's still executing: the current request belongs to this
        // tab *and* the running mirror still holds its nonce (`request` alone can't tell —
        // it stays set after settle to keep the results body mounted).
        let in_flight = self
            .request
            .read()
            .as_ref()
            .filter(|s| s.tab == id && *self.running.read() == Some(s.run))
            .map(|s| s.run);

        // A blank buffer can't run — the button gates to Disabled. Subscribed on
        // `Chan::Tab(id)`, so typing re-derives it; `chars().all` early-exits on the first
        // real character (no rope→String materialise per keystroke).
        let blank = radio
            .read()
            .tabs
            .get(&id)
            .is_none_or(|t| t.editor.rope.chars().all(|c| c.is_whitespace()));

        // A press is an *action*: snapshot the editor text now, mint a fresh nonce, and
        // set it as the window's current execution. The blank guard backs up the visual
        // gate (Explain/Analyze share it). `read()` here is peek-equivalent: inside an
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

        let run_state = if in_flight.is_some() {
            RunState::Running
        } else if blank {
            RunState::Disabled
        } else {
            RunState::Idle
        };

        // Running → the press is Cancel: engine-side abort (tag-guarded, S14 — a stale press
        // can't kill a newer run) + drop the trigger, unmounting the results body back to
        // Empty. Otherwise it's Run. Disabled never fires (RunButton swallows it).
        let run_press = move |_| match in_flight {
            Some(run) => {
                engine.cancel(id.into(), run.into());
                request.set(None);
            }
            None => press(QueryMode::Run),
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
            .child(RunButton::new(run_state).on_press(run_press))
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
