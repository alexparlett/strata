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
//! ## One list, keyed by kind
//!
//! **A provider's identity is its kind.** Anthropic is Anthropic; there is no second one,
//! nothing to name and nothing to rename. So [`Ai::providers`] is keyed by [`ProviderKind`], and
//! a kind absent from the map is one the user has never enabled — which is the same thing its
//! toggle says, rather than a second copy of it.
//!
//! That includes [`ProviderKind::OpenAiCompatible`], which was briefly a *list* of named,
//! id-keyed endpoints so that several could exist at once. Withdrawn: gateways exist to
//! multiplex (`LiteLLM` and its kind put many backends behind one OpenAI-compatible address), so
//! a second multiplexer here would sit in front of a solved problem while costing a sum-typed
//! identity that every surface downstream — the composer's picker, a chat's selection, the
//! transcript — would have had to carry. One row, addressed by its base URL, and the model list
//! the gateway reports is what distinguishes what is behind it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::secret::SecretRef;

/// Which kind of brain — the providers the app offers a row for.
///
/// The serde spelling is a **stable token**, not the label: renaming what the pane calls a
/// provider must not orphan everybody's configuration. `Ord` because [`Ai::providers`] is keyed
/// by it; the ordering is the declaration order and carries no meaning beyond a stable map.
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
    /// Any endpoint speaking OpenAI's chat-completions API — llama.cpp, vLLM, LM Studio, or a
    /// gateway. One row like any other: its base URL is what makes it addressable, and is
    /// therefore the one thing it cannot do without.
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

/// A provider's setup: whether it is on, and what addresses it.
///
/// **The key is a reference, never the secret** ([`SecretRef`]) — that is a property of the
/// types rather than a rule to remember, and it is why this struct can derive `Serialize` at
/// all. `None` is the valid, ordinary state: the provider's own environment variable is the
/// fallback, and the pane's subtext names which one.
///
/// `base_url` is only meaningful for the kinds whose table row admits one (Ollama, and the
/// compatible endpoint). It is a `String` rather than an `Option<String>` because a cleared box
/// and an absent field are the same answer — "use the kind's default, if it has one" — and two
/// spellings of one answer is what the blank-reads-as-absent rule exists to prevent.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ProviderSetup {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub key: Option<SecretRef>,
}

/// The assistant's whole configuration: which brains are set up, and what a new chat starts
/// with.
///
/// One struct rather than four flat fields of [`Settings`](crate::config::Settings), for
/// `AgentAccess`'s reason: they are read and written as a unit, and the Settings draft's
/// per-field diff is against exactly this value.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Ai {
    /// The providers the user has touched. A kind absent from this map has never been enabled
    /// and holds no credential.
    #[serde(default)]
    pub providers: BTreeMap<ProviderKind, ProviderSetup>,
    /// Which provider a new chat starts on. `None` is a valid state the chat pane renders
    /// honestly — nothing is enabled yet, or the default was turned off and no other was on to
    /// take its place.
    #[serde(default)]
    pub default_provider: Option<ProviderKind>,
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
    /// This provider's setup, or `None` if the user has never touched it.
    ///
    /// Absence is a real answer and the same one everywhere: never enabled, no endpoint typed,
    /// no key stored. Callers read it as such rather than pre-seeding the map.
    pub fn setup(&self, kind: ProviderKind) -> Option<&ProviderSetup> {
        self.providers.get(&kind)
    }

    /// Every provider the user could pick right now, in the table's own order.
    ///
    /// What AI ▸ Chat's provider dropdown offers and what the chat pane's picker offers, so the
    /// UI can never name a provider it has no credential for.
    pub fn enabled(&self) -> impl Iterator<Item = ProviderKind> + '_ {
        self.providers
            .iter()
            .filter(|(_, setup)| setup.enabled)
            .map(|(kind, _)| *kind)
    }

    /// Whether `kind` is enabled — the question
    /// [`default_provider`](Ai::default_provider) has to answer before a new chat takes it.
    pub fn is_enabled(&self, kind: ProviderKind) -> bool {
        self.setup(kind).is_some_and(|setup| setup.enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(kind: ProviderKind) -> (ProviderKind, ProviderSetup) {
        (
            kind,
            ProviderSetup {
                enabled: true,
                ..ProviderSetup::default()
            },
        )
    }

    /// A provider nobody has touched reads as absent rather than as a default-shaped row, which
    /// is what lets "never enabled" and "has an entry that is off" stay one answer.
    #[test]
    fn an_untouched_provider_has_no_setup() {
        let ai = Ai {
            providers: [on(ProviderKind::Anthropic)].into_iter().collect(),
            ..Ai::default()
        };
        assert!(ai.setup(ProviderKind::Anthropic).is_some());
        assert!(ai.setup(ProviderKind::Groq).is_none());
        assert!(!ai.is_enabled(ProviderKind::Groq));
    }

    /// Only what the user could actually send to: a disabled provider holds a key and is still
    /// not on offer.
    #[test]
    fn only_enabled_providers_are_offered() {
        let ai = Ai {
            providers: [
                on(ProviderKind::Anthropic),
                (
                    ProviderKind::Groq,
                    ProviderSetup {
                        enabled: false,
                        base_url: String::new(),
                        key: Some(SecretRef::mint()),
                    },
                ),
                on(ProviderKind::Ollama),
            ]
            .into_iter()
            .collect(),
            ..Ai::default()
        };
        assert_eq!(
            ai.enabled().collect::<Vec<_>>(),
            vec![ProviderKind::Anthropic, ProviderKind::Ollama],
            "a stored key is not the same as being on"
        );
    }

    /// The serde spelling is the wire format for everybody's configuration, so it is asserted
    /// rather than left to the derive: a variant renamed for a label change would silently
    /// orphan every provider the user had set up.
    #[test]
    fn the_persisted_tokens_are_stable() {
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
            default_provider: Some(ProviderKind::Anthropic),
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
