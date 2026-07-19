//! The project window **root shell** (rail · sidebar · workbench · drawer).
//!
//! Phase 1c (Session station slice): initialise this window's per-window Session store and prove
//! the tab-management model — new / duplicate / close / reopen / switch / dirty — reacts through
//! the Radio channels. The strip here is a **throwaway** text list; the real DS tab strip + the
//! `CodeEditor` bound to each tab's `Writable<CodeEditorData>` slice land in the next slice.
//!
//! The engine `Command -> Event -> rows` round-trip below is the earlier 1b proof, kept until the
//! query layer (freya-query) replaces it.

use freya::prelude::*;
use freya::radio::RadioStation;
use strata_core::engine::{Command, Event};
use strata_model::QueryOutput;

use crate::apps::project::contexts::engine_ctx::EngineCtx;
use crate::apps::project::state::{use_init_session, Chan, SessionState, TabId};

pub struct ProjectApp;

impl App for ProjectApp {
    fn render(&self) -> impl IntoElement {
        use_init_theme(|| crate::theme::strata_theme("midnight"));

        // This window's Session store (opens one blank tab), provided via context.
        let session = use_init_session();

        let mut result = use_state(|| Option::<QueryOutput>::None);
        let mut error = use_state(|| Option::<String>::None);

        use_provide_context(|| EngineCtx::new());

        // Spawn the engine once and drain its event stream into local state. (The query layer
        // promotes this to the freya-query router.) `spawn` is Freya's — it runs on the UI
        // executor, so writing state after `.await` is safe.
        use_hook(move || {
            let mut evt_rx = consume_context::<EngineCtx>().take_evt_rx();
            spawn(async move {
                while let Some(ev) = evt_rx.recv().await {
                    match ev {
                        Event::QueryResult {
                            result: Ok((out, _)),
                            ..
                        } => {
                            error.set(None);
                            result.set(Some(out));
                        }
                        Event::QueryResult {
                            result: Err(e), ..
                        } => error.set(Some(e)),
                        _ => {}
                    }
                }
            });
        });

        let on_run = move |_| {
            let engine = consume_context::<EngineCtx>();
            let req = engine.next_req();
            engine.send(Command::Query {
                req_id: req,
                ws_id: 1,
                sql: "SELECT 1 AS n, 'hello' AS greeting".to_string(),
                page_size: 100,
            });
        };

        let err = error.read().clone();
        let out = result.read().clone();

        rect()
            .expanded()
            .theme_background()
            .vertical()
            .child(session_strip(session))
            .child(Button::new().on_press(on_run).child("Run SELECT 1"))
            .maybe(err.is_some(), |el| {
                el.child(
                    label()
                        .text(err.clone().unwrap_or_default())
                        .color(Color::from_rgb(220, 80, 80)),
                )
            })
            .map(out, |el, out| el.child(results_table(&out)))
    }
}

/// Throwaway tab strip over the Session store: names (dirty-dotted), switch, close, + New,
/// Reopen. Reads the whole store (so it re-renders on any change) — the real strip will
/// subscribe per `Chan::Tab(id)`.
fn session_strip(mut session: RadioStation<SessionState, Chan>) -> impl IntoElement {
    let (rows, active, can_reopen) = {
        let s = session.read();
        let rows: Vec<(TabId, String, bool)> = s
            .order
            .iter()
            .map(|id| {
                let t = &s.tabs[id];
                (*id, t.name.clone(), t.is_dirty())
            })
            .collect();
        (rows, s.active, s.can_reopen())
    };

    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .padding(8.)
        .children(rows.into_iter().map(move |(id, name, dirty)| {
            let is_active = active == Some(id);
            let title = match (is_active, dirty) {
                (true, true) => format!("[* {name}]"),
                (true, false) => format!("[{name}]"),
                (false, true) => format!("* {name}"),
                (false, false) => name,
            };
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .child(
                    Button::new()
                        .on_press(move |_| {
                            session.write_channel(Chan::Tabs).switch(id);
                        })
                        .child(title),
                )
                .child(
                    Button::new()
                        .on_press(move |_| {
                            session.write_channel(Chan::Tabs).close_one(id);
                        })
                        .child("x"),
                )
                .into()
        }))
        .child(
            Button::new()
                .on_press(move |_| {
                    session.write_channel(Chan::Tabs).open_blank();
                })
                .child("+ New"),
        )
        .maybe(can_reopen, move |el| {
            el.child(
                Button::new()
                    .on_press(move |_| {
                        session.write_channel(Chan::Tabs).reopen_last();
                    })
                    .child("Reopen"),
            )
        })
}

/// A dead-simple columns + rows table over a query page (fixed-width text cells). The real
/// selectable/virtualized grid is a later slice.
fn results_table(out: &QueryOutput) -> impl IntoElement {
    rect()
        .vertical()
        .child(
            rect().horizontal().children(
                out.columns
                    .iter()
                    .map(|c| rect().width(Size::px(160.)).child(c.name.clone()).into()),
            ),
        )
        .children(out.rows.iter().map(|row| {
            rect()
                .horizontal()
                .children(
                    row.iter()
                        .map(|cell| rect().width(Size::px(160.)).child(cell.text.clone()).into()),
                )
                .into()
        }))
}
