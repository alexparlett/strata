//! **The provider seam** — which brain answers a send, and the one table every surface reads
//! that from.
//!
//! Three surfaces have to agree about providers and none may restate the others: Settings
//! maintains the roster, a conversation holds the pick, and this module turns a resolved pick into
//! a `genai` client. So the kinds, their labels, their key and base-URL policy and **which effort
//! rungs they offer** are one table ([`PROVIDERS`]), and a form that offers a field the table does
//! not declare does not compile against it.
//!
//! [`ProviderKind`] and [`Effort`] live in `strata_core::ai` because they are what
//! [`Settings`](strata_core::config::Settings) persists; the accessors here ([`info`], [`label`],
//! [`efforts`]) are free functions for that reason, and a kind added without a row is still a
//! build error.
//!
//! [`Selection`] is plain data handed in with every send — nothing here reads Settings or holds a
//! client between turns, which is the whole of "several chat panes, each on its own provider": two
//! conversations disagreeing is two values, not a mode.
//!
//! **Effort is offered per model, and mapped per model by `genai`.** Whether the control exists is
//! the kind's [`Efforts`] rule ("will the pinned `genai` actually send an effort for this model"),
//! and a [`Selection`] that sets a rung the model has no menu for is refused rather than silently
//! ignored. What a rung *means* is `genai`'s, already mapped per model at the pinned version. The
//! rules are name fragments, so they fall behind what providers ship — which is why
//! [`Efforts::Only`] is **default-closed**: falling behind costs a knob, never a refused request.
//!
//! [`Brain::resolve`] is the only place a `genai::Client` is built, and it either builds one or
//! names the missing field and the pane it is set in ([`SelectionError`]) — with no network attempt
//! either way, so the chat pane degrades honestly instead of reporting a timeout for an empty box.

use std::env;
use std::fmt;

use genai::adapter::AdapterKind;
use genai::chat::{CacheControl, ChatOptions, ReasoningEffort};
use genai::resolver::{
    AuthData, AuthResolver, Endpoint, Error as ResolverError, ProviderConfig, ServiceTargetResolver,
};
use genai::{Client, ModelIden, ServiceTarget};
use strata_core::ai::{Effort, ProviderKind};
use strata_core::secret::Secret;
use url::Url;

use crate::assistant::turn::bounded_error;

/// **Does this adapter parse a reasoning keyword off the end of a model name?**
///
/// It happens only when no explicit effort is set: the adapter takes a trailing `-<keyword>` as
/// the reasoning setting and sends the *prefix* as the model. [`Brain::resolve`] refuses that
/// case rather than let a request name a model the user did not pick; this is the half of the
/// rule that says where to look.
///
/// **The list is longer than the adapters that contain the parse**, because a pass-through
/// adapter inherits it: `impl_pass_through_adapter!` forwards `to_web_request_data` verbatim to
/// its delegate, so `DeepSeek`, Groq and xAI all run `OpenAIAdapter::util_to_web_request_data` —
/// and with it `ReasoningEffort::from_model_name` — under their own [`AdapterKind`]. Reading
/// only the three adapters that *implement* the parse would leave `grok-4-max` quietly querying
/// `grok-4`, which is the exact failure this exists to prevent, arrived at through delegation.
fn strips_effort_suffix(adapter: AdapterKind) -> bool {
    matches!(
        adapter,
        AdapterKind::Anthropic
            | AdapterKind::OpenAI
            | AdapterKind::OpenAIResp
            | AdapterKind::DeepSeek
            | AdapterKind::Groq
            | AdapterKind::Xai
    )
}

/// The rung, in `genai`'s vocabulary.
///
/// A free function rather than a method, because [`Effort`] is `strata-core`'s — it is a
/// persisted token, and this is the one place the token becomes a request field.
fn genai_effort(effort: Effort) -> ReasoningEffort {
    match effort {
        Effort::Low => ReasoningEffort::Low,
        Effort::Medium => ReasoningEffort::Medium,
        Effort::High => ReasoningEffort::High,
        Effort::XHigh => ReasoningEffort::XHigh,
        Effort::Max => ReasoningEffort::Max,
    }
}

/// Every rung, for the one kind whose models we cannot know.
const LADDER: &[Effort] = &[
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::XHigh,
    Effort::Max,
];

