//! **Test, and the model list — which are one call.**
//!
//! There is no ping in `genai` and there does not need to be: listing a provider's models is a
//! live request against its endpoint with its configured credential, which is exactly what a
//! connection test proves, and its answer is exactly what a model picker needs. A separate
//! reachability probe would be a second round trip proving strictly less, and two results that
//! could disagree about one provider.
//!
//! ## One request, two things to keep
//!
//! [`refresh`] is the whole mechanism, and every gesture that fetches a list is a call into it:
//! the Test press in the Configure dialog, and the staleness kick a model picker makes when it
//! opens. What comes back is kept in two places that are not two caches:
//!
//! - the **names** go to the app-global listings satellite
//!   ([`Listings`](strata_core::models::Listings)), because they outlive this window and this
//!   run of the app — a picker fed only by a live call is empty at every launch;
//! - the **outcome** stays here as a [`Probe`], because "a request is in flight", "it came back"
//!   and "it failed, and here is what the provider said" are facts about a request the user just
//!   made, and a "verified" restored from disk at launch would be a claim nothing had checked.
//!
//! So the probe carries a count and never a list. A `Probe::Verified` holding the names again
//! would be the second cache the design is arranged to avoid, and the two could disagree.
//!
//! **Editing a credential retracts both.** A "verified" beside a key that has since been retyped
//! is a claim about a request that was never made — the same "only real facts" rule the row's
//! subline follows — and the names it returned describe an endpoint nobody would call now.
//! [`SettingsCtx::forget_provider`] is that retraction, in one line, so neither can be dropped
//! without the other.
//!
//! ## Where the work runs
//!
//! On a thread of its own ([`crate::task::offload`]), because a keystore read and an HTTP round
//! trip are both things the render thread must never wait on. What drives the request is
//! `provider::list_models_blocking` — over in the crate that owns `genai`, so a runtime and an
//! HTTP client are never things this frontend has to carry. One gesture, one thread, one
//! request, torn down after; a window that closes mid-fetch drops the answer.

use std::collections::BTreeMap;

use freya::prelude::spawn_forever;
use strata_agent::assistant::list_models_blocking;
use strata_core::ai::ProviderKind;
use strata_core::secret::{Secret, SecretRef};

use crate::apps::settings::SettingsCtx;
use crate::state::write_listings;
use crate::task::offload;

/// What is known about one brain's endpoint, from having actually asked it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum Probe {
    /// Never tested, or retracted by an edit. The row shows what it knows without asking.
    #[default]
    Untested,
    /// A request is in flight.
    Testing,
    /// The provider answered, with this many models. **A count and not the list** — the names
    /// are the satellite's ([`refresh`]), and a second copy here is a second cache. Zero is a
    /// real answer and a different one from a failure: the endpoint is reachable and serves
    /// nothing.
    Verified { count: usize },
    /// The provider's own words, already bounded by `list_models`.
    Failed { why: String },
}

impl Probe {
    /// The status line under the credential row, and the tone to paint it.
    ///
    /// `None` while untested: a row that has never been asked has nothing to report, and
    /// "unknown" printed under every provider is noise rather than information.
    pub fn status(&self) -> Option<(Tone, String)> {
        match self {
            Probe::Untested => None,
            Probe::Testing => Some((Tone::Working, "testing…".into())),
            Probe::Verified { count } => Some((
                Tone::Good,
                match count {
                    0 => "connection verified, no models offered".into(),
                    1 => "connection verified, 1 model".into(),
                    n => format!("connection verified, {n} models"),
                },
            )),
            Probe::Failed { why } => Some((Tone::Bad, why.clone())),
        }
    }
}

/// Which semantic tone a status line takes. The shared `tones()` hook resolves it — a pane does
/// not own success/warning/error colours (AGENTS.md §3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Working,
    Good,
    Bad,
}

