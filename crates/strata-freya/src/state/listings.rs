//! The app-global **model listings** — what each provider last reported, for as long as the
//! app is installed rather than as long as a window is open.
//!
//! **App-global** because two surfaces pick from this list and neither owns it: Settings ▸ AI ▸
//! Chat picks what a new chat starts on, and the composer picks per conversation. A list held by
//! either window would be empty in the other and empty at the next launch.
//!
//! **Disk is a startup input**, exactly as for the config store: [`create_global_listings`] reads
//! the file once in `main`, [`write_listings`] is the only writer.
//!
//! Distinct from [`Probes`], which is *not* persisted and must not be: a probe is the state of a
//! request the user made minutes ago, and a "verified" restored from disk would be a claim nothing
//! had checked. A listing is the answer that request returned.
//!
//! **[`refresh`] is the whole mechanism** — the Test press in Settings' Configure dialog and the
//! staleness kick a model picker makes when it opens are both calls into it. There is no separate
//! ping and there does not need to be: listing a provider's models is a live request with the
//! configured credential, which is what a connection test proves. Here rather than in Settings,
//! because the composer needs the same funnel with no `SettingsCtx` in reach, and the in-flight
//! guard, the two keeps and the retraction rule are exactly what a second copy would get subtly
//! different.
//!
//! What comes back is kept in two places that are **not** two caches: the **names** go to the
//! app-global satellite because they outlive this window and this run of the app, and the
//! **outcome** stays in a [`Probes`] slot owned by whichever window asked. So the probe carries a
//! count and never a list — a [`Probe::Verified`] holding the names again is the second cache this
//! design avoids.
//!
//! **Editing a credential retracts both** through `SettingsCtx::forget_provider`, in one line, so
//! neither can be dropped without the other: a "verified" beside a retyped key is a claim about a
//! request that was never made.
//!
//! The work runs on a thread of its own ([`crate::task::offload`]) — a keystore read and an HTTP
//! round trip are both things the render thread must never wait on — driven by
//! `provider::list_models_blocking`, over in the crate that owns `genai`.

use std::collections::BTreeMap;

use freya::prelude::{spawn_forever, State, TaskHandle};
use strata_agent::assistant::list_models_blocking;
use strata_core::ai::{Ai, ProviderKind};
use strata_core::models::{self, Listings};
use strata_core::secret::{Secret, SecretRef};

use crate::task::offload;

/// The app-global listings slot — created in `main` and handed to every window root.
pub type ModelListings = State<Listings>;

/// The app-global probes slot, beside it — see [`Probes`].
pub type ProviderProbes = State<Probes>;

/// Create the probes slot. Call **once**, in `main`, like its neighbours.
pub fn create_global_probes() -> ProviderProbes {
    State::create_global(Probes::default())
}

/// Load the satellite into the one app-global slot. Call **once**, in `main`, before `launch`
/// — not a hook.
///
/// The read is synchronous and blocking, like [`config::load`](strata_core::config::load) two
/// lines above it: there is no event loop yet to hold up, and this is a small JSON file in the
/// user's own config directory rather than a project on a mount that may have stopped
/// answering.
pub fn create_global_listings() -> ModelListings {
    State::create_global(models::load())
}

/// Mutate the listings and persist them — **the** write path; nothing else calls
/// [`models::save`].
///
/// Returns whether the edit reached disk. The in-memory slot is updated either way, on
/// `write_config`'s reasoning: the surface must show what was just fetched, and a listing that
/// fails to persist costs one refetch at the next launch.
///
/// **Not reported at a surface**, and that is the difference from `SettingsCtx::apply`: nothing
/// here is a deliberate commit the user pressed a button for. This is a cache being filled by a
/// fetch they did not ask for, so a failure is a `tracing` line and a refetch, never a message
/// about a file they have no reason to know exists.
///
/// The write itself is synchronous, like every other write of a file in the config directory
/// (`state::write_config`). It runs after an offloaded fetch has already come back, so the
/// blocking half of the refresh — the keystore read and the HTTP round trip — is off the render
/// thread; what is left is a few hundred bytes to the same directory the config store writes to
/// on every project open.
pub fn write_listings(state: ModelListings, edit: impl FnOnce(&mut Listings)) -> bool {
    let mut state = state;
    edit(&mut state.write());
    match models::save(&state.peek()) {
        Ok(()) => true,
        Err(e) => {
            tracing::error!("{e}");
            false
        }
    }
}

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
            Probe::Testing => Some((Tone::Working, "Testing…".into())),
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
/// not own success/warning/error colours.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Working,
    Good,
    Bad,
}

