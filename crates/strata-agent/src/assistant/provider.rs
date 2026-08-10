//! **The provider seam** — which brain answers a send, and the one table every surface reads
//! that from.
//!
//! Three surfaces have to agree about providers and none of them may restate the others:
//! Settings maintains the roster (AS-03 — which brains exist, their endpoints, their keys),
//! a chat conversation holds the pick (AS-04 — which entry, which model, what effort), and
//! this module turns a resolved pick into a `genai` client. So the kinds, their labels, their
//! key policy, their base-URL policy and **which effort rungs they offer** are one table
//! ([`PROVIDERS`]), and a form that offers a field the table does not declare is a form that
//! does not compile against it.
//!
//! ## The pick is per send, so a window is not a mode
//!
//! [`Selection`] is plain data handed in with every send. Nothing here reads Settings, holds
//! a client between turns, or remembers what the last conversation chose — which is the whole
//! of "several chat panes, each on its own provider, model and effort": two conversations
//! disagreeing is two [`Selection`] values, not a mode somewhere. The def/runtime split, one
//! layer down from where the app applies it.
//!
//! ## Effort is offered per **model**, and mapped per model by `genai`
//!
//! Reasoning effort is not a portable knob: OpenAI spells it `reasoning_effort`, Anthropic as
//! an `output_config.effort` or a thinking budget depending on the model, Gemini as a
//! `thinkingLevel` or a `thinkingBudget`, and Ollama not at all. Nor is it a property of the
//! *provider*: `claude-opus-4-5` takes an effort and `claude-sonnet-4-5` does not, `gpt-5`
//! does and `gpt-4o` does not. So the split is:
//!
//! - **Whether the control is offered is a property of the model**, decided by the kind's
//!   [`Efforts`] rule in the table, asked through [`ProviderKind::efforts`]. A model with no
//!   rungs gets no menu, and a [`Selection`] that sets one anyway is refused rather than
//!   silently ignored. What belongs in a rule is "will the pinned `genai` actually send an
//!   effort for this model" — for Anthropic that is the modern Claude families, for OpenAI the
//!   reasoning models (`reasoning_effort` is not a field `gpt-4o` accepts), for Gemini the
//!   thinking ones. Each row says which and why.
//! - **What a rung means for a model that has one is `genai`'s**, verified at the pinned
//!   version: its Anthropic adapter already gates `xhigh` and `max` per model, and its Gemini
//!   adapter already knows `gemini-3` takes a thinking *level* where 2.5 takes a *budget*.
//!   Restating that here would be a second copy of a mapping that already exists.
//!
//! The rules are name fragments mirroring what the pinned `genai` recognizes, so they fall
//! behind what the providers ship — which is why [`Efforts::Only`] is **default-closed**:
//! falling behind costs a knob the user cannot reach yet, never a menu whose settings the
//! provider refuses. A `genai` bump is the moment to revisit them.
//!
//! ## One construction site
//!
//! [`Brain::resolve`] is the only place a `genai::Client` is built, and it either builds one
//! or names the field that is missing and the pane it is set in ([`SelectionError`]) — with
//! no network attempt either way, which is what lets the chat pane degrade honestly (AS-04)
//! instead of reporting a timeout for an empty box.

use std::env;
use std::fmt;

use genai::adapter::AdapterKind;
use genai::chat::{CacheControl, ChatOptions, ReasoningEffort};
use genai::resolver::{
    AuthData, AuthResolver, Endpoint, Error as ResolverError, ServiceTargetResolver,
};
use genai::{Client, ModelIden, ServiceTarget};
use serde::{Deserialize, Serialize};
use strata_core::secret::Secret;
use url::Url;

/// Which kind of brain — the five the roster offers.
///
/// The serde spelling is what AS-03 persists, so it is a stable token rather than the label:
/// renaming what the pane calls a provider must not orphan everybody's roster.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    Gemini,
    Ollama,
    /// Any endpoint speaking OpenAI's chat-completions API: llama.cpp, vLLM, LM Studio, a
    /// gateway. The base URL is the whole of what makes it addressable, so it is required.
    OpenAiCompatible,
}

/// One rung of the reasoning ladder, as a person picks it.
///
/// Deliberately the keyword rungs and not `genai`'s full [`ReasoningEffort`], which also
/// carries `Budget(u32)`, `Minimal` and `None`. A token budget is a provider-shaped number
/// with no meaning across providers, `Minimal` is one vendor's legacy spelling of `Low`, and
/// `None` is what an unset [`Selection::effort`] already says — three ways to offer a control
/// that has one job.
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
    /// What the picker's row says.
    pub fn label(self) -> &'static str {
        match self {
            Effort::Low => "Low",
            Effort::Medium => "Medium",
            Effort::High => "High",
            Effort::XHigh => "XHigh",
            Effort::Max => "Max",
        }
    }

    fn genai(self) -> ReasoningEffort {
        match self {
            Effort::Low => ReasoningEffort::Low,
            Effort::Medium => ReasoningEffort::Medium,
            Effort::High => ReasoningEffort::High,
            Effort::XHigh => ReasoningEffort::XHigh,
            Effort::Max => ReasoningEffort::Max,
        }
    }
}