/// The three rungs every reasoning model accepts. `XHigh` and `Max` are newer and narrower, and
/// **a rung a model does not accept is not a rung to offer**: `genai` clamps `Max` down to
/// `"high"` for Anthropic and Gemini without telling anyone, and passes it through verbatim for
/// OpenAI, which has no `max` value at all. Either way the footer would name a rung that was not
/// what got sent, which is the same "a field silently ignored is a lie on screen" the base URL
/// and the key are refused for.
const KEYWORDS: &[Effort] = &[Effort::Low, Effort::Medium, Effort::High];

/// One family of models and the rungs it really accepts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rungs {
    /// Matched with `contains`, against a name the user typed free-form.
    pub models: &'static [&'static str],
    /// Names that `models` matches but must not claim — the non-reasoning variants that share a
    /// prefix with a reasoning model. `contains` is open in the over-match direction, and
    /// default-closed does nothing about that: `gpt-5-chat-latest` contains `gpt-5`.
    pub except: &'static [&'static str],
    /// The rungs these models accept.
    pub rungs: &'static [Effort],
}

/// **Which models of a kind offer which reasoning rungs** — the rule, per kind, in the table.
///
/// Reasoning is a *model* capability, not a provider one: `claude-opus-4-5` takes an effort and
/// `claude-sonnet-4-5` does not. A per-kind answer is wrong in both directions — it hides a control
/// that works, or offers one that breaks the turn — and the answer is a **set of rungs** rather
/// than a yes/no, because the vendors disagree about the top of the ladder.
///
/// **[`Only`](Efforts::Only) is default-closed**, which is the safety argument for keeping name
/// lists at all: they fall behind what providers ship, and falling behind must cost a knob the user
/// can report rather than a menu whose settings the provider refuses. Not a *complete* argument,
/// because `contains` also over-matches — that is what [`Rungs::except`] is for.
///
/// These lists mirror what the **pinned** `genai` will actually send, not its matching *mechanism*.
/// A `genai` bump is the moment to revisit them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Efforts {
    /// No model of this kind has one. Ollama's API carries no such field at all.
    Never,
    /// Every model, every rung: the endpoint is the user's own and they are the authority on
    /// what it accepts. Nothing is sent unless a rung is picked, and an endpoint that rejects
    /// the field says so in its own words.
    Always,
    /// Only these families, each with the rungs it accepts.
    Only(&'static [Rungs]),
}