/// Every provider's probe, for as long as the app is running.
///
/// **One slot, app-global**, because the thing it guards is app-global: `Listings` is one value
/// on disk, and the surfaces that read it live in different windows — Settings ▸ AI ▸ Providers
/// runs the test, Settings ▸ AI ▸ Chat and every project window's chat composer report what it
/// said. Per window it was two mistakes at once: `SettingsCtx::forget_provider` could only
/// retract the Settings copy, so a credential edit left every composer's picker convinced it had
/// already asked and stranded on a stale offer; and two windows could hold a refresh for one
/// provider at the same time — one against the Settings draft, one against committed config —
/// racing two different endpoints' answers into the one satellite that persists.
///
/// **Still not persisted**, and that part is unchanged: a verification is a fact about a request
/// made minutes ago, and a "verified" restored at launch would be a claim nothing had checked.
/// The *names* it returned are what outlive the run, in the satellite above.
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

/// Everything a probe needs, copied out of whatever holds it before the thread starts.
///
/// Owned rather than borrowed because it crosses a thread boundary, and it carries the
/// [`SecretRef`] rather than the secret: the keystore read happens **on that thread**, so a
/// pasted key that has not been stored yet is passed as itself and one already stored is never
/// held by the caller at all. Exposure is managed by lifetime, which is `strata_core::secret`'s
/// own rule.
pub struct Ask {
    pub kind: ProviderKind,
    pub base_url: String,
    /// A key typed into a draft and not yet committed. Takes precedence over `stored`: it is
    /// what Apply would write, so it is what a test should prove.
    pub typed: Option<Secret>,
    /// The key already in the keystore, if nothing has replaced it.
    pub stored: Option<SecretRef>,
}

impl Ask {
    /// What **committed config** holds for `kind` — the ask every surface outside the Settings
    /// window makes, since there is no draft anywhere else.
    ///
    /// The Settings window has its own constructor over the uncommitted draft (`FromDraft`),
    /// because there a test has to prove what Apply *would* write rather than what is filed.
    pub fn from_config(ai: &Ai, kind: ProviderKind) -> Ask {
        let setup = ai.setup(kind);
        Ask {
            kind,
            base_url: setup.map(|s| s.base_url.clone()).unwrap_or_default(),
            typed: None,
            stored: setup.and_then(|s| s.key.clone()),
        }
    }
}

/// **Whether a surface showing `kind`'s models should ask again**, in the background.
///
/// Two conditions, and both are load-bearing. The listing is stale (or absent) — otherwise there
/// is nothing to fetch. And nothing has asked yet *in this window*: a refresh that failed leaves
/// the listing absent, so the staleness question alone would ask again on every repaint, and
/// [`Probe::Untested`] is true exactly once per provider per window. A deliberate Test in
/// Settings is the way to ask again.
///
/// One copy, because two surfaces make the same background kick: Settings ▸ AI ▸ Chat when its
/// page opens, and the chat composer's model picker when it opens.
pub fn needs_asking(listings: ModelListings, probes: ProviderProbes, kind: ProviderKind) -> bool {
    listings.peek().needs_refresh(kind) && matches!(probes.peek().get(kind), Probe::Untested)
}

/// **Ask the provider what it serves, and keep both halves of the answer.**
///
/// The one mechanism every fetching gesture calls. Names to the satellite, outcome to the
/// [`Probe`] — see the module doc for why those are two places and not two caches.
///
/// **Re-entrant by design and guarded once, here.** A request already in flight for this kind is
/// left alone rather than raced, so a Test press during a background refresh (or a second press)
/// cannot land two answers out of order. The guard is taken *before* the task exists, so two
/// gestures in one frame cannot both get past it.
///
/// **It spawns the task itself, and on the window rather than on the caller.** A plain `spawn`
/// binds the future to the scope that made it, so a dialog closed mid-test would drop the request
/// and leave the probe reading `Testing` for the rest of the window's life — a row saying
/// "testing…" about nothing, and a kind no further refresh can get past its own guard. The request
/// belongs to the window, which is `spawn_forever`'s scope; that is also why this is not an
/// `async fn` the caller awaits, so no new call site can forget the lifetime rule or the guard.
///
/// **Returns the request**, so a caller that later decides the answer is unwanted can stop it
/// arriving rather than only dropping it once it has landed. `None` means the guard swallowed the
/// call, and the handle for the in-flight one belongs to whoever started it.
pub fn refresh(listings: ModelListings, probes: ProviderProbes, ask: Ask) -> Option<TaskHandle> {
    let kind = ask.kind;
    let mut probes = probes;
    if matches!(probes.peek().get(kind), Probe::Testing) {
        return None;
    }
    probes.write().set(kind, Probe::Testing);

    Some(spawn_forever(async move {
        let settled = match run(ask).await {
            Ok(models) => {
                let count = models.len();
                write_listings(listings, |listings| listings.set(kind, models));
                Probe::Verified { count }
            }
            Err(why) => Probe::Failed { why },
        };

        let mut probes = probes;
        probes.write().set(kind, settled);
    }))
}

/// Ask the provider what it serves, off the render thread.
///
/// The provider's own words on a failure, already bounded by `list_models`.
async fn run(ask: Ask) -> Result<Vec<String>, String> {
    let answer = offload(move || {
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
