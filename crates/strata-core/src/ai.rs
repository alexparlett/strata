//! **The assistant's configuration vocabulary** (AS-03) — which brains the user has set up,
//! and what a new chat starts with.
//!
//! This module holds the persisted **tokens** and nothing that knows how to talk to a provider.
//! The knowledge — which `genai` adapter serves a kind, which models of it offer a reasoning
//! control, what a base URL has to look like — is one table in `strata_agent::assistant::
//! provider`, next to the `genai` pin it is verified against. The split is forced and it is
//! also right: `strata-agent` depends on *this* crate (for [`crate::secret`]), so a type
//! [`crate::config::Settings`] has to name cannot live up there, and a serde token has no
//! business knowing about an HTTP adapter either way.
//!
//! **A provider entry carries what addresses the provider and nothing about what it is asked.**
//! No model, no effort: those are a conversation's, picked in the chat pane and seeded from
//! [`Ai::default_model`] / [`Ai::default_effort`]. That is the def/runtime split applied to the
//! assistant — the same line `ConnectionDef` draws when a connection names a bucket and a
//! *table* names the connection.
//!
//! ## Two lists, because they are two different things
//!
//! - **A built-in provider's identity is its kind.** Anthropic is Anthropic; there is no second
//!   one, nothing to name and nothing to rename. So [`Ai::providers`] is keyed by
//!   [`ProviderKind`], and a kind absent from the map is one the user has never enabled —
//!   which is the same thing its toggle says, rather than a second copy of it.
//! - **A custom endpoint's identity is minted.** Any host speaking OpenAI's chat-completions
//!   API is reachable (llama.cpp, vLLM, LM Studio, a gateway) and there is no reason to have
//!   only one, so [`Ai::endpoints`] is a list the user maintains — keyed by a [`Uuid`], because
//!   its display name is the one thing whose whole purpose is to be retyped and
//!   [`Ai::default_brain`] points at it. The saved-query precedent, not the connection one.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::secret::SecretRef;

/// Which kind of brain — the providers the app offers a row for.
///
/// The serde spelling is a **stable token**, not the label: renaming what the pane calls a
/// provider must not orphan everybody's configuration. `Ord` because [`Ai::providers`] is keyed
/// by it; the ordering is the declaration order and carries no meaning beyond a stable map.
///
/// [`OpenAiCompatible`](ProviderKind::OpenAiCompatible) has no row of its own in Settings — it
/// is the kind every [`CustomEndpoint`] is, which is why it is the one variant a
/// [`BrainRef::Builtin`] never names.
///
/// **Cohere is deliberately absent**, though the pinned `genai` speaks to it and the design
/// canvas lists it. Its adapter never reads a request's `tools` and answers a `Tool`-role
/// message with `MessageRoleNotSupported` — so a Cohere entry could be enabled, could pass a
/// connection test, and could then never call a tool: the assistant would answer about data it
/// had not read, and fail outright on the next send. A provider that cannot run the loop is not
/// a provider this offers.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    Gemini,
    DeepSeek,
    Groq,
    Xai,
    Ollama,
    /// Any endpoint speaking OpenAI's chat-completions API. Configured as a named
    /// [`CustomEndpoint`] rather than as a single row: the base URL is the whole of what makes
    /// one addressable, so two of them are two endpoints and not two spellings of one.
    OpenAiCompatible,
}

/// One rung of the reasoning ladder, as a person picks it.
///
/// Deliberately the keyword rungs and not `genai`'s full `ReasoningEffort`, which also carries
/// `Budget(u32)`, `Minimal` and `None`. A token budget is a provider-shaped number with no
/// meaning across providers, `Minimal` is one vendor's legacy spelling of `Low`, and `None` is
/// what an unset effort already says — three ways to offer a control that has one job.
///
/// **Which rungs a given model actually offers is not a property of this type.** It is
/// `strata_agent::assistant::provider::efforts`, asked per model, because reasoning is a model
/// capability: `claude-opus-4-5` takes an effort and `claude-sonnet-4-5` does not.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl Effort {
    /// What the picker's segment says.
    pub fn label(self) -> &'static str {
        match self {
            Effort::Low => "Low",
            Effort::Medium => "Medium",
            Effort::High => "High",
            Effort::XHigh => "XHigh",
            Effort::Max => "Max",
        }
    }
}