impl Efforts {
    /// The rungs `model` offers — empty when it offers none.
    pub fn rungs(&self, model: &str) -> &'static [Effort] {
        match self {
            Efforts::Never => &[],
            Efforts::Always => LADDER,
            Efforts::Only(families) => families
                .iter()
                .find(|family| {
                    family.models.iter().any(|name| model.contains(name))
                        && !family.except.iter().any(|name| model.contains(name))
                })
                .map_or(&[][..], |family| family.rungs),
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
    /// Which kind this is.
    pub kind: ProviderKind,
    /// What every surface calls it.
    pub label: &'static str,
    /// Whether it takes an endpoint, and what its default is.
    pub base_url: BaseUrl,
    /// Whether it takes a key, and which environment variable it falls back to.
    pub key: KeyUse,
    /// Which of this kind's models offer a reasoning control, and therefore whether the
    /// composer footer draws one for the model in hand. Ask it through
    /// [`efforts`]; a [`Selection`] carrying a rung the model does not offer is
    /// refused rather than sent.
    pub efforts: Efforts,
    /// The `genai` adapter this kind routes through. Private: no surface outside this module
    /// has any business naming one, and [`Provider::adapter`] is what resolves the one kind
    /// that is two adapters.
    adapter: AdapterKind,
}

/// **The table.** One row per kind, and the only place any of these facts is written.
pub const PROVIDERS: [Provider; 8] = [
    Provider {
        kind: ProviderKind::Anthropic,
        label: "Anthropic",
        base_url: BaseUrl::Provider,
        key: KeyUse::Env("ANTHROPIC_API_KEY"),
        efforts: Efforts::Only(&[
            Rungs {
                models: &["claude-opus-4-5"],
                except: &[],
                rungs: KEYWORDS,
            },
            Rungs {
                models: &["claude-opus-5", "claude-sonnet-5", "fable", "mythos"],
                except: &[],
                rungs: LADDER,
            },
        ]),
        adapter: AdapterKind::Anthropic,
    },
    Provider {
        kind: ProviderKind::OpenAi,
        label: "OpenAI",
        base_url: BaseUrl::Provider,
        key: KeyUse::Env("OPENAI_API_KEY"),
        efforts: Efforts::Only(&[Rungs {
            models: &["gpt-5", "gpt-6", "codex", "o1", "o3", "o4"],
            except: &["-chat", "o1-mini", "o1-preview"],
            rungs: KEYWORDS,
        }]),
        adapter: AdapterKind::OpenAI,
    },
    Provider {
        kind: ProviderKind::Gemini,
        label: "Gemini",
        base_url: BaseUrl::Provider,
        key: KeyUse::Env("GEMINI_API_KEY"),
        efforts: Efforts::Only(&[Rungs {
            models: &["gemini-2.5", "gemini-3", "gemma-4"],
            except: &[],
            rungs: KEYWORDS,
        }]),
        adapter: AdapterKind::Gemini,
    },
    Provider {
        kind: ProviderKind::DeepSeek,
        label: "DeepSeek",
        base_url: BaseUrl::Provider,
        key: KeyUse::Env("DEEPSEEK_API_KEY"),
        efforts: Efforts::Only(&[]),
        adapter: AdapterKind::DeepSeek,
    },
    Provider {
        kind: ProviderKind::Groq,
        label: "Groq",
        base_url: BaseUrl::Provider,
        key: KeyUse::Env("GROQ_API_KEY"),
        efforts: Efforts::Only(&[]),
        adapter: AdapterKind::Groq,
    },
    Provider {
        kind: ProviderKind::Xai,
        label: "xAI",
        base_url: BaseUrl::Provider,
        key: KeyUse::Env("XAI_API_KEY"),
        efforts: Efforts::Only(&[]),
        adapter: AdapterKind::Xai,
    },
    Provider {
        kind: ProviderKind::Ollama,
        label: "Ollama",
        base_url: BaseUrl::Editable("http://localhost:11434/"),
        key: KeyUse::Unused,
        efforts: Efforts::Never,
        adapter: AdapterKind::Ollama,
    },
    Provider {
        kind: ProviderKind::OpenAiCompatible,
        label: "OpenAI-compatible",
        base_url: BaseUrl::Required,
        key: KeyUse::Anonymous,
        efforts: Efforts::Always,
        adapter: AdapterKind::OpenAI,
    },
];

/// Every kind, in the order Settings lists them — **read off the table**, so there is no second
/// list of the kinds to fall out of step with it. A fixed-size array literal here would keep
/// compiling after a variant was added, and the new provider would be silently missing from
/// every surface built from this.
pub fn all() -> impl Iterator<Item = ProviderKind> {
    PROVIDERS.iter().map(|provider| provider.kind)
}

/// This kind's row of [`PROVIDERS`]. A match rather than an index, so a kind added without a
/// row is a build error rather than a panic on the day somebody picks it.
///
/// Free functions rather than inherent methods because [`ProviderKind`] is `strata-core`'s —
/// it is what [`Settings`](strata_core::config::Settings) persists, and the crate that holds
/// the config cannot depend on the crate that holds `genai`. The property is unchanged: still
/// one table, still one exhaustive match, still a build error for a kind without a row.
pub fn info(kind: ProviderKind) -> &'static Provider {
    match kind {
        ProviderKind::Anthropic => &PROVIDERS[0],
        ProviderKind::OpenAi => &PROVIDERS[1],
        ProviderKind::Gemini => &PROVIDERS[2],
        ProviderKind::DeepSeek => &PROVIDERS[3],
        ProviderKind::Groq => &PROVIDERS[4],
        ProviderKind::Xai => &PROVIDERS[5],
        ProviderKind::Ollama => &PROVIDERS[6],
        ProviderKind::OpenAiCompatible => &PROVIDERS[7],
    }
}

/// What every surface calls this kind.
pub fn label(kind: ProviderKind) -> &'static str {
    info(kind).label
}

