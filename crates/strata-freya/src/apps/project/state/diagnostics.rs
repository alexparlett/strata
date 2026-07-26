//! The window's **one validation driver**: every open tab's diagnostics kept in step with the
//! two things they describe — the tab's text, and the catalog.
//!
//! ## Why one, and why here
//!
//! Validation used to be a hook inside `EditorTab`, which is mounted only for the tab on screen,
//! so a tab whose SQL arrived without being typed — restored at project open, reopened with
//! ⇧⌘T, opened from a saved query or to edit a view — was never validated at all, and its empty
//! diagnostics read as *clean* when they meant *nobody looked*. Rather than enumerate those
//! entry points (a list that goes stale the moment a new one appears), each tab records a
//! [`Stamp`] of what its diagnostics describe, and this driver reconciles stamps:
//! [`SessionState::stale_tabs`] is the whole work list, and a restored / reopened / duplicated
//! tab, an edited one, and one left behind by a pass a tab switch cancelled are all the same
//! thing — the stamp does not match.
//!
//! It is a **hook, not a component per tab**, because it needs only three subscriptions and they
//! are fixed: [`Chan::Text`] (the synthetic fan-in every `Chan::Tab(_)` write derives, so one
//! subscription watches *any* tab's buffer), [`Chan::Tabs`], and the catalog. A component per
//! tab would mean one `Chan::Tab(id)` subscription each — on revisions that, for a background
//! tab, cannot move, because nothing but a mounted editor writes a buffer — and N independent
//! debounces with no ordering between them.
//!
//! ## The catalog is a gate, not just an input
//!
//! `Engine::register` **deregisters before it re-infers**, so mid-scan `table_exist` is false
//! for every table being rebuilt. A pass then would report "not found" for tables sitting right
//! there. So while the catalog is `Scanning` nothing validates: no false
//! diagnostic is ever *produced*, rather than produced and retracted, and the squiggles already
//! on screen simply stay put rather than blanking. When the pass releases into a new epoch every
//! tab goes stale at once and is re-derived against the catalog it just built — which is how a
//! problem the user fixed in Table Config clears without them opening the tab.

use std::time::Duration;

use async_io::Timer;
use freya::prelude::{spawn, use_consume, use_side_effect, use_state, TaskHandle, WritableUtils};
use freya::radio::{use_radio, ChannelSelection, Radio};
use strata_code_editor::prelude::DecorationSeverity;
use strata_model::{Diagnostic, Severity, TabId};

use crate::apps::project::contexts::EngineCtx;

use super::catalog::use_catalog;
use super::{Chan, SessionState, Stamp};

/// How long the work list must sit still before a pass fires. Every wake cancels and re-arms,
/// so a typing burst validates once, on its settled text.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// The extra quiet a pass waits out before it may **introduce** problems (~1s of quiet in
/// total). See [`hold`].
const SURFACE_HOLD: Duration = Duration::from_millis(700);

/// How long a settled pass must wait before it may *introduce* problems the tab wasn't already
/// showing — `None` to apply immediately.
///
/// Half-written SQL reads as broken constantly, so a pass that would *add* something holds for a
/// further beat, and any keystroke inside it cancels the task before anything shows. Clearing or
/// keeping what is already on screen never waits: fixes land fast.
///
/// It applies to **typing only**. A tab being looked at for the first time (`previous` is
/// `None` — restored, reopened, opened from a view) is not half-written, and neither is a
/// re-check after the catalog moved (`previous` at the same revision); holding either would just
/// delay the truth by 700ms for nobody's benefit.
pub fn hold(previous: Option<Stamp>, revision: u64, introduces_new: bool) -> Option<Duration> {
    let typed = previous.is_some_and(|s| s.revision != revision);
    (typed && introduces_new).then_some(SURFACE_HOLD)
}