impl std::fmt::Display for Effort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// What a conversation points at: one of the built-in providers, or one named endpoint.
///
/// The thing a chat's model and effort are picked *against*, and the seed a new chat takes from
/// [`Ai::default_brain`]. A sum type rather than a `Uuid` for everything, because minting an id
/// for Anthropic would be inventing an identity for something that already has one — and the
/// invented one could then disagree with the kind it claims.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrainRef {
    Builtin(ProviderKind),
    Custom(Uuid),
}

/// A built-in provider's setup: whether it is on, and the one credential it takes.
///
/// **The key is a reference, never the secret** ([`SecretRef`]) — that is a property of the
/// types rather than a rule to remember, and it is why this struct can derive `Serialize` at
/// all. `None` is the valid, ordinary state: the provider's own environment variable is the
/// fallback, and the pane's subtext names which one.
///
/// `base_url` is only meaningful for the kinds whose table row admits one (Ollama). It is a
/// `String` rather than an `Option<String>` because a cleared box and an absent field are the
/// same answer — "use the kind's default" — and two spellings of one answer is what the
/// blank-reads-as-absent rule exists to prevent.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ProviderSetup {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub key: Option<SecretRef>,
}

/// One named OpenAI-compatible endpoint.
///
/// The name is the user's and is display only — [`Ai::endpoints`]'s key is what anything points
/// at. `base_url` is required (an endpoint with no address is not one); the key is optional and
/// **has no environment fallback**, deliberately: the host is whatever the user typed, and
/// `genai`'s default for its OpenAI adapter would post their `OPENAI_API_KEY` to it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CustomEndpoint {
    pub name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub key: Option<SecretRef>,
}

/// The assistant's whole configuration: which brains are set up, and what a new chat starts
/// with.
///
/// One struct rather than five flat fields of [`Settings`](crate::config::Settings), for
/// `AgentAccess`'s reason: they are read and written as a unit, and the Settings draft's
/// per-field diff is against exactly this value.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Ai {
    /// The built-in providers the user has touched. A kind absent from this map has never been
    /// enabled and holds no credential.
    #[serde(default)]
    pub providers: BTreeMap<ProviderKind, ProviderSetup>,
    /// The named OpenAI-compatible endpoints, in insertion-independent id order.
    #[serde(default)]
    pub endpoints: BTreeMap<Uuid, CustomEndpoint>,
    /// Which brain a new chat starts on. `None` is a valid state the chat pane renders
    /// honestly — nothing is enabled yet, or the default's provider was turned off and no other
    /// was on to take its place.
    #[serde(default)]
    pub default_brain: Option<BrainRef>,
    /// The model a new chat starts on. Free text, because a model name is the provider's
    /// vocabulary: the pane offers the list the provider reports and still accepts a name no
    /// list mentions (a private deployment, a gateway that serves no `/models`).
    #[serde(default)]
    pub default_model: String,
    /// The reasoning rung a new chat starts on. `None` sends no preference and takes the
    /// model's own default — which is also the only valid value for a model with no rungs.
    #[serde(default)]
    pub default_effort: Option<Effort>,
}

impl Ai {
    /// This brain's setup, whichever list it lives in: whether it is enabled, its base URL, and
    /// its key reference.
    ///
    /// One lookup, so the surfaces that read a brain cannot disagree about it — the same
    /// reasoning `KeyStatus::of` is one function rather than one per surface. `None` means the
    /// reference resolves to nothing: a custom endpoint that was deleted while a chat pointed
    /// at it, which is a state the pane reports rather than one this silently repairs.
    pub fn setup(&self, brain: &BrainRef) -> Option<Setup<'_>> {
        match brain {
            BrainRef::Builtin(kind) => self.providers.get(kind).map(|p| Setup {
                kind: *kind,
                name: None,
                enabled: p.enabled,
                base_url: &p.base_url,
                key: p.key.as_ref(),
            }),
            BrainRef::Custom(id) => self.endpoints.get(id).map(|e| Setup {
                kind: ProviderKind::OpenAiCompatible,
                name: Some(&e.name),
                enabled: e.enabled,
                base_url: &e.base_url,
                key: e.key.as_ref(),
            }),
        }
    }

    /// Every brain the user could pick right now — the enabled built-ins in table order, then
    /// the enabled endpoints.
    ///
    /// What AI ▸ Chat's provider dropdown offers and what the chat pane's picker offers, so the
    /// UI can never name a provider it has no credential for.
    pub fn enabled(&self) -> impl Iterator<Item = BrainRef> + '_ {
        let builtins = self
            .providers
            .iter()
            .filter(|(_, setup)| setup.enabled)
            .map(|(kind, _)| BrainRef::Builtin(*kind));
        let custom = self
            .endpoints
            .iter()
            .filter(|(_, endpoint)| endpoint.enabled)
            .map(|(id, _)| BrainRef::Custom(*id));
        builtins.chain(custom)
    }

    /// Whether `brain` is enabled *and* still resolves — the question
    /// [`default_brain`](Ai::default_brain) has to answer before a new chat takes it.
    pub fn is_enabled(&self, brain: &BrainRef) -> bool {
        self.setup(brain).is_some_and(|setup| setup.enabled)
    }
}