/// The effort rungs `model` offers — empty when it offers none, which is what a picker draws no
/// control for. Per **model**, because reasoning is a model capability: `claude-opus-4-5` takes
/// an effort and `claude-sonnet-4-5` does not.
pub fn efforts(kind: ProviderKind, model: &str) -> &'static [Effort] {
    info(kind).efforts.rungs(model)
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
    /// Normalizing the trailing slash is the load-bearing half: adapters join their path onto this,
    /// so without it `http://host/v1` reaches `http://host/chat/completions` (`Url::join` replaces
    /// the last segment) and `http://localhost:11434` reaches `http://localhost:11434api/chat`.
    ///
    /// **The slash goes on the parsed URL's path, never on the raw text.** A base URL may carry a
    /// query, so appending to the text would put the slash inside it —
    /// `https://gw.example/v1?api-version=2024-02-01` reaching
    /// `https://gw.example/chat/completions?api-version=2024-02-01%2F`, the `/v1` eaten and the
    /// version corrupted.
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
    /// Which provider to send to.
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

    /// The selection with `url` as its endpoint.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Selection {
        self.base_url = Some(url.into());
        self
    }

    /// The selection with `key` as its credential.
    pub fn with_key(mut self, key: Secret) -> Selection {
        self.api_key = Some(key);
        self
    }

    /// The selection asking for `effort`.
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
///
/// **Each one names the surface that fixes it, and there are two.** A provider entry carries
/// what addresses the provider — its endpoint and its key — and a conversation carries what the
/// provider is asked. So a base URL or a key sends the user to Settings ▸ AI ▸ Providers, and a
/// model or an effort sends them to the chat pane's own pickers. Naming one surface for both
/// was true only while the roster held a default model, which it does not (AS-03).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SelectionError {
    /// No model name.
    NoModel {
        /// The provider asked.
        kind: ProviderKind,
    },
    /// The kind needs a base URL and the selection carries none.
    NoBaseUrl {
        /// The provider asked.
        kind: ProviderKind,
    },
    /// The kind has no base URL to set, and one was set anyway. Refused rather than dropped:
    /// a field that is silently ignored is a lie on screen.
    BaseUrlNotUsed {
        /// The provider asked.
        kind: ProviderKind,
    },
    /// The base URL is not a URL this can call.
    BadBaseUrl {
        /// The URL as it was given.
        url: String,
        /// What is wrong with it.
        why: String,
    },
    /// A keyed provider with no key in the roster and nothing in its environment variable.
    NoKey {
        /// The provider asked.
        kind: ProviderKind,
        /// The environment variable it falls back to.
        env: &'static str,
    },
    /// A key was set for a provider that takes none.
    KeyNotUsed {
        /// The provider asked.
        kind: ProviderKind,
    },
    /// An effort rung this **model** does not offer — including any rung at all for a model
    /// with no reasoning control.
    NoSuchEffort {
        /// The provider asked.
        kind: ProviderKind,
        /// The model asked.
        model: String,
        /// The rung it does not offer.
        effort: Effort,
    },
    /// The model's own name ends in what the adapter reads as a reasoning keyword, so the
    /// request would name a **different model** than the one on screen.
    ModelReadsAsEffort {
        /// The provider asked.
        kind: ProviderKind,
        /// The model as it is spelled on screen.
        model: String,
        /// The name the request would actually carry.
        sent: String,
        /// The rung the adapter read out of the suffix.
        read_as: String,
    },
}

impl fmt::Display for SelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SelectionError::NoModel { kind } => {
                let kind = label(*kind);
                write!(f, "Choose a model for {kind} in the chat pane.")
            }
            SelectionError::NoBaseUrl { kind } => {
                let kind = label(*kind);
                write!(
                    f,
                    "{kind} needs a base URL. Set one in Settings > AI > Providers."
                )
            }
            SelectionError::BaseUrlNotUsed { kind } => {
                let kind = label(*kind);
                write!(
                    f,
                    "{kind} has its own endpoint and takes no base URL. Clear it in Settings > \
                     AI > Providers."
                )
            }
            SelectionError::BadBaseUrl { url, why } => write!(f, "{why} Base URL: '{url}'."),
            SelectionError::NoKey { kind, env } => {
                let kind = label(*kind);
                write!(
                    f,
                    "{kind} needs an API key. Set one in Settings > AI > Providers, or set \
                     '{env}'."
                )
            }
            SelectionError::KeyNotUsed { kind } => {
                let kind = label(*kind);
                write!(
                    f,
                    "{kind} takes no API key. Clear it in Settings > AI > Providers."
                )
            }
            SelectionError::NoSuchEffort {
                kind,
                model,
                effort,
            } => {
                let rungs = efforts(*kind, model);
                let kind = label(*kind);
                match rungs {
                    [] => write!(f, "{kind} '{model}' has no reasoning effort setting."),
                    _ => write!(
                        f,
                        "{kind} '{model}' does not offer '{effort}' reasoning effort."
                    ),
                }
            }
            SelectionError::ModelReadsAsEffort {
                kind,
                model,
                sent,
                read_as,
            } => {
                let rungs = efforts(*kind, model);
                let name = label(*kind);
                write!(
                    f,
                    "{name} '{model}' ends with '-{read_as}', which is read as a reasoning \
                     setting rather than part of the name, so the request would ask for \
                     '{sent}'."
                )?;
                match rungs.is_empty() {
                    true => write!(f, " Choose a different model in the chat pane."),
                    false => write!(f, " Choose a reasoning effort to send the full name."),
                }
            }
        }
    }
}