/// Every provider's probe, for as long as the window is open.
///
/// **On the window, not on the pane**, for the reason [`PropRows`](crate::apps::settings::views::PropRows)
/// is: AI ▸ Providers runs the test and AI ▸ Chat reports what it said, and a result thrown away
/// by navigating between the two would leave the model picker unable to say why it has nothing
/// to offer — and would let it start the same failing request again on every visit.
///
/// Not persisted, and deliberately: a verification is a fact about a request made minutes ago,
/// and a "verified" restored from disk at launch would be a claim nothing had checked. The
/// *names* that request returned are persisted, in the satellite — see the module doc.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Probes(BTreeMap<ProviderKind, Probe>);

impl Probes {
    pub fn get(&self, kind: ProviderKind) -> &Probe {
        static UNTESTED: Probe = Probe::Untested;
        self.0.get(&kind).unwrap_or(&UNTESTED)
    }

    pub fn set(&mut self, kind: ProviderKind, probe: Probe) {
        self.0.insert(kind, probe);
    }

    /// Retract what is known about `kind` — its credential or endpoint just changed, so the
    /// last answer describes a request nobody would make now.
    pub fn forget(&mut self, kind: ProviderKind) {
        self.0.remove(&kind);
    }
}

/// Everything a probe needs, copied out of the draft before the thread starts.
///
/// Owned rather than borrowed because it crosses a thread boundary, and it carries the
/// [`SecretRef`] rather than the secret: the keystore read happens **on that thread**, so a
/// pasted key that has not been stored yet is passed as itself and one already stored is never
/// held by the pane at all. Exposure is managed by lifetime, which is `strata_core::secret`'s
/// own rule.
pub struct Ask {
    pub kind: ProviderKind,
    pub base_url: String,
    /// A key typed into the draft and not yet committed. Takes precedence over `stored`: it is
    /// what Apply would write, so it is what a test should prove.
    pub typed: Option<Secret>,
    /// The key already in the keystore, if the draft has not replaced it.
    pub stored: Option<SecretRef>,
}

impl Ask {
    /// What the window currently holds for `kind` — the ask a surface makes when it is not the
    /// Configure dialog.
    ///
    /// The dialog builds its own from its boxes, because it tests what is on screen rather than
    /// what is filed. Everywhere else — a picker refreshing a stale list — the draft *is* what
    /// is on screen, and a pending key beats the stored marker for the dialog's own reason: it
    /// is what Apply would send.
    ///
    /// `peek` throughout: this is called from an effect and from event handlers, and
    /// subscribing them to the whole draft would re-run every one of them on any keystroke in
    /// the window.
    pub fn from_draft(ctx: SettingsCtx, kind: ProviderKind) -> Ask {
        let keys = ctx.ai_keys.peek();
        let pending = keys.touched(kind);
        Ask {
            kind,
            base_url: ctx.base_url_of(kind),
            typed: Secret::new(keys.get(kind)),
            // An entry that is *touched* and empty is a pending removal, so falling back to the
            // stored marker there would authenticate with a key on its way out — the same trap
            // the dialog's Test names.
            stored: (!pending)
                .then(|| {
                    ctx.draft
                        .peek()
                        .ai
                        .setup(kind)
                        .and_then(|setup| setup.key.clone())
                })
                .flatten(),
        }
    }
}

