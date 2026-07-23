use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{QueryMode, RunId};
use crate::apps::project::state::{Chan, ProjChan, ProjectState, SessionState, TabId};
use crate::apps::project::views::workbench::editor::actions;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::run_button::{RunButton, RunState};
use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::{use_radio, use_radio_station};

/// The editor query toolbar, built to the comp. The bar itself only needs the editor surface (its
/// background) and the divider colour. The Run control is its own three-state `RunButton`; the rest
/// are outline [`Button`]s wrapping an icon (the rationalised button model — no bespoke IconButton).
///
/// Run / Explain / Analyze are wired (P2-15): a press snapshots the tab's editor text into a
/// fresh-nonce `QuerySpec` in the tab's own `request` slot (`QueryTab::request`, written on
/// `Chan::Request`) — the results pane's `use_query` picks it up (state-arch §6). While that
/// press is in flight (the `running` mirror holds its nonce) Run wears its Cancel dress —
/// pressing it aborts engine-side and drops the trigger, the same action as the Running
/// body's control. A blank buffer disables Run.
///
/// The editing actions are wired to [`actions`] (P2-16): Format / Clear rewrite the buffer
/// (history-tracked); Eye saves the buffer as a new `saved_view_N` catalog view; Save is the
/// dispatch-on-origin (view → `CREATE OR REPLACE VIEW`, saved query → upsert by id,
/// scratch → new saved query under the tab's name).
#[derive(PartialEq)]
pub struct EditorToolbar {
    pub id: TabId,
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
        // The Project store — save-target access only, so no channel subscription (the
        // toolbar shows nothing catalog-derived).
        let project = use_radio_station::<ProjectState, ProjChan>();
        // The tab's Run trigger, on its own channel — a press re-renders this toolbar
        // without waking the editor, and keystrokes (on `Chan::Tab`) never land here twice.
        let request_radio = use_radio::<SessionState, Chan>(Chan::Request(id));

        // This tab's request while it's still executing: the tab has a request *and* the
        // running mirror still holds its nonce (the request alone can't tell — it stays set
        // after settle to keep the results body mounted).
        let in_flight = request_radio
            .read()
            .request(id)
            .filter(|s| *self.running.read() == Some(s.run))
            .map(|s| s.run);

        // A blank buffer can't run — the button gates to Disabled. Subscribed on
        // `Chan::Tab(id)`, so typing re-derives it; `chars().all` early-exits on the first
        // real character (no rope→String materialise per keystroke).
        let blank = radio
            .read()
            .tabs
            .get(&id)
            .is_none_or(|t| t.editor.rope.chars().all(|c| c.is_whitespace()));

        // A press is an *action* — `actions::press_query` snapshots the text, mints a
        // fresh nonce, and sets the tab's current execution; the ⌘↵ listener in the
        // workbench dispatches the very same call.
        let press = move |mode: QueryMode| actions::press_query(radio, id, mode);

        let run_state = if in_flight.is_some() {
            RunState::Running
        } else if blank {
            RunState::Disabled
        } else {
            RunState::Idle
        };

        // The save actions' handles (the engine is moved into `run_press` below).
        let save_engine = engine.clone();
        let view_engine = engine.clone();

        // Running → the press is Cancel (`actions::cancel_run` — shared with the Running
        // body's control and Esc). Otherwise it's Run. Disabled never fires (RunButton
        // swallows it).
        let run_press = move |_| match in_flight {
            Some(run) => actions::cancel_run(&engine, radio, id, run),
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
            .child(tool(IconName::Format).on_press(move |_| actions::format(radio, id)))
            .child(tool(IconName::Trash).on_press(move |_| actions::clear(radio, id)))
            .child(Divider::vertical().length(Size::px(18.)).color(border))
            .child(tool(IconName::Eye).on_press(move |_| {
                actions::save_as_view(radio, project, view_engine.clone(), id)
            }))
            .child(tool(IconName::Save).on_press(move |_| {
                actions::save(radio, project, save_engine.clone(), id)
            }));

        rect()
            .width(Size::fill())
            .vertical()
            .child(row)
            .child(Divider::horizontal().color(border))
    }
}