impl fmt::Display for Effort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// The ladder a model offers when it offers one. Our vocabulary; the per-model collapse of a
/// rung (`xhigh` down to `high`, a keyword to a token budget) stays `genai`'s — see the module
/// note.
const LADDER: &[Effort] = &[
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::XHigh,
    Effort::Max,
];

/// **Which models of a kind offer a reasoning control** — the rule, per kind, in the table.
///
/// Reasoning is a *model* capability, not a provider one: `claude-opus-4-5` takes an effort and
/// `claude-sonnet-4-5` does not, `gpt-5` does and `gpt-4o` does not. A per-kind answer is
/// therefore wrong in both directions — it hides a control that works, or offers one that
/// breaks the turn.
///
/// **[`Only`](Efforts::Only) is default-closed, and that is the whole safety argument.** These
/// name fragments are a list somebody has to maintain against what the providers ship, so it
/// *will* fall behind. Defaulting to "no control" makes falling behind cost a knob the user
/// cannot reach yet — an omission they can report — instead of a menu that produces a request
/// the provider refuses. Matching is `contains`, which is how `genai` matches model names
/// internally, so the two agree about what a name means.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Efforts {
    /// No model of this kind has one. Ollama's API carries no such field at all.
    Never,
    /// Every model: the endpoint is the user's own and they know what it accepts. Nothing is
    /// sent unless a rung is picked, and an endpoint that rejects the field says so itself.
    Always,
    /// Only models whose name contains one of these.
    Only(&'static [&'static str]),
}

impl Efforts {
    /// The rungs `model` offers — empty when it offers none.
    pub fn rungs(self, model: &str) -> &'static [Effort] {
        match self {
            Efforts::Never => &[],
            Efforts::Always => LADDER,
            Efforts::Only(names) => match names.iter().any(|name| model.contains(name)) {
                true => LADDER,
                false => &[],
            },
        }
    }
}

/// What a kind does with a base URL.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BaseUrl {
    /// The provider's own, and not the user's to change. The field is absent from the form.
    Provider,
    /// Editable, with this as the default when the user has typed none.
    Editable(&'static str),
    /// There is no default. Without one the provider has no address at all.
    Required,
}

/// What a kind does with an API key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyUse {
    /// A key is required, and this environment variable is the fallback when the roster
    /// entry holds none — `genai`'s own default for the adapter, named here so Settings can
    /// say which variable in its help text rather than hand-typing one.
    Env(&'static str),
    /// A key is sent when there is one and the call is made without one when there is not.
    /// **No environment fallback**, deliberately: this is the arbitrary-endpoint case, and
    /// `genai`'s default for its OpenAI adapter would post the user's `OPENAI_API_KEY` to
    /// whatever host they typed.
    Anonymous,
    /// The provider takes no key. The field is absent from the form.
    Unused,
}

/// One provider kind, in full — everything Settings, the composer footer and this module
/// each need to know about it.
#[derive(Clone, Copy, Debug)]
pub struct Provider {
    pub kind: ProviderKind,
    /// What every surface calls it.
    pub label: &'static str,
    pub base_url: BaseUrl,
    pub key: KeyUse,
    /// Which of this kind's models offer a reasoning control, and therefore whether the
    /// composer footer draws one for the model in hand. Ask it through
    /// [`ProviderKind::efforts`]; a [`Selection`] carrying a rung the model does not offer is
    /// refused rather than sent.
    pub efforts: Efforts,
    /// A current model name, for the model field's placeholder. A hint, never a list: model
    /// names churn faster than a release cycle, so the field is free-form text and an unknown
    /// name is answered by the provider itself.
    pub model_example: &'static str,
    /// The `genai` adapter this kind routes through. Private: no surface outside this module
    /// has any business naming one, and [`Provider::adapter`] is what resolves the one kind
    /// that is two adapters.
    adapter: AdapterKind,
}