/// A brain's setup, read through whichever list holds it.
///
/// Borrowed rather than owned: the caller has the [`Ai`] in hand and the key is a reference to a
/// keystore entry either way, so copying the strings would buy nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Setup<'a> {
    /// The provider kind — [`ProviderKind::OpenAiCompatible`] for every custom endpoint.
    pub kind: ProviderKind,
    /// The user's name for a custom endpoint. `None` for a built-in, whose name is its kind's
    /// label and is therefore the table's to give.
    pub name: Option<&'a str>,
    pub enabled: bool,
    pub base_url: &'a str,
    pub key: Option<&'a SecretRef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(kind: ProviderKind, enabled: bool) -> (ProviderKind, ProviderSetup) {
        (
            kind,
            ProviderSetup {
                enabled,
                ..ProviderSetup::default()
            },
        )
    }

    /// The two lists answer one question, and a reference into either resolves the same way.
    #[test]
    fn a_brain_resolves_through_whichever_list_holds_it() {
        let id = Uuid::new_v4();
        let ai = Ai {
            providers: [built(ProviderKind::Anthropic, true)].into_iter().collect(),
            endpoints: [(
                id,
                CustomEndpoint {
                    name: "Workstation".into(),
                    base_url: "http://localhost:8080/v1/".into(),
                    enabled: true,
                    key: None,
                },
            )]
            .into_iter()
            .collect(),
            ..Ai::default()
        };

        let anthropic = ai
            .setup(&BrainRef::Builtin(ProviderKind::Anthropic))
            .unwrap();
        assert_eq!(anthropic.kind, ProviderKind::Anthropic);
        assert_eq!(anthropic.name, None, "a built-in is named by its table row");

        let custom = ai.setup(&BrainRef::Custom(id)).unwrap();
        assert_eq!(custom.kind, ProviderKind::OpenAiCompatible);
        assert_eq!(custom.name, Some("Workstation"));
        assert_eq!(custom.base_url, "http://localhost:8080/v1/");
    }

    /// **A dangling reference is reported, not repaired.** A chat pointing at an endpoint the
    /// user deleted must not silently resolve to another one.
    #[test]
    fn a_reference_to_something_that_is_gone_resolves_to_nothing() {
        let ai = Ai {
            providers: [built(ProviderKind::Anthropic, true)].into_iter().collect(),
            ..Ai::default()
        };
        assert!(ai.setup(&BrainRef::Custom(Uuid::new_v4())).is_none());
        assert!(ai.setup(&BrainRef::Builtin(ProviderKind::Groq)).is_none());
        assert!(!ai.is_enabled(&BrainRef::Builtin(ProviderKind::Groq)));
    }

    /// Only what the user could actually send to: a disabled provider holds a key and is still
    /// not on offer.
    #[test]
    fn only_enabled_brains_are_offered() {
        let off = Uuid::new_v4();
        let ai = Ai {
            providers: [
                built(ProviderKind::Anthropic, true),
                built(ProviderKind::Groq, false),
                built(ProviderKind::Ollama, true),
            ]
            .into_iter()
            .collect(),
            endpoints: [(
                off,
                CustomEndpoint {
                    name: "Parked".into(),
                    base_url: "http://localhost:8080/v1/".into(),
                    enabled: false,
                    key: None,
                },
            )]
            .into_iter()
            .collect(),
            ..Ai::default()
        };
        let offered: Vec<BrainRef> = ai.enabled().collect();
        assert_eq!(
            offered,
            vec![
                BrainRef::Builtin(ProviderKind::Anthropic),
                BrainRef::Builtin(ProviderKind::Ollama),
            ]
        );
        assert!(!ai.is_enabled(&BrainRef::Custom(off)));
    }

    /// The serde spelling is the wire format for everybody's configuration, so it is asserted
    /// rather than left to the derive: a variant renamed for a label change would silently
    /// orphan every provider the user had set up.
    #[test]
    fn the_persisted_tokens_are_stable() {
        let json = serde_json::to_string(&BrainRef::Builtin(ProviderKind::OpenAi)).unwrap();
        assert_eq!(json, r#"{"builtin":"open_ai"}"#);
        assert_eq!(
            serde_json::to_string(&Effort::XHigh).unwrap(),
            r#""x_high""#
        );
        for (kind, token) in [
            (ProviderKind::Anthropic, "anthropic"),
            (ProviderKind::OpenAi, "open_ai"),
            (ProviderKind::Gemini, "gemini"),
            (ProviderKind::DeepSeek, "deep_seek"),
            (ProviderKind::Groq, "groq"),
            (ProviderKind::Xai, "xai"),
            (ProviderKind::Ollama, "ollama"),
            (ProviderKind::OpenAiCompatible, "open_ai_compatible"),
        ] {
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{token}\"")
            );
        }
    }

    /// An `Ai` that has never been touched round-trips as an empty object, so an older config
    /// file loads without the field and a fresh one does not grow noise.
    #[test]
    fn an_untouched_config_is_empty_both_ways() {
        let ai: Ai = serde_json::from_str("{}").unwrap();
        assert_eq!(ai, Ai::default());
        assert_eq!(ai.enabled().count(), 0);
    }

    /// **A key cannot reach the config file, and this asserts it on the bytes.**
    ///
    /// The claim the whole design rests on is that config carries a *reference* and never the
    /// secret — and that it is a property of the types rather than a rule to remember. The types
    /// are what make it true ([`SecretRef`] is all that is serializable; [`Secret`] has no serde
    /// path at all), so what is left to check is that nothing has grown a second way in: a
    /// `String` field for a "temporary" paste, a `#[serde(flatten)]` that picks one up.
    ///
    /// Written against the serialized text rather than the struct, because the file is what
    /// leaks. A test on the fields would pass on the day someone adds one.
    #[test]
    fn a_serialized_roster_carries_references_and_no_secret() {
        let secret = "sk-ant-do-not-write-me-down";
        let key = SecretRef::mint();
        let ai = Ai {
            providers: [(
                ProviderKind::Anthropic,
                ProviderSetup {
                    enabled: true,
                    base_url: String::new(),
                    key: Some(key.clone()),
                },
            )]
            .into_iter()
            .collect(),
            endpoints: [(
                Uuid::new_v4(),
                CustomEndpoint {
                    name: "Workstation".into(),
                    base_url: "http://localhost:8080/v1/".into(),
                    enabled: true,
                    key: Some(SecretRef::mint()),
                },
            )]
            .into_iter()
            .collect(),
            default_brain: Some(BrainRef::Builtin(ProviderKind::Anthropic)),
            default_model: "claude-sonnet-5".into(),
            default_effort: Some(Effort::High),
        };

        let written = serde_json::to_string(&ai).unwrap();
        assert!(
            !written.contains(secret),
            "a secret reached the config text: {written}"
        );
        assert!(
            !written.contains("sk-"),
            "something key-shaped reached the config text: {written}"
        );
        // The marker *does* travel — otherwise the key could never be found again. Asked by
        // serializing the reference itself rather than formatting it: `SecretRef` has no
        // `Display`, which is the same austerity `Secret` gets and is worth keeping.
        let marker = serde_json::to_string(&key).unwrap();
        let marker = marker.trim_matches('"');
        assert!(
            written.contains(marker),
            "the reference has to travel, or the key is unreachable: {written}"
        );

        // And it comes back as the same reference, which is what makes the round trip a
        // reference round trip rather than a re-mint.
        let read: Ai = serde_json::from_str(&written).unwrap();
        assert_eq!(read, ai);
        assert_eq!(
            read.providers[&ProviderKind::Anthropic].key.as_ref(),
            Some(&key)
        );
    }
}