/// **Ask the provider what it serves, and keep both halves of the answer.**
///
/// The one mechanism every fetching gesture calls: the Configure dialog's Test press, the enable
/// toggle, and a picker's staleness kick. Names to the satellite, outcome to the [`Probe`] — see
/// the module doc for why those are two places and not two caches.
///
/// **Re-entrant by design and guarded once, here.** A request already in flight for this kind is
/// left alone rather than raced, so a Test press during a background refresh (or a second press)
/// cannot land two answers out of order. The guard is taken *before* the task exists, so two
/// gestures in one frame cannot both get past it.
///
/// **It spawns the task itself, and on the window rather than on the caller.** A plain `spawn`
/// binds the future to the scope that made it, so a dialog closed mid-test or a pane navigated
/// away from would drop the request and leave the probe reading `Testing` for the rest of the
/// window's life — a row saying "testing…" about nothing, and a kind no further refresh can get
/// past its own guard. The request belongs to the window, whose state it settles into: it must
/// outlive the surface that asked and die with the window, which is exactly `spawn_forever`'s
/// scope. That is also why this is not an `async fn` the caller awaits — the lifetime rule and
/// the guard belong together in the funnel, where no new call site can forget either.
pub fn refresh(ctx: SettingsCtx, ask: Ask) {
    let kind = ask.kind;
    let mut probes = ctx.probes;
    if matches!(probes.peek().get(kind), Probe::Testing) {
        return;
    }
    probes.write().set(kind, Probe::Testing);

    spawn_forever(async move {
        let settled = match run(ask).await {
            Ok(models) => {
                let count = models.len();
                // **The names first, then the outcome.** The satellite is what a picker reads,
                // so writing it before the probe means the count and the list can never be seen
                // disagreeing in a repaint between the two.
                write_listings(ctx.listings, |listings| listings.set(kind, models));
                Probe::Verified { count }
            }
            Err(why) => Probe::Failed { why },
        };

        let mut probes = ctx.probes;
        probes.write().set(kind, settled);
    });
}

/// Ask the provider what it serves, off the render thread.
///
/// The provider's own words on a failure, already bounded by `list_models`.
async fn run(ask: Ask) -> Result<Vec<String>, String> {
    let answer = offload(move || {
        // The key is read here, on the worker, and lives exactly as long as the request. A
        // keystore call blocks — which is the other half of why this is not on the render
        // thread — and a `SecretRef` that resolves to nothing is not an error: it means no key
        // is set, which `list_models` then answers for the kind in its own words.
        let key = match ask.typed {
            Some(typed) => Some(typed),
            None => match ask.stored.as_ref().map(SecretRef::get) {
                Some(Ok(secret)) => secret,
                Some(Err(e)) => return Err(e.to_string()),
                None => None,
            },
        };

        let base_url = (!ask.base_url.trim().is_empty()).then_some(ask.base_url.as_str());
        list_models_blocking(ask.kind, base_url, key.as_ref())
    })
    .await;

    match answer {
        Some(answer) => answer,
        // The thread never answered — it could not start, or it panicked. Neither is a fact
        // about the provider, so this must not claim to have reached one.
        None => Err("The test could not be run.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe nobody has run says nothing, and a probe that came back empty says so — the two
    /// are different answers and only one of them is about the provider.
    #[test]
    fn an_untested_row_reports_nothing_and_an_empty_answer_reports_itself() {
        assert!(Probe::Untested.status().is_none());
        let (tone, said) = Probe::Verified { count: 0 }.status().unwrap();
        assert_eq!(tone, Tone::Good);
        assert!(said.contains("no models"), "{said}");
    }

    /// The count is what the row shows, so it is not pluralized by hand at the call site.
    #[test]
    fn a_verified_probe_counts_what_came_back() {
        let one = Probe::Verified { count: 1 };
        assert_eq!(one.status().unwrap().1, "connection verified, 1 model");
        let two = Probe::Verified { count: 2 };
        assert_eq!(two.status().unwrap().1, "connection verified, 2 models");
    }

    /// **An edit retracts the answer.** A key retyped after a successful test leaves a claim
    /// about a request nobody would make now.
    #[test]
    fn editing_a_credential_forgets_what_was_verified() {
        let kind = ProviderKind::Anthropic;
        let mut probes = Probes::default();
        probes.set(kind, Probe::Verified { count: 4 });
        assert!(probes.get(kind).status().is_some());

        probes.forget(kind);
        assert_eq!(probes.get(kind), &Probe::Untested);
        assert!(probes.get(kind).status().is_none());
    }

    /// A provider nobody has touched reads as untested rather than as missing, so no caller has
    /// to branch on presence.
    #[test]
    fn an_unknown_provider_reads_as_untested() {
        assert_eq!(Probes::default().get(ProviderKind::Groq), &Probe::Untested);
    }
}
