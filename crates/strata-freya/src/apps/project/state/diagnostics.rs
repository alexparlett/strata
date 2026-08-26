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
//! tab, an edited one, and one whose verdict was dropped for describing a world that had already
//! moved are all the same thing — the stamp does not match.
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
//! A pass applies the catalog **row by row**, so mid-scan it is a real half-applied state: a def
//! that has not registered yet is genuinely not found, and one the pass is about to remove is
//! still there. So while the catalog is `Scanning` nothing validates: no diagnostic about a state
//! that never persists is *produced*, rather than produced and retracted, and the squiggles
//! already on screen simply stay put rather than blanking. A table being *rebuilt* is not one of
//! those states: `Catalog::register` builds the new provider aside and swaps it in, so a name
//! never stops resolving. When the pass releases into a new epoch every
//! tab goes stale at once and is re-derived against the catalog it just built — which is how a
//! problem the user fixed in Table Config clears without them opening the tab.

use std::time::Duration;

use async_io::Timer;
use freya::prelude::{spawn, use_consume, use_side_effect, use_state, State, WritableUtils};
use freya::radio::{use_radio, ChannelSelection, Radio};
use strata_code_editor::prelude::DecorationSeverity;
use strata_model::{Diagnostic, Severity, TabId};

use crate::apps::project::contexts::EngineCtx;

use super::catalog::{use_catalog, Catalog};
use super::{Chan, SessionState, Stamp};

/// How long a tab's text must sit still before it is validated — see [`settle`]. A typing burst
/// therefore validates once, on the text the user stopped at, not once per window.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// The extra quiet a pass waits out before it may **introduce** problems (~1s of quiet in
/// total). See [`hold`].
const SURFACE_HOLD: Duration = Duration::from_millis(700);

/// How long a settled pass must wait before it may *introduce* problems the tab wasn't already
/// showing — `None` to apply immediately.
///
/// Half-written SQL reads as broken constantly, so a pass that would *add* something holds for a
/// further beat — and a keystroke inside that beat moves the tab's revision, so the verdict is
/// dropped unapplied and re-derived. Clearing or keeping what is already on screen never waits:
/// fixes land fast.
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
    let session = use_radio::<SessionState, Chan>(Chan::Text);
    let strip = use_radio::<SessionState, Chan>(Chan::Tabs);
    let catalog = use_catalog();
    let draining = use_state(|| false);

    use_side_effect(move || {
        let (_, _) = (session.read(), strip.read());
        let Some(epoch) = catalog.read().epoch() else {
            return;
        };
        if *draining.peek() || session.read().stale_tabs(epoch).is_empty() {
            return;
        }

        let engine = engine.clone();
        let mut draining = draining;
        draining.set(true);
        spawn(async move {
            let _guard = Draining(draining);
            loop {
                let Some(epoch) = catalog.peek().epoch() else {
                    break;
                };
                let Some(id) = session.read().stale_tabs(epoch).into_iter().next() else {
                    break;
                };
                settle(session, id).await;
                pass(session, catalog, &engine, id, epoch).await;
            }
        });
    });
}

/// Wait until `id`'s text has stopped moving — a full [`DEBOUNCE`] with no new revision — so a
/// typing burst validates **once**, on the text the user actually settled on, rather than every
/// `DEBOUNCE` for the length of the burst.
///
/// Only the **active** tab is waited on, and returns immediately for anything else. Nothing but
/// a mounted editor writes a buffer, so a background tab's revision cannot move: making it wait
/// would buy nothing and cost `DEBOUNCE` *per restored tab* at project open, which is exactly the
/// serial-drain latency the loop is arranged to avoid.
async fn settle(session: Radio<SessionState, Chan>, id: TabId) {
    loop {
        let before = {
            let s = session.read();
            if s.active != Some(id) {
                return;
            }
            match s.tabs.get(&id) {
                Some(tab) => tab.editor.revision(),
                None => return,
            }
        };
        Timer::after(DEBOUNCE).await;
        let quiet = session
            .read()
            .tabs
            .get(&id)
            .is_none_or(|tab| tab.editor.revision() == before);
        if quiet {
            return;
        }
    }
}

/// Marks a drain as running for as long as it is alive. On `Drop` — settled *or* cancelled —
/// the flag clears, so the effect can start the next one.
struct Draining(State<bool>);

impl Drop for Draining {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// One tab's pass: validate what it holds *now*, then apply the verdict to the tab and its
/// buffer. A tab closed while the pass ran simply doesn't take the answer.
async fn pass(
    mut session: Radio<SessionState, Chan>,
    catalog: Catalog,
    engine: &EngineCtx,
    id: TabId,
    epoch: u64,
) {
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

    let diagnostics = engine.lang().validate(sql).await;

    let introduces_new = diagnostics.iter().any(|d| !shown.contains(d));
    if let Some(wait) = hold(previous, revision, introduces_new) {
        Timer::after(wait).await;
    }

    let moved_on = session
        .read()
        .tabs
        .get(&id)
        .is_none_or(|t| t.editor.revision() != revision);
    if moved_on || catalog.peek().epoch() != Some(epoch) {
        return;
    }

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