impl std::error::Error for SelectionError {}

/// **Where a kind's requests go.** `None` means "genai's default for the adapter", which is the
/// right answer for exactly the kinds that own their address.
///
/// A blank box is **absent**, not present-and-empty: a text input yields `Some("")` for a field
/// the user has already cleared, and matching on presence alone would answer that with "takes no
/// base URL. Clear it in Settings" — an instruction they have already followed and cannot follow
/// again. Same reading the model gets.
///
/// Its own function because two callers need the same answer: [`Brain::resolve`], and
/// [`list_models`] — which runs *before* a model exists and so cannot go through the other one.
fn address(kind: ProviderKind, base_url: Option<&str>) -> Result<Option<String>, SelectionError> {
    let typed = base_url.map(str::trim).filter(|url| !url.is_empty());
    match (info(kind).base_url, typed) {
        (BaseUrl::Provider, None) => Ok(None),
        (BaseUrl::Provider, Some(_)) => Err(SelectionError::BaseUrlNotUsed { kind }),
        (BaseUrl::Editable(default), None) => Ok(Some(default.to_string())),
        (BaseUrl::Required, None) => Err(SelectionError::NoBaseUrl { kind }),
        (BaseUrl::Editable(_) | BaseUrl::Required, Some(url)) => {
            Ok(Some(Provider::check_base_url(url).map_err(|why| {
                SelectionError::BadBaseUrl {
                    url: url.to_string(),
                    why,
                }
            })?))
        }
    }
}

/// **What a kind's requests authenticate with.** `None` means "genai's default", which is only
/// ever taken by a kind whose default is not a key at all (Ollama's constant).
///
/// Split out beside [`address`] and for the same reason.
fn credential(
    kind: ProviderKind,
    key: Option<&Secret>,
) -> Result<Option<AuthData>, SelectionError> {
    match (info(kind).key, key) {
        (KeyUse::Unused, Some(_)) => Err(SelectionError::KeyNotUsed { kind }),
        (KeyUse::Unused, None) => Ok(None),
        (KeyUse::Env(_) | KeyUse::Anonymous, Some(key)) => {
            Ok(Some(AuthData::Key(key.expose().to_string())))
        }
        (KeyUse::Env(var), None) => match env::var(var) {
            Ok(value) if !value.trim().is_empty() => Ok(Some(AuthData::from_env(var))),
            _ => Err(SelectionError::NoKey { kind, env: var }),
        },
        (KeyUse::Anonymous, None) => Ok(Some(AuthData::Key(String::new()))),
    }
}

/// **The models this provider reports, asked with the credential it is configured with.**
///
/// Settings ▸ AI's Test action *and* its model dropdown, which are one call rather than two:
/// there is no ping in `genai` and there does not need to be, because listing the models is a
/// live request against the endpoint with the credential — exactly what a test proves — and its
/// answer is exactly what the picker needs. A separate reachability probe would be a second
/// round trip that proves strictly less.
///
/// The two refusals come first, so a missing key or a malformed URL is named as itself rather
/// than arriving as a 401 or a DNS failure. Everything past that is the provider's own words,
/// **bounded** before it is shown: a gateway 5xx is an HTML page, and `genai` carries it into
/// its error whole (the same cut the transcript makes on a turn's failure).
///
/// Ordered and de-duplicated, because a picker is a list a person reads: `genai` returns the
/// provider's own order, which for the OpenAI-shaped endpoints is neither stable nor meaningful.
pub async fn list_models(
    kind: ProviderKind,
    base_url: Option<&str>,
    key: Option<&Secret>,
    pool: &reqwest::Client,
) -> Result<Vec<String>, String> {
    let endpoint = address(kind, base_url).map_err(|e| e.to_string())?;
    let auth = credential(kind, key).map_err(|e| e.to_string())?;

    let adapter = info(kind).adapter;
    let mut config = ProviderConfig::default();
    if let Some(endpoint) = endpoint {
        config = config.with_endpoint(Endpoint::from_owned(endpoint));
    }
    if let Some(auth) = auth {
        config = config.with_auth(auth);
    }

    let client = Client::builder().with_reqwest(pool.clone()).build();
    let mut models = client
        .all_model_names(adapter, config)
        .await
        .map_err(|e| bounded_error(&e.to_string()))?;
    models.sort();
    models.dedup();
    Ok(models)
}