/// **The table.** One row per kind, and the only place any of these facts is written.
pub const PROVIDERS: [Provider; 5] = [
    Provider {
        kind: ProviderKind::Anthropic,
        label: "Anthropic",
        base_url: BaseUrl::Provider,
        key: KeyUse::Env("ANTHROPIC_API_KEY"),
        // The models genai 0.7 will send an `output_config.effort` for. Narrower than this
        // once, on the theory that enabling thinking broke the next tool round because genai
        // never returns the thinking block — withdrawn: genai's own notes say thinking is *on
        // by default* for Sonnet 5 and always on for Fable, so if that were fatal Anthropic
        // tool use would be broken there with or without an effort set.
        efforts: Efforts::Only(&[
            "claude-opus-4-5",
            "claude-opus-4-6",
            "claude-opus-4-7",
            "claude-opus-4-8",
            "claude-opus-5",
            "claude-sonnet-4-6",
            "claude-sonnet-5",
            "fable",
            "mythos",
        ]),
        model_example: "claude-sonnet-4-5",
        adapter: AdapterKind::Anthropic,
    },
    Provider {
        kind: ProviderKind::OpenAi,
        label: "OpenAI",
        base_url: BaseUrl::Provider,
        key: KeyUse::Env("OPENAI_API_KEY"),
        // OpenAI's reasoning models. `reasoning_effort` is not a field the others accept, so
        // offering it for `gpt-4o` would be a menu whose every setting is an error. The
        // Responses models (`gpt-5`, `codex`) round-trip their reasoning item because
        // `Brain::resolve` sets `capture_reasoning_content`.
        efforts: Efforts::Only(&["gpt-5", "gpt-6", "codex", "o1", "o3", "o4"]),
        model_example: "gpt-5",
        // Nominal. `gpt-5` and the codex models speak the Responses API and the rest speak
        // chat completions, which is a per-model fork `adapter()` asks genai to make.
        adapter: AdapterKind::OpenAI,
    },
    Provider {
        kind: ProviderKind::Gemini,
        label: "Gemini",
        base_url: BaseUrl::Provider,
        key: KeyUse::Env("GEMINI_API_KEY"),
        // The thinking models: genai sends `thinkingLevel` for `gemini-3`/`gemma-4` and a
        // `thinkingBudget` for the rest, and a model with no thinking config refuses both.
        // Gemini's streamer captures thought signatures unconditionally and genai puts them
        // back, so a tool round is safe here.
        efforts: Efforts::Only(&["gemini-2.5", "gemini-3", "gemma-4"]),
        model_example: "gemini-3-pro-preview",
        adapter: AdapterKind::Gemini,
    },
    Provider {
        kind: ProviderKind::Ollama,
        label: "Ollama",
        base_url: BaseUrl::Editable("http://localhost:11434/"),
        key: KeyUse::Unused,
        // Ollama's API carries no reasoning-effort field and genai's adapter sends none, so
        // the control would be a menu that changes nothing whatever model is named.
        efforts: Efforts::Never,
        model_example: "qwen3:14b",
        adapter: AdapterKind::Ollama,
    },
    Provider {
        kind: ProviderKind::OpenAiCompatible,
        label: "OpenAI-compatible",
        base_url: BaseUrl::Required,
        key: KeyUse::Anonymous,
        // The one kind whose models we cannot know: the endpoint is the user's own, so they
        // are the authority on whether it reasons. Nothing is sent unless a rung is picked,
        // and an endpoint that rejects the field says so in its own words.
        efforts: Efforts::Always,
        model_example: "llama-3.3-70b",
        adapter: AdapterKind::OpenAI,
    },
];

impl ProviderKind {
    /// Every kind, in the order the roster's picker lists them — **read off the table**, so
    /// there is no second list of the kinds to fall out of step with it. A fixed-size array
    /// literal here would keep compiling after a sixth variant was added, and the new
    /// provider would be silently missing from every surface built from this.
    pub fn all() -> impl Iterator<Item = ProviderKind> {
        PROVIDERS.iter().map(|provider| provider.kind)
    }

    /// This kind's row of [`PROVIDERS`]. A match rather than an index, so a kind added
    /// without a row is a build error rather than a panic on the day somebody picks it.
    pub fn info(self) -> &'static Provider {
        match self {
            ProviderKind::Anthropic => &PROVIDERS[0],
            ProviderKind::OpenAi => &PROVIDERS[1],
            ProviderKind::Gemini => &PROVIDERS[2],
            ProviderKind::Ollama => &PROVIDERS[3],
            ProviderKind::OpenAiCompatible => &PROVIDERS[4],
        }
    }

    /// What every surface calls this kind.
    pub fn label(self) -> &'static str {
        self.info().label
    }

    /// The effort rungs `model` offers — empty when it offers none, which is what the
    /// composer footer draws no control for. Per **model**, because reasoning is a model
    /// capability: `claude-opus-4-5` takes an effort and `claude-sonnet-4-5` does not.
    pub fn efforts(self, model: &str) -> &'static [Effort] {
        self.info().efforts.rungs(model)
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl Provider {
    /// Which `genai` adapter serves `model` for this kind.
    ///
    /// One kind is two adapters: OpenAI's newer models speak the Responses API and the older
    /// ones chat completions, and **which is which is genai's knowledge, not ours** — so it is
    /// asked. Its answer is taken only if it stayed in the family, because
    /// `AdapterKind::from_model` falls back to Ollama for any name it does not recognize, and
    /// a key-bearing provider silently rerouted to localhost is the worst kind of wrong.
    fn adapter(&self, model: &str) -> AdapterKind {
        match self.kind {
            ProviderKind::OpenAi => match AdapterKind::from_model(model) {
                Ok(kind @ (AdapterKind::OpenAI | AdapterKind::OpenAIResp)) => kind,
                _ => AdapterKind::OpenAI,
            },
            _ => self.adapter,
        }
    }

    /// The one copy of the base-URL rule, called by [`Brain::resolve`] **and** by the Settings
    /// form (AS-03), on `Provider::check_address`'s precedent: two places that judge an
    /// address differently is a form that accepts what the client then refuses.
    ///
    /// Normalizing the trailing slash is the load-bearing half. Every adapter joins its path
    /// onto this — Ollama by `format!("{base}api/chat")`, the OpenAI family through
    /// `Url::join` — so `http://host/v1` reaches `http://host/chat/completions` (join replaces
    /// the last segment) and `http://localhost:11434` reaches `http://localhost:11434api/chat`.
    /// Both fail as a connection error naming a URL the user never typed.
    ///
    /// **The slash goes on the parsed URL's path, never on the raw text.** A base URL may carry
    /// a query — genai's OpenAI adapter supports exactly that, lifting the query off before it
    /// joins and putting it back after — and appending to the text would put the slash inside
    /// the query instead, so `https://gw.example/v1?api-version=2024-02-01` would reach
    /// `https://gw.example/chat/completions?api-version=2024-02-01%2F`: the `/v1` eaten and the
    /// version corrupted. That is the same failure this function exists to prevent, arrived at
    /// from the other side.
    pub fn check_base_url(url: &str) -> Result<String, String> {
        let url = url.trim();
        if url.is_empty() {
            return Err("A base URL is required.".into());
        }
        let mut parsed = Url::parse(url).map_err(|e| format!("'{url}' is not a URL: {e}."))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(format!("'{url}' must be an http or https URL."));
        }
        if !parsed.path().ends_with('/') {
            let path = format!("{}/", parsed.path());
            parsed.set_path(&path);
        }
        Ok(parsed.to_string())
    }
}

