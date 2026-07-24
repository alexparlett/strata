//! Live SQL validation (P2-18): a per-tab, debounced pass of the editor text through
//! the engine's dry-plan validator (`Engine::validate` — lexical lints, managed-DDL
//! policy, and parse → resolve → analyze against the live session, never executing).
//!
//! One settled pass lands in two places: **squiggles** as decorations inside the tab's
//! own `CodeEditorData` (the editor lines re-render off their existing buffer
//! subscription), and the tab's `diagnostics` on `Chan::Diagnostics(id)` (the Problems
//! feed, P3-12). Not freya-query: validation follows the buffer, so per-text cache
//! entries would only pile up — this is a cancel-and-rearm debounce, where cancelling
//! a mid-await pass *is* the supersede (a stale result is simply never applied).

use std::time::Duration;

use async_io::Timer;
use freya::prelude::{spawn, use_side_effect, use_state, TaskHandle, Writable, WritableUtils};
use freya::radio::use_radio;
use strata_code_editor::prelude::{CodeEditorData, DecorationSeverity};
use strata_model::{Diagnostic, Severity};

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{Chan, SessionState, TabId};

/// How long the buffer must sit quiet before a pass fires. Every text change cancels
/// and re-arms, so a typing burst validates once, on its settled text.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// The extra hold before **new** problems surface (~1s of quiet in total). Asymmetric
/// on purpose: a pass that only clears or keeps what's already shown applies at the
/// 300ms mark (fixes vanish fast), but one that would introduce a fresh squiggle
/// waits this much longer — half-typed SQL reads as broken constantly, and any
/// keystroke during the hold cancels the task before anything shows.
const SURFACE_DELAY: Duration = Duration::from_millis(700);

/// Drive validation for tab `id`: subscribe to its editor buffer, gate on the text
/// [`revision`](CodeEditorData::revision) (caret traffic never re-validates), and
/// apply each settled pass to the editor's decorations + the tab's diagnostics.
pub fn use_validation(id: TabId, editor: Writable<CodeEditorData>, engine: EngineCtx) {
    let radio = use_radio::<SessionState, Chan>(Chan::Diagnostics(id));
    // The armed (debouncing or in-flight) pass. Replaced wholesale on every change;
    // scope-bound like any spawn, so closing the tab cancels it too.
    let pending = use_state(|| None::<TaskHandle>);
    let last_revision = use_state(|| None::<u64>);

    use_side_effect(move || {
        // The read subscribes this effect to the tab's buffer channel.
        let revision = editor.read().revision();
        let mut last_revision = last_revision;
        if *last_revision.peek() == Some(revision) {
            return;
        }
        last_revision.set(Some(revision));

        if let Some(task) = *pending.peek() {
            task.cancel();
        }
        let engine = engine.clone();
        let mut editor = editor.clone();
        let task = spawn(async move {
            Timer::after(DEBOUNCE).await;
            // Text at fire time — any newer keystroke would have cancelled this task.
            let sql = editor.peek().rope.to_string();
            let diagnostics = engine.validate(sql).await;

            // New problems wait out the longer quiet window before showing; a pass
            // that only clears/keeps existing ones applies right away.
            let introduces_new = {
                let session = radio.read();
                let applied = session.diagnostics(id);
                diagnostics.iter().any(|d| !applied.contains(d))
            };
            if introduces_new {
                Timer::after(SURFACE_DELAY).await;
            }

            editor.write_if(|mut data| {
                data.set_decorations(diagnostics.iter().filter_map(|d| {
                    d.span
                        .clone()
                        .map(|span| (span, decoration_severity(d.severity), d.message.clone()))
                }))
            });
            let mut radio = radio;
            // Always write: even an identical list must re-stamp the revision it was
            // computed for — the Run gate compares it against the live buffer.
            radio
                .write_channel(Chan::Diagnostics(id))
                .set_diagnostics(id, diagnostics, revision);
        });
        let mut pending = pending;
        pending.set(Some(task));
    });
}

/// Severity → squiggle class (the editor colours it from its theme).
fn decoration_severity(severity: Severity) -> DecorationSeverity {
    match severity {
        Severity::Error => DecorationSeverity::Error,
        Severity::Warning => DecorationSeverity::Warning,
        Severity::Info => DecorationSeverity::Info,
    }
}
