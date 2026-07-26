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
use crate::components::divider::Divider;
use crate::components::typography::{Control, Path};
use crate::components::ACTION_HEIGHT;

/// The strip's inset (canvas `padding: var(--sp-4) var(--sp-5)`).
const FOOTER_PADDING: Gaps = Gaps::new(12., 16., 12., 16.);

#[derive(PartialEq)]
pub struct Footer;

impl Component for Footer {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let error = use_theme().read().colors().error;
        let ctx = use_consume::<ExportCtx>();
        let engine = use_consume::<EngineCtx>();
        let platform = use_hook(Platform::get);

        let status = ctx.status.read().clone();
        let writing = status == Status::Writing;

        // What the export will actually write — the scope's own count, not the grid's.
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

        // A failure replaces the row count: it is the more important thing on the strip, and
        // the window is the only place that can explain it.
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
                run_export(ctx, engine.clone(), platform.clone());
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
                    .spacing(12.)
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
fn run_export(mut ctx: ExportCtx, engine: EngineCtx, platform: Platform) {
    let (draft, target) = (ctx.draft.peek().clone(), ctx.target.peek().clone());
    let partitioned = draft.partition.is_active();
    let suggested = draft.suggested_name(&target);

    spawn(async move {
        // A partitioned export builds a tree, so it needs a folder to build it in; every other
        // format writes one file and needs its name.
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
        // Dismissing the dialog is a decision, not a failure — nothing to report.
        let Some(path) = picked else {
            return;
        };

        // The spec is built here, after the destination is known, so a bad delimiter is
        // reported before anything is written rather than as a `COPY` parse error.
        let spec = match draft.spec(&target, path.to_string_lossy().into_owned()) {
            Ok(spec) => spec,
            Err(why) => {
                ctx.status.set(Status::Failed(why));
                return;
            }
        };

        ctx.status.set(Status::Writing);
        match engine.export(target.snapshot, spec).await {
            // Done: the file is on disk and there is nothing left to decide here. (The
            // confirmation belongs in the Events drawer — P3-13 — which is where every other
            // completed action will report; there is no toast surface yet, and inventing one
            // for this window alone is the sort of local fold that later has to be undone.)
            Ok(_) => platform.close_current_window(),
            Err(why) => ctx.status.set(Status::Failed(why)),
        }
    });
}