/// One conversation's pick, resolved: everything needed to talk to a model, and nothing else.
///
/// Built by the app per send from the roster entry (AS-03) the conversation points at and the
/// overrides the composer footer holds (AS-04). The key arrives **already read** from the OS
/// keystore, which is what keeps this crate keystore-free exactly as it is Freya-free — and it
/// arrives as a [`Secret`], so a `tracing::debug!("{selection:?}")` cannot print it.
#[derive(Clone, PartialEq, Debug)]
pub struct Selection {
    pub kind: ProviderKind,
    /// The model name, as the provider spells it.
    pub model: String,
    /// The endpoint, for the kinds whose [`BaseUrl`] admits one. `None` takes the kind's
    /// default where it has one.
    pub base_url: Option<String>,
    /// The key, for the kinds whose [`KeyUse`] admits one. `None` falls back to the
    /// provider's environment variable where the kind declares one.
    pub api_key: Option<Secret>,
    /// The reasoning rung, for the kinds that offer any. `None` sends no preference at all
    /// and takes the model's own default.
    pub effort: Option<Effort>,
}

impl Selection {
    /// The least a selection can be: a kind and a model, with every optional field unset.
    pub fn new(kind: ProviderKind, model: impl Into<String>) -> Selection {
        Selection {
            kind,
            model: model.into(),
            base_url: None,
            api_key: None,
            effort: None,
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Selection {
        self.base_url = Some(url.into());
        self
    }

    pub fn with_key(mut self, key: Secret) -> Selection {
        self.api_key = Some(key);
        self
    }

    pub fn with_effort(mut self, effort: Effort) -> Selection {
        self.effort = Some(effort);
        self
    }
}

/// Why a [`Selection`] cannot make a client: the field that is missing, and where it is set.
///
/// Every one of these is answered before a socket is opened. A half-configured provider that
/// reported a connection timeout would send the user looking at their network for a box they
/// never filled in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SelectionError {
    /// No model name.
    NoModel { kind: ProviderKind },
    /// The kind needs a base URL and the selection carries none.
    NoBaseUrl { kind: ProviderKind },
    /// The kind has no base URL to set, and one was set anyway. Refused rather than dropped:
    /// a field that is silently ignored is a lie on screen.
    BaseUrlNotUsed { kind: ProviderKind },
    /// The base URL is not a URL this can call.
    BadBaseUrl { url: String, why: String },
    /// A keyed provider with no key in the roster and nothing in its environment variable.
    NoKey {
        kind: ProviderKind,
        env: &'static str,
    },
    /// A key was set for a provider that takes none.
    KeyNotUsed { kind: ProviderKind },
    /// An effort rung this **model** does not offer — including any rung at all for a model
    /// with no reasoning control.
    NoSuchEffort {
        kind: ProviderKind,
        model: String,
        effort: Effort,
    },
}

impl fmt::Display for SelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SelectionError::NoModel { kind } => {
                write!(f, "Choose a model for {kind} in Settings > Assistant.")
            }
            SelectionError::NoBaseUrl { kind } => write!(
                f,
                "{kind} needs a base URL. Set one in Settings > Assistant."
            ),
            SelectionError::BaseUrlNotUsed { kind } => write!(
                f,
                "{kind} has its own endpoint and takes no base URL. Clear it in Settings > \
                 Assistant."
            ),
            SelectionError::BadBaseUrl { url, why } => write!(f, "{why} Base URL: '{url}'."),
            SelectionError::NoKey { kind, env } => write!(
                f,
                "{kind} needs an API key. Set one in Settings > Assistant, or set '{env}'."
            ),
            SelectionError::KeyNotUsed { kind } => write!(
                f,
                "{kind} takes no API key. Clear it in Settings > Assistant."
            ),
            SelectionError::NoSuchEffort {
                kind,
                model,
                effort,
            } => match kind.efforts(model) {
                [] => write!(f, "{kind} '{model}' has no reasoning effort setting."),
                _ => write!(
                    f,
                    "{kind} '{model}' does not offer '{effort}' reasoning effort."
                ),
            },
        }
    }
}