/// [`list_models`], for a caller with no runtime and no pool.
///
/// **Settings' Test press.** The Settings window has neither an [`Assistant`](super::Assistant)
/// nor an `Engine`, and standing an app-wide runtime up so a settings page can press a button
/// once would be a lifetime bought for the wrong reason — so this makes a current-thread runtime
/// and a client, uses them, and drops both. That is the right trade for a one-off press and the
/// wrong one for a turn, which is exactly why the pool is a parameter over there and absent
/// here.
///
/// It **blocks**, so the caller runs it off the render thread (`strata_freya::task::offload`).
/// Living here rather than in the frontend is what keeps `reqwest` and a Tokio runtime out of
/// `strata-freya` entirely: the crate that owns `genai` owns how `genai` is driven.
pub fn list_models_blocking(
    kind: ProviderKind,
    base_url: Option<&str>,
    key: Option<&Secret>,
) -> Result<Vec<String>, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Could not start a worker for the request: {e}."))?;
    let pool = reqwest::Client::builder()
        .build()
        .map_err(|e| format!("Could not build an HTTP client: {e}."))?;
    runtime.block_on(list_models(kind, base_url, key, &pool))
}

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
        let provider = info(kind);

        let model = selection.model.trim();
        if model.is_empty() {
            return Err(SelectionError::NoModel { kind });
        }

        let adapter = provider.adapter(model);

        match selection.effort {
            Some(effort) if !provider.efforts.rungs(model).contains(&effort) => {
                return Err(SelectionError::NoSuchEffort {
                    kind,
                    model: model.to_string(),
                    effort,
                })
            }
            Some(_) => {}
            None if strips_effort_suffix(adapter) => {
                if let (Some(read_as), sent) = ReasoningEffort::from_model_name(model) {
                    return Err(SelectionError::ModelReadsAsEffort {
                        kind,
                        model: model.to_string(),
                        sent: sent.to_string(),
                        read_as: read_as.to_string(),
                    });
                }
            }
            None => {}
        }

        let endpoint = address(kind, selection.base_url.as_deref())?;
        let auth = credential(kind, selection.api_key.as_ref())?;

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
            .with_capture_content(true)
            .with_capture_tool_calls(true);

        if !provider.efforts.rungs(model).is_empty() {
            options = options.with_capture_reasoning_content(true);
        }

        if adapter == AdapterKind::Anthropic {
            options = options.with_cache_control(CacheControl::Ephemeral);
        }

        if let Some(effort) = selection.effort {
            options = options.with_reasoning_effort(genai_effort(effort));
        }

        Ok(Brain {
            client: builder.build(),
            model: ModelIden::new(adapter, model.to_string()),
            options,
        })
    }

    /// The client the selection built.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The model it addresses.
    pub fn model(&self) -> &ModelIden {
        &self.model
    }

    /// The options every send carries.
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
        for kind in all() {
            assert_eq!(info(kind).kind, kind);
        }
        assert_eq!(all().count(), PROVIDERS.len());
    }

    /// **The variable a kind falls back to is genai's, not ours.**
    ///
    /// The name stays written in the table because the pane's help text needs a `&'static str`
    /// to put on screen — but a name typed here and a name genai reads are two copies of one
    /// fact, and the copy on screen is the one the user acts on. So it is asserted against
    /// `AdapterKind::default_key_env_name()` rather than trusted: a genai bump that renames a
    /// variable fails here instead of telling the user to export something nothing reads.
    ///
    /// The same reasoning `ModelReadsAsEffort` asks `ReasoningEffort::from_model_name` rather
    /// than copying its keyword list.
    #[test]
    fn every_environment_fallback_is_the_one_genai_actually_reads() {
        for kind in all() {
            let provider = info(kind);
            let KeyUse::Env(var) = provider.key else {
                continue;
            };
            assert_eq!(
                Some(var),
                provider.adapter.default_key_env_name(),
                "{} names a variable genai does not read",
                label(kind)
            );
        }
    }

    /// **A pass-through adapter inherits the parse it delegates to.** `DeepSeek`, Groq and xAI
    /// forward `to_web_request_data` straight to `OpenAIAdapter`, so a trailing reasoning
    /// keyword is stripped off their model names too — and a guard that only knew the three
    /// adapters *containing* the parse would let `grok-4-max` be sent as `grok-4`.
    #[test]
    fn a_delegating_kind_inherits_its_delegates_name_parse() {
        for (kind, model, sent) in [
            (ProviderKind::Xai, "grok-4-max", "grok-4"),
            (
                ProviderKind::DeepSeek,
                "deepseek-chat-high",
                "deepseek-chat",
            ),
            (ProviderKind::Groq, "llama-3.3-low", "llama-3.3"),
        ] {
            let selection = Selection::new(kind, model).with_key(Secret::new("k").unwrap());
            let Err(SelectionError::ModelReadsAsEffort { sent: asked, .. }) =
                Brain::resolve(&selection, &pool())
            else {
                panic!("{} '{model}' would be sent as '{sent}'", label(kind));
            };
            assert_eq!(asked, sent);
        }
    }

    /// **The three pass-through kinds offer no rung, and that is the closed default working.**
    /// `OpenAIAdapter::insert_openai_reasoning_effort` writes whatever it is given onto the body
    /// with no per-model gate, so a rung offered here is one the provider takes or 400s on —
    /// and which of the two is a fact genai's source cannot answer.
    #[test]
    fn a_kind_whose_models_are_unverified_offers_nothing() {
        for kind in [
            ProviderKind::DeepSeek,
            ProviderKind::Groq,
            ProviderKind::Xai,
        ] {
            for model in ["deepseek-reasoner", "grok-4", "llama-3.3-70b", "anything"] {
                assert!(efforts(kind, model).is_empty(), "{} '{model}'", label(kind));
            }
        }
    }

    /// **Reasoning is a model capability, so the menu is per model.** Every kind with a rule
    /// answers both ways for models of its own: the modern Claude models take an effort and
    /// `claude-sonnet-4-5` does not, `gpt-5` does and `gpt-4o` does not.
    #[test]
    fn the_ladder_is_offered_per_model_not_per_provider() {
        let anthropic = ProviderKind::Anthropic;
        for model in [
            "claude-opus-5-0",
            "claude-sonnet-5",
            "claude-fable-5",
            "claude-mythos-5",
        ] {
            assert_eq!(efforts(anthropic, model), LADDER, "{model}");
        }
        assert_eq!(efforts(anthropic, "claude-opus-4-5"), KEYWORDS);
        for model in [
            "claude-opus-4-6",
            "claude-opus-4-7",
            "claude-opus-4-8",
            "claude-sonnet-4-6",
        ] {
            assert!(efforts(anthropic, model).is_empty(), "{model}");
        }
        assert!(efforts(anthropic, "claude-sonnet-4-5").is_empty());
        assert!(efforts(anthropic, "claude-haiku-4-5").is_empty());

        let openai = ProviderKind::OpenAi;
        assert_eq!(efforts(openai, "gpt-5.2"), KEYWORDS);
        assert_eq!(efforts(openai, "o3-mini"), KEYWORDS);
        assert!(efforts(openai, "gpt-4o").is_empty());
        for model in ["gpt-5-chat-latest", "o1-mini", "o1-preview"] {
            assert!(efforts(openai, model).is_empty(), "{model}");
        }

        let gemini = ProviderKind::Gemini;
        assert_eq!(efforts(gemini, "gemini-3-pro-preview"), KEYWORDS);
        assert_eq!(efforts(gemini, "gemini-2.5-flash"), KEYWORDS);
        assert!(efforts(gemini, "gemini-2.0-flash").is_empty());
    }

    /// **A model whose own name ends in a reasoning keyword is refused, not silently
    /// rewritten.** With no explicit effort the Anthropic and OpenAI adapters parse the suffix
    /// off and send the prefix as the model, so this would query `qwen3` at `qwen3-max`'s
    /// price. Picking a rung stops the parse from ever running, which is what the message says.
    #[test]
    fn a_model_name_that_reads_as_an_effort_is_refused() {
        let compatible = Selection::new(ProviderKind::OpenAiCompatible, "qwen3-max")
            .with_base_url("http://localhost:8080/v1/");
        let Err(refused) = Brain::resolve(&compatible, &pool()) else {
            panic!("a name the adapter rewrites must not be sent");
        };
        assert_eq!(
            refused,
            SelectionError::ModelReadsAsEffort {
                kind: ProviderKind::OpenAiCompatible,
                model: "qwen3-max".into(),
                sent: "qwen3".into(),
                read_as: "max".into(),
            }
        );
        let said = refused.to_string();
        assert!(
            said.contains("'qwen3'"),
            "it names what would be sent: {said}"
        );
        assert!(said.contains("reasoning effort"), "and the way out: {said}");

        let picked = compatible.with_effort(Effort::High);
        let brain = Brain::resolve(&picked, &pool()).expect("an effort keeps the whole name");
        assert_eq!(brain.model().model_name.as_str(), "qwen3-max");

        let ollama = Selection::new(ProviderKind::Ollama, "qwen3-max");
        assert!(Brain::resolve(&ollama, &pool()).is_ok());
    }

    /// **The two options that are not sent flat.** `cache_control` means a prompt-cache
    /// breakpoint only on Anthropic — genai reads it for the OpenAI family as a
    /// `prompt_cache_retention` on the body, which an arbitrary compatible endpoint may
    /// refuse — and `capture_reasoning_content` turns on Gemini's `includeThoughts`, which a
    /// model with no thinking config rejects.
    #[test]
    fn the_conditional_options_are_asked_about_the_model() {
        let claude = Brain::resolve(
            &Selection::new(ProviderKind::Anthropic, "claude-sonnet-5")
                .with_key(Secret::new("k").unwrap()),
            &pool(),
        )
        .unwrap();
        assert!(claude.options().cache_control.is_some());
        assert_eq!(claude.options().capture_reasoning_content, Some(true));

        let compatible = Brain::resolve(
            &Selection::new(ProviderKind::OpenAiCompatible, "llama-3.3-70b")
                .with_base_url("http://localhost:8080/v1/"),
            &pool(),
        )
        .unwrap();
        assert!(
            compatible.options().cache_control.is_none(),
            "an arbitrary endpoint is not sent a field genai turns into prompt_cache_retention"
        );

        let flash = Brain::resolve(
            &Selection::new(ProviderKind::Gemini, "gemini-2.0-flash")
                .with_key(Secret::new("k").unwrap()),
            &pool(),
        )
        .unwrap();
        assert_eq!(flash.options().capture_reasoning_content, None);
    }

    /// The two kinds whose answer does not depend on the model, and why they differ: Ollama's
    /// API has no such field for any model, and a compatible endpoint is the user's own, so
    /// they are the authority on what it accepts.
    #[test]
    fn a_kind_may_answer_the_same_for_every_model() {
        for model in ["qwen3:14b", "gpt-5", "anything"] {
            assert!(efforts(ProviderKind::Ollama, model).is_empty(), "{model}");
            assert_eq!(efforts(ProviderKind::OpenAiCompatible, model), LADDER);
        }
    }

    /// An unknown name gets no control rather than a broken one. The rules are name fragments
    /// and will fall behind what the providers ship; falling behind must cost a knob, never a
    /// menu whose settings the provider refuses.
    #[test]
    fn an_unrecognized_model_is_closed_not_open() {
        for kind in all().filter(|kind| !matches!(info(*kind).efforts, Efforts::Always)) {
            assert!(
                efforts(kind, "some-model-shipped-next-quarter").is_empty(),
                "{}",
                label(kind)
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

        let sonnet = Selection::new(ProviderKind::Anthropic, "claude-sonnet-4-5")
            .with_key(Secret::new("sk-test").unwrap())
            .with_effort(Effort::High);
        assert_eq!(
            Brain::resolve(&sonnet, &pool())
                .err()
                .map(|e| e.to_string()),
            Some("Anthropic 'claude-sonnet-4-5' has no reasoning effort setting.".to_string())
        );

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
            "OpenAI-compatible needs a base URL. Set one in Settings > AI > Providers."
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
        let KeyUse::Env(var) = info(ProviderKind::Anthropic).key else {
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
                    "Anthropic needs an API key. Set one in Settings > AI > Providers, or set \
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
        assert_eq!(info(ProviderKind::OpenAiCompatible).key, KeyUse::Anonymous);
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
        let brain = Brain::resolve(
            &Selection::new(ProviderKind::OpenAi, "gpt-5").with_key(Secret::new("k").unwrap()),
            &pool(),
        )
        .unwrap();
        assert_eq!(brain.options().capture_reasoning_content, Some(true));
    }

    /// The OpenAI kind is two adapters, and the fork is genai's own knowledge — but only
    /// inside the family: an unrecognized name must not fall through to Ollama.
    #[test]
    fn the_openai_kind_routes_a_responses_model_and_never_leaves_the_family() {
        let openai = info(ProviderKind::OpenAi);
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
