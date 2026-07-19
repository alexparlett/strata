//! The project window **root shell** (rail · sidebar · workbench · drawer).
//!
//! Phase 1b (bridge slice): spawn the shared engine, drain its event stream, and prove the
//! `Command -> Event -> rows` round-trip with a hardcoded query. The SQL input + per-window
//! Radio station (`state/`) + the extracted `views/workbench` land in the next slice; the
//! rail/sidebar/inspector/drawer follow in later phases.

use freya::prelude::*;
use strata_core::engine::{Command, Event};
use strata_model::QueryOutput;
use crate::apps::project::contexts::engine_ctx;
use crate::apps::project::contexts::engine_ctx::EngineCtx;

pub struct ProjectApp;

impl App for ProjectApp {
    fn render(&self) -> impl IntoElement {
        use_init_theme(|| crate::theme::strata_theme("midnight"));

        let mut result = use_state(|| Option::<QueryOutput>::None);
        let mut error = use_state(|| Option::<String>::None);

        use_provide_context(|| EngineCtx::new());

        // Spawn the engine once and drain its event stream into local state. (1b's next
        // slice promotes this to the per-window Radio station.) `spawn` is Freya's, not
        // Tokio's — it runs on the UI executor, so writing state after `.await` is safe.
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

/// A dead-simple columns + rows table over a query page (fixed-width text cells). The real
/// selectable/virtualized grid is Phase 2.
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