impl std::error::Error for SelectionError {}

/// A [`Selection`] that can be talked to: the client, the model it addresses, and the options
/// the effort rung became.
///
/// One per turn, **over a connection pool that is not**. The two are separable and were once
/// conflated here: the resolver closures carry this selection's key and endpoint and must live
/// exactly one turn, but the `reqwest` pool underneath them has no business being torn down
/// with them — genai's own default client tunes `pool_max_idle_per_host(4)` and a 20-second
/// HTTP/2 keep-alive, which pays for nothing if every user message dials a fresh TCP and TLS
/// handshake. So the pool is the app's ([`Assistant`](super::Assistant)) and is cloned in.
///
/// The key's exposure is still a turn: a configured key becomes a plain `String` inside
/// `AuthData` here, which is the copy `genai` puts in the `Authorization` header — the
/// `strata_core::secret` module's own point that exposure is managed by lifetime rather than by
/// guarding one link of six.
pub struct Brain {
    client: Client,
    model: ModelIden,
    options: ChatOptions,
}

impl Brain {
    /// **The one construction site.** Everything the table says about the kind is applied
    /// here, and a selection that cannot make a client says which field is missing.
    ///
    /// `pool` is the app-lifetime HTTP client every turn shares; `reqwest::Client` is an
    /// internal `Arc`, so the clone is a refcount bump.
    pub fn resolve(selection: &Selection, pool: &reqwest::Client) -> Result<Brain, SelectionError> {
        let kind = selection.kind;
        let provider = kind.info();

        let model = selection.model.trim();
        if model.is_empty() {
            return Err(SelectionError::NoModel { kind });
        }

        if let Some(effort) = selection.effort {
            if !provider.efforts.rungs(model).contains(&effort) {
                return Err(SelectionError::NoSuchEffort {
                    kind,
                    model: model.to_string(),
                    effort,
                });
            }
        }

        // -- The endpoint. `None` here means "genai's default for the adapter", which is the
        //    right answer for exactly the kinds that own their address.
        //
        //    A blank box is **absent**, not present-and-empty: a text input yields `Some("")`
        //    for a field the user has already cleared, and matching on presence alone would
        //    answer that with "takes no base URL. Clear it in Settings" — an instruction they
        //    have already followed and cannot follow again. Same reading `model` gets.
        let typed = selection
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty());
        let endpoint: Option<String> = match (provider.base_url, typed) {
            (BaseUrl::Provider, None) => None,
            (BaseUrl::Provider, Some(_)) => return Err(SelectionError::BaseUrlNotUsed { kind }),
            (BaseUrl::Editable(default), None) => Some(default.to_string()),
            (BaseUrl::Required, None) => return Err(SelectionError::NoBaseUrl { kind }),
            (BaseUrl::Editable(_) | BaseUrl::Required, Some(url)) => Some(
                Provider::check_base_url(url).map_err(|why| SelectionError::BadBaseUrl {
                    url: url.to_string(),
                    why,
                })?,
            ),
        };

        // -- The key. `None` here also means "genai's default", which is only ever taken by a
        //    kind whose default is not a key at all (Ollama's constant).
        let auth: Option<AuthData> = match (provider.key, selection.api_key.as_ref()) {
            (KeyUse::Unused, Some(_)) => return Err(SelectionError::KeyNotUsed { kind }),
            (KeyUse::Unused, None) => None,
            (KeyUse::Env(_) | KeyUse::Anonymous, Some(key)) => {
                Some(AuthData::Key(key.expose().to_string()))
            }
            // Only the variable's *name* is handed to genai, which reads it per request — so
            // the key is never cached in a value of ours. Its presence still has to be checked
            // here, because "the key is missing" must be answerable before a socket opens
            // rather than as a 401 three seconds later; `var_os` is what does that without
            // copying the key onto our heap to look at it.
            (KeyUse::Env(var), None) => match env::var_os(var) {
                Some(value) if !value.is_empty() => Some(AuthData::from_env(var)),
                _ => return Err(SelectionError::NoKey { kind, env: var }),
            },
            // No key, and no variable to fall back to. An empty bearer is what a local
            // endpoint expects and what a real one answers 401 to, in its own words.
            (KeyUse::Anonymous, None) => Some(AuthData::Key(String::new())),
        };