/// Drive validation for every open tab. Call once in the window root, after the engine, the
/// project (which provides the catalog) and the session are in place.
pub fn use_diagnostics() {
    let engine = use_consume::<EngineCtx>();
    // Any tab's buffer. `Chan::Text` is the fan-in that makes this one subscription rather than
    // one per tab — see the module note. Doubles as this driver's handle onto the store: the
    // channel is named explicitly at every write, so which one it was created with only decides
    // what wakes the effect.
    let session = use_radio::<SessionState, Chan>(Chan::Text);
    // A tab opened, closed, reopened, switched, or renamed.
    let strip = use_radio::<SessionState, Chan>(Chan::Tabs);
    let catalog = use_catalog();
    // The armed (debouncing or running) pass. Replaced wholesale on every wake; scope-bound
    // like any `spawn`, so closing the window cancels it.
    let pending = use_state(|| None::<TaskHandle>);

    use_side_effect(move || {
        // The three subscriptions. Read for the side effect of subscribing — what to do is
        // decided from the peeked store below, so a wake that changed nothing costs a
        // `stale_tabs` walk and returns.
        let (_, _) = (session.read(), strip.read());
        // Cancellation *is* the supersede: a task dropped mid-await never applies its answer,
        // so there is no stale-result check to get wrong.
        let mut supersede = || {
            if let Some(task) = *pending.peek() {
                task.cancel();
            }
        };

        let Some(epoch) = catalog.read().epoch() else {
            // Mid-scan: the gate. **The armed pass goes with it** — `Engine::register`
            // deregisters before it re-infers, so a pass that fires now resolves against a
            // catalog mid-teardown and stamps "not found" on tables that are sitting right
            // there, against an epoch that is already spent. Returning without superseding is
            // what would make the gate produce-and-retract instead of never produce.
            supersede();
            return;
        };

        let work = session.read().stale_tabs(epoch);
        if work.is_empty() {
            return;
        }

        supersede();
        let engine = engine.clone();
        let task = spawn(async move {
            Timer::after(DEBOUNCE).await;
            // Serial, and `stale_tabs` put the active tab first: at project open with twenty
            // restored tabs everything is stale at once, and twenty concurrent dry plans would
            // queue on the engine's two workers ahead of the user's first Run.
            for id in work {
                pass(session, &engine, id, epoch).await;
            }
        });
        let mut pending = pending;
        pending.set(Some(task));
    });
}

/// One tab's pass: validate what it holds *now*, then apply the verdict to the tab and its
/// buffer. A tab closed while the pass ran simply doesn't take the answer.
async fn pass(mut session: Radio<SessionState, Chan>, engine: &EngineCtx, id: TabId, epoch: u64) {
    // Text, revision and prior verdict read together, so the stamp written below names exactly
    // the text that was validated. Taken *after* the debounce, which is why a burst validates
    // its settled text rather than the text that armed it.
    // `read` outside a reactive context is peek-equivalent — a task has none, so this
    // subscribes nothing.
    let Some((sql, revision, previous, shown)) = session.read().tabs.get(&id).map(|t| {
        (
            t.text(),
            t.editor.revision(),
            t.validated,
            t.diagnostics.clone(),
        )
    }) else {
        return;
    };

    let diagnostics = engine.validate(sql).await;

    let introduces_new = diagnostics.iter().any(|d| !shown.contains(d));
    if let Some(wait) = hold(previous, revision, introduces_new) {
        Timer::after(wait).await;
    }

    // The squiggles, into the tab's own buffer — including a tab with no editor mounted, so
    // switching to it shows them immediately instead of a beat later. Silenced when the
    // decorations are unchanged: `Chan::Tab(id)` wakes the editor, autosave *and* this driver,
    // and a repeat pass has nothing for any of them.
    session.write_with_channel_selection(|state| {
        let changed = state
            .tabs
            .get_mut(&id)
            .is_some_and(|tab| tab.editor.set_decorations(decorations(&diagnostics)));
        match changed {
            true => ChannelSelection::Select(Chan::Tab(id)),
            false => ChannelSelection::Silence,
        }
    });
    session.write_channel(Chan::Diagnostics).set_diagnostics(
        id,
        Stamp { revision, epoch },
        diagnostics,
    );
}

/// The spanned diagnostics, as the editor's decoration layer wants them. An unspanned one can't
/// squiggle, so it lists in Problems and nowhere else.
fn decorations(
    diagnostics: &[Diagnostic],
) -> Vec<(std::ops::Range<usize>, DecorationSeverity, String)> {
    diagnostics
        .iter()
        .filter_map(|d| {
            d.span
                .clone()
                .map(|span| (span, severity(d.severity), d.message.clone()))
        })
        .collect()
}

/// Severity → squiggle class (the editor colours it from its theme).
fn severity(severity: Severity) -> DecorationSeverity {
    match severity {
        Severity::Error => DecorationSeverity::Error,
        Severity::Warning => DecorationSeverity::Warning,
        Severity::Info => DecorationSeverity::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Typing is the only thing the surface hold protects against, and only when the pass would
    /// *add* something. Everything else applies at the debounce.
    #[test]
    fn the_surface_hold_is_for_typing_that_introduces_problems() {
        let was = Some(Stamp {
            revision: 7,
            epoch: 3,
        });

        assert_eq!(
            hold(was, 8, true),
            Some(SURFACE_HOLD),
            "the user typed and the pass found something new — wait it out"
        );
        assert_eq!(
            hold(was, 8, false),
            None,
            "typing that only clears or keeps applies at once, so fixes land fast"
        );
        assert_eq!(
            hold(was, 7, true),
            None,
            "same text, new catalog epoch: not half-written, so no hold"
        );
        assert_eq!(
            hold(None, 7, true),
            None,
            "a first look at a restored tab is not half-written either"
        );
    }
}
