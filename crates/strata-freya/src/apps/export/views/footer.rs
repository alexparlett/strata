//! The window's footer — the row count on the left, Cancel and Export on the right.
//!
//! **This is the only thing in the window that writes anything.** Export asks for a
//! destination, builds the spec, calls the engine, and closes on success. Cancel just closes;
//! nothing was committed, so there is nothing to undo.
//!
//! **No size estimate.** The canvas quotes "≈ 1.2 MB" here and above the preview, computed from
//! invented per-codec compression factors. A fabricated byte figure standing beside a real row
//! count is exactly what the column inspector rejected (P3-08), so the footer quotes the rows —
//! which were read from the run — and nothing else.
//!
//! **The destination is the native dialog.** A partitioned export writes a *directory*, so it
//! asks for a folder; everything else asks for a filename, pre-filled with the format's
//! extension and any compression suffix so what is offered matches what is written.

use freya::prelude::*;

use crate::apps::export::{
    model::thousands, ExportCtx, ExportThemePartial, ExportThemePreference, Status,
};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::{log_event, LogCtx, LogLevel};
use crate::components::divider::Divider;
use crate::components::metrics::ACTION_HEIGHT;
use crate::components::metrics::{SP_4, SP_5};
use crate::components::tones::tones;
use crate::components::typography::{Control, Path};
use strata_engine::EngineError;

/// The strip's inset (canvas `padding: var(--sp-4) var(--sp-5)`).
const FOOTER_PADDING: Gaps = Gaps::new(SP_4, SP_5, SP_4, SP_5);

#[derive(PartialEq)]
pub struct Footer;

impl Component for Footer {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let error = tones().error;
        let ctx = use_consume::<ExportCtx>();
        let engine = use_consume::<EngineCtx>();
        let log = use_consume::<LogCtx>();
        let platform = use_hook(Platform::get);

        let status = ctx.status.read().clone();
        let writing = status == Status::Writing;

        let meta = {
            let draft = ctx.draft.read();
            let target = ctx.target.read();
            let rows = match draft.scope {
                crate::apps::export::ScopeChoice::All => target.total,
                crate::apps::export::ScopeChoice::Page => {
                    let start = target.page.saturating_sub(1) * target.page_size;
                    target.total.saturating_sub(start).min(target.page_size)
                }
            };
            let unit = if rows == 1 { "row" } else { "rows" };
            let mut meta = format!("{} {unit}", thousands(rows));
            if draft.partition.is_active() {
                meta.push_str(" · directory");
            }
            meta
        };

        let left: Element = match &status {
            Status::Failed(message) => Path::new(message.clone())
                .color(error)
                .max_lines(2)
                .wrap()
                .into_element(),
            _ => Path::new(meta).color(theme.label_color).into_element(),
        };

        let cancel = {
            let platform = platform.clone();
            Button::new()
                .height(Size::px(ACTION_HEIGHT))
                .enabled(!writing)
                .on_press(move |_: Event<PressEventData>| platform.close_current_window())
                .child(Control::new("Cancel"))
        };

        let export = Button::new()
            .filled()
            .height(Size::px(ACTION_HEIGHT))
            .enabled(!writing)
            .on_press(move |_: Event<PressEventData>| {
                run_export(ctx, engine.clone(), log, platform.clone());
            })
            .child(Control::new(if writing {
                "Exporting…"
            } else {
                "Export"
            }));

        rect()
            .width(Size::fill())
            .vertical()
            .child(Divider::horizontal().color(theme.border_fill))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(SP_4)
                    .padding(FOOTER_PADDING)
                    .background(theme.background)
                    .child(rect().width(Size::flex(1.)).child(left))
                    .child(cancel)
                    .child(export),
            )
    }
}

/// Ask for a destination, then write. Spawned, because both halves wait: the dialog on the
/// user, the `COPY` on the engine.
fn run_export(mut ctx: ExportCtx, engine: EngineCtx, log: LogCtx, platform: Platform) {
    let (draft, target) = (ctx.draft.peek().clone(), ctx.target.peek().clone());
    let partitioned = draft.partition.is_active();
    let suggested = draft.suggested_name(&target);

    spawn(async move {
        let picked = if partitioned {
            rfd::AsyncFileDialog::new()
                .set_title("Choose a folder for the partitioned export")
                .pick_folder()
                .await
                .map(|handle| handle.path().join(suggested))
        } else {
            rfd::AsyncFileDialog::new()
                .set_title("Export results")
                .set_file_name(&suggested)
                .save_file()
                .await
                .map(|handle| handle.path().to_path_buf())
        };
        let Some(path) = picked else {
            return;
        };

        let spec = match draft.spec(&target, path.to_string_lossy().into_owned()) {
            Ok(spec) => spec,
            Err(why) => {
                ctx.status.set(Status::Failed(why));
                return;
            }
        };

        ctx.status.set(Status::Writing);
        match engine.snapshot(target.snapshot).export(spec).await {
            Ok(report) => {
                log_event(
                    log,
                    LogLevel::Ok,
                    format!(
                        "Exported {} rows to {}",
                        thousands(report.rows),
                        report.path
                    ),
                );
                platform.close_current_window();
            }
            Err(why) => {
                if !matches!(why, EngineError::Stopped(_)) {
                    log_event(log, LogLevel::Error, format!("Export failed: {why}"));
                }
                ctx.status.set(Status::Failed(why.to_string()));
            }
        }
    });
}