        let mut builder = Client::builder().with_reqwest(pool.clone());
        if let Some(auth) = auth {
            builder = builder.with_auth_resolver(AuthResolver::from_resolver_fn(
                move |_: ModelIden| -> Result<Option<AuthData>, ResolverError> {
                    Ok(Some(auth.clone()))
                },
            ));
        }
        if let Some(endpoint) = endpoint {
            builder =
                builder.with_service_target_resolver(ServiceTargetResolver::from_resolver_fn(
                    move |target: ServiceTarget| -> Result<ServiceTarget, ResolverError> {
                        Ok(ServiceTarget {
                            endpoint: Endpoint::from_owned(endpoint.clone()),
                            ..target
                        })
                    },
                ));
        }

        let mut options = ChatOptions::default()
            // The turn appends the assistant's own message to the conversation from these,
            // rather than from the deltas it forwarded — genai's concatenation is the one
            // that also carries tool calls and the thought signatures Gemini 3 requires back.
            .with_capture_content(true)
            .with_capture_tool_calls(true)
            // **Not a display option — it is what makes reasoning survive a tool round.** On
            // genai's OpenAI Responses adapter this flag is what inserts
            // `include: ["reasoning.encrypted_content"]` on the request *and* what makes its
            // streamer record the thought signatures; without it a gpt-5 tool loop re-sends
            // `function_call` items with no reasoning item in front of them, which OpenAI
            // either refuses or answers having discarded the model's chain of thought every
            // round. Gemini's streamer captures signatures unconditionally, which is why the
            // gap was invisible from that side.
            .with_capture_reasoning_content(true)
            // **The cache the rest of the design is arranged around.** Two decisions exist to
            // keep a request's prefix byte-identical across a conversation — the manifest is
            // sorted (`StrataTools::manifest`) and pinned context rides the user's message
            // rather than the system prompt (`Ask::message`) — and both bought nothing until
            // a breakpoint was actually asked for. Request-level is the right level: genai
            // places it on the **static prefix**, the tools plus system block, which is
            // exactly the part those two decisions hold still. Message-level breakpoints stay
            // unused; a rolling one over the transcript is a separate question with its own
            // cost, and this is the one that pays on every turn of every conversation.
            //
            // Needs genai 0.7 — 0.6.5 ignored request-level cache control for Anthropic
            // outright, which is why the pin moved.
            .with_cache_control(CacheControl::Ephemeral);
        if let Some(effort) = selection.effort {
            options = options.with_reasoning_effort(effort.genai());
        }

        Ok(Brain {
            client: builder.build(),
            // A `ModelIden` rather than a bare name, so nothing is inferred from spelling:
            // `AdapterKind::from_model` falls back to Ollama for an unrecognized name, which
            // for a roster entry that names a provider explicitly would be a silent misroute.
            model: ModelIden::new(provider.adapter(model), model.to_string()),
            options,
        })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn model(&self) -> &ModelIden {
        &self.model
    }

