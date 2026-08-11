//! **Test, and the model list — which are one call.**
//!
//! There is no ping in `genai` and there does not need to be: listing a provider's models is a
//! live request against its endpoint with its configured credential, which is exactly what a
//! connection test proves, and its answer is exactly what AI ▸ Chat's model dropdown needs. A
//! separate reachability probe would be a second round trip proving strictly less, and two
//! results that could disagree about one provider.
//!
//! So a [`Probe`] is what the Test button writes *and* what the model picker reads.
//!
//! **Editing a credential retracts the result.** A "verified" beside a key that has since been
//! retyped is a claim about a request that was never made — the same "only real facts" rule the
//! row's subline follows. [`Probes::forget`] is that retraction, and the pane calls it from the
//! same handler that writes the draft.
//!
//! ## Where the work runs
//!
//! On a thread of its own ([`crate::task::offload`]), because a keystore read and an HTTP round
//! trip are both things the render thread must never wait on. What drives the request is
//! `provider::list_models_blocking` — over in the crate that owns `genai`, so a runtime and an
//! HTTP client are never things this frontend has to carry. One press, one thread, one request,
//! torn down after.

use std::collections::BTreeMap;

use strata_agent::assistant::list_models_blocking;
use strata_core::ai::ProviderKind;
use strata_core::secret::{Secret, SecretRef};

use crate::task::offload;

/// What is known about one brain's endpoint, from having actually asked it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum Probe {
    /// Never tested, or retracted by an edit. The row shows what it knows without asking.
    #[default]
    Untested,
    /// A request is in flight.
    Testing,
    /// The provider answered. The models are the picker's list — empty is a real answer, and a
    /// different one from a failure: the endpoint is reachable and serves nothing.
    Verified { models: Vec<String> },
    /// The provider's own words, already bounded by `list_models`.
    Failed { why: String },
}

impl Probe {
    /// The models this probe knows about — none unless a request actually came back.
    pub fn models(&self) -> &[String] {
        match self {
            Probe::Verified { models } => models,
            _ => &[],
        }
    }

    /// The status line under the credential row, and the tone to paint it.
    ///
    /// `None` while untested: a row that has never been asked has nothing to report, and
    /// "unknown" printed under every provider is noise rather than information.
    pub fn status(&self) -> Option<(Tone, String)> {
        match self {
            Probe::Untested => None,
            Probe::Testing => Some((Tone::Working, "testing…".into())),
            Probe::Verified { models } => Some((
                Tone::Good,
                match models.len() {
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
/// is: AI ▸ Providers runs the test and AI ▸ Chat reads the models it returned, and a result
/// thrown away by navigating between the two would make the model picker empty exactly when the
/// user has just proved it need not be.
///
/// Not persisted, and deliberately: a verification is a fact about a request made minutes ago,
/// and a "verified" restored from disk at launch would be a claim nothing had checked.
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

/// Ask the provider what it serves, off the render thread.
///
/// Returns the [`Probe`] to store — never a partial state: the caller sets [`Probe::Testing`]
/// before awaiting this and replaces it with whatever comes back.
pub async fn run(ask: Ask) -> Probe {
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
        Some(Ok(models)) => Probe::Verified { models },
        Some(Err(why)) => Probe::Failed { why },
        // The thread never answered — it could not start, or it panicked. Neither is a fact
        // about the provider, so this must not claim to have reached one.
        None => Probe::Failed {
            why: "The test could not be run.".into(),
        },
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
        let (tone, said) = Probe::Verified { models: Vec::new() }.status().unwrap();
        assert_eq!(tone, Tone::Good);
        assert!(said.contains("no models"), "{said}");
    }

    /// The count is what the row shows, so it is not pluralized by hand at the call site.
    #[test]
    fn a_verified_probe_counts_what_came_back() {
        let one = Probe::Verified {
            models: vec!["gpt-5".into()],
        };
        assert_eq!(one.status().unwrap().1, "connection verified, 1 model");
        let two = Probe::Verified {
            models: vec!["gpt-5".into(), "gpt-4o".into()],
        };
        assert_eq!(two.status().unwrap().1, "connection verified, 2 models");
        assert_eq!(two.models().len(), 2);
    }

    /// **An edit retracts the answer.** A key retyped after a successful test leaves a claim
    /// about a request nobody would make now.
    #[test]
    fn editing_a_credential_forgets_what_was_verified() {
        let kind = ProviderKind::Anthropic;
        let mut probes = Probes::default();
        probes.set(
            kind,
            Probe::Verified {
                models: vec!["claude-sonnet-5".into()],
            },
        );
        assert!(!probes.get(kind).models().is_empty());

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