    pub fn options(&self) -> &ChatOptions {
        &self.options
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pool a test resolves against. Real, because `Brain::resolve` takes the app's own —
    /// building one is cheap and shapes no production signature.
    fn pool() -> reqwest::Client {
        reqwest::Client::builder().build().unwrap()
    }

    /// The table is the vocabulary, so every kind must have exactly one row and find it.
    #[test]
    fn every_kind_has_its_own_row() {
        // The whole property: a kind's row is *its* row. A mis-indexed arm in `info` would
        // hand out another provider's env var and effort ladder, and this is what catches it.
        // `all()` reads the table, so there is no second list to check it against.
        for kind in ProviderKind::all() {
            assert_eq!(kind.info().kind, kind);
        }
        assert_eq!(ProviderKind::all().count(), PROVIDERS.len());
    }

    /// **Reasoning is a model capability, so the menu is per model.** Every kind with a rule
    /// answers both ways for models of its own: the modern Claude models take an effort and
    /// `claude-sonnet-4-5` does not, `gpt-5` does and `gpt-4o` does not.
    #[test]
    fn the_ladder_is_offered_per_model_not_per_provider() {
        let anthropic = ProviderKind::Anthropic;
        for model in [
            "claude-opus-4-5",
            "claude-opus-4-6",
            "claude-opus-4-8",
            "claude-opus-5-0",
            "claude-sonnet-4-6",
            "claude-sonnet-5",
            "claude-fable-5",
        ] {
            assert_eq!(anthropic.efforts(model), LADDER, "{model}");
        }
        assert!(anthropic.efforts("claude-sonnet-4-5").is_empty());
        assert!(anthropic.efforts("claude-haiku-4-5").is_empty());

        let openai = ProviderKind::OpenAi;
        assert_eq!(openai.efforts("gpt-5.2"), LADDER);
        assert_eq!(openai.efforts("o3-mini"), LADDER);
        assert!(openai.efforts("gpt-4o").is_empty());

        let gemini = ProviderKind::Gemini;
        assert_eq!(gemini.efforts("gemini-3-pro-preview"), LADDER);
        assert_eq!(gemini.efforts("gemini-2.5-flash"), LADDER);
        assert!(gemini.efforts("gemini-2.0-flash").is_empty());
    }

    /// The two kinds whose answer does not depend on the model, and why they differ: Ollama's
    /// API has no such field for any model, and a compatible endpoint is the user's own, so
    /// they are the authority on what it accepts.
    #[test]
    fn a_kind_may_answer_the_same_for_every_model() {
        for model in ["qwen3:14b", "gpt-5", "anything"] {
            assert!(ProviderKind::Ollama.efforts(model).is_empty(), "{model}");
            assert_eq!(ProviderKind::OpenAiCompatible.efforts(model), LADDER);
        }
    }

    /// An unknown name gets no control rather than a broken one. The rules are name fragments
    /// and will fall behind what the providers ship; falling behind must cost a knob, never a
    /// menu whose settings the provider refuses.
    #[test]
    fn an_unrecognized_model_is_closed_not_open() {
        for kind in [
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::Gemini,
        ] {
            assert!(
                kind.efforts("some-model-shipped-next-quarter").is_empty(),
                "{kind}"
            );
        }
    }

    #[test]
    fn an_effort_a_kind_does_not_offer_is_refused_before_any_call() {
        let selection = Selection::new(ProviderKind::Ollama, "qwen3:14b").with_effort(Effort::High);
        let Err(e) = Brain::resolve(&selection, &pool()) else {
            panic!("Ollama has no effort control, so this cannot make a client");
        };
        assert_eq!(
            e,
            SelectionError::NoSuchEffort {
                kind: ProviderKind::Ollama,
                model: "qwen3:14b".into(),
                effort: Effort::High,
            }
        );
        assert_eq!(
            e.to_string(),
            "Ollama 'qwen3:14b' has no reasoning effort setting."
        );

        // And the message names the *model*, because that is what the user would change.
        let sonnet = Selection::new(ProviderKind::Anthropic, "claude-sonnet-4-5")
            .with_key(Secret::new("sk-test").unwrap())
            .with_effort(Effort::High);
        assert_eq!(
            Brain::resolve(&sonnet, &pool())
                .err()
                .map(|e| e.to_string()),
            Some("Anthropic 'claude-sonnet-4-5' has no reasoning effort setting.".to_string())
        );

        // The model of the same kind that does support one resolves with the rung set.
        let opus = Selection::new(ProviderKind::Anthropic, "claude-opus-4-5")
            .with_key(Secret::new("sk-test").unwrap())
            .with_effort(Effort::High);
        let brain = Brain::resolve(&opus, &pool()).unwrap();
        assert!(brain.options().reasoning_effort.is_some());
    }

    #[test]
    fn a_compatible_endpoint_without_a_url_names_the_field_and_the_pane() {
        let selection = Selection::new(ProviderKind::OpenAiCompatible, "llama-3.3-70b");
        let Err(e) = Brain::resolve(&selection, &pool()) else {
            panic!("a compatible endpoint has no address without a base URL");
        };
        assert_eq!(
            e.to_string(),
            "OpenAI-compatible needs a base URL. Set one in Settings > Assistant."
        );
    }

    /// A keyed provider with nothing in the roster falls back to its variable, and says which
    /// one when that is empty too.
    ///
    /// Read rather than set: mutating the process environment from a test races every other
    /// test in the binary, and both branches are worth asserting anyway — a developer with a
    /// real key exported is exercising the fallback that ships.
    #[test]
    fn a_keyed_provider_with_no_key_anywhere_names_its_variable() {
        let KeyUse::Env(var) = ProviderKind::Anthropic.info().key else {
            panic!("Anthropic is a keyed provider");
        };
        let resolved = Brain::resolve(
            &Selection::new(ProviderKind::Anthropic, "claude-sonnet-4-5"),
            &pool(),
        );
        match env::var(var) {
            Ok(value) if !value.trim().is_empty() => assert!(
                resolved.is_ok(),
                "'{var}' is set, so the environment fallback should have made a client"
            ),
            _ => assert_eq!(
                resolved.err().map(|e| e.to_string()),
                Some(
                    "Anthropic needs an API key. Set one in Settings > Assistant, or set \
                     'ANTHROPIC_API_KEY'."
                        .to_string()
                )
            ),
        }
    }

    /// The trailing slash is the whole reason this check exists: every adapter joins its path
    /// onto the base, and both join rules lose or corrupt a segment without one.
    #[test]
    fn a_base_url_is_normalized_to_the_slash_every_adapter_joins_onto() {
        assert_eq!(
            Provider::check_base_url("http://localhost:11434").unwrap(),
            "http://localhost:11434/"
        );
        assert_eq!(
            Provider::check_base_url(" https://gateway.example/v1/ ").unwrap(),
            "https://gateway.example/v1/"
        );
        assert!(Provider::check_base_url("localhost:11434").is_err());
        assert!(Provider::check_base_url("ftp://host/v1/").is_err());
        assert!(Provider::check_base_url("   ").is_err());
    }

    /// **The slash belongs to the path, not to the text.** A gateway base carrying an
    /// api-version query is a shape genai's OpenAI adapter supports on purpose, and appending
    /// to the raw string put the slash inside the query — so the join then ate `/v1` and the
    /// version reached the wire corrupted.
    #[test]
    fn a_query_bearing_base_url_keeps_its_query() {
        assert_eq!(
            Provider::check_base_url("https://gw.example/v1?api-version=2024-02-01").unwrap(),
            "https://gw.example/v1/?api-version=2024-02-01"
        );
        assert_eq!(
            Provider::check_base_url("https://gw.example/v1/?api-version=2024-02-01").unwrap(),
            "https://gw.example/v1/?api-version=2024-02-01"
        );
    }

    /// A provider that owns its address must not silently ignore one that was typed anyway.
    #[test]
    fn a_base_url_on_a_provider_that_owns_its_endpoint_is_refused() {
        let selection = Selection::new(ProviderKind::Anthropic, "claude-sonnet-4-5")
            .with_key(Secret::new("sk-test").unwrap())
            .with_base_url("https://proxy.example/v1/");
        assert_eq!(
            Brain::resolve(&selection, &pool()).err(),
            Some(SelectionError::BaseUrlNotUsed {
                kind: ProviderKind::Anthropic
            })
        );
    }

    /// A key on a provider that sends none is the same lie in the other direction.
    #[test]
    fn a_key_on_a_provider_that_takes_none_is_refused() {
        let selection = Selection::new(ProviderKind::Ollama, "qwen3:14b")
            .with_key(Secret::new("sk-test").unwrap());
        assert_eq!(
            Brain::resolve(&selection, &pool()).err(),
            Some(SelectionError::KeyNotUsed {
                kind: ProviderKind::Ollama
            })
        );
    }

    /// The compatible kind must never reach for `OPENAI_API_KEY`: the endpoint is whatever
    /// host the user typed, and genai's own default for that adapter would post their OpenAI
    /// key to it.
    #[test]
    fn a_compatible_endpoint_has_no_environment_fallback() {
        assert_eq!(ProviderKind::OpenAiCompatible.info().key, KeyUse::Anonymous);
        let brain = Brain::resolve(
            &Selection::new(ProviderKind::OpenAiCompatible, "llama-3.3-70b")
                .with_base_url("http://localhost:8080/v1"),
            &pool(),
        )
        .unwrap();
        assert_eq!(brain.model().adapter_kind, AdapterKind::OpenAI);
    }

    /// **A blank box is a box the user cleared.** Matching on presence alone answered it with
    /// "takes no base URL. Clear it in Settings" — an instruction already followed.
    #[test]
    fn a_blank_base_url_reads_as_absent_not_as_present() {
        for blank in ["", "   "] {
            let selection = Selection::new(ProviderKind::Ollama, "qwen3:14b").with_base_url(blank);
            let brain = Brain::resolve(&selection, &pool())
                .unwrap_or_else(|e| panic!("a cleared box is not a value: {e}"));
            assert_eq!(brain.model().adapter_kind, AdapterKind::Ollama);
        }
    }

    /// Reasoning has to survive a tool round, and on genai's OpenAI Responses adapter this
    /// flag is what carries it — without it a gpt-5 tool loop re-sends its calls with the
    /// reasoning item missing in front of them.
    #[test]
    fn reasoning_content_is_captured_so_it_can_be_sent_back() {
        let brain =
            Brain::resolve(&Selection::new(ProviderKind::Ollama, "qwen3:14b"), &pool()).unwrap();
        assert_eq!(brain.options().capture_reasoning_content, Some(true));
    }

    /// The OpenAI kind is two adapters, and the fork is genai's own knowledge — but only
    /// inside the family: an unrecognized name must not fall through to Ollama.
    #[test]
    fn the_openai_kind_routes_a_responses_model_and_never_leaves_the_family() {
        let openai = ProviderKind::OpenAi.info();
        assert_eq!(openai.adapter("gpt-5"), AdapterKind::OpenAIResp);
        assert_eq!(openai.adapter("gpt-4o"), AdapterKind::OpenAI);
        assert_eq!(
            openai.adapter("some-private-deployment"),
            AdapterKind::OpenAI
        );
    }

    /// Ollama needs nothing but a model name, which is the point of it being in the roster.
    #[test]
    fn ollama_resolves_with_nothing_configured() {
        let brain =
            Brain::resolve(&Selection::new(ProviderKind::Ollama, "qwen3:14b"), &pool()).unwrap();
        assert_eq!(brain.model().adapter_kind, AdapterKind::Ollama);
        assert!(brain.options().reasoning_effort.is_none());
    }
}
