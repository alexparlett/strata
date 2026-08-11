//! **What each provider last reported, and when** (AS-06) — the model listings satellite.
//!
//! A model is chosen from what its provider serves, everywhere it is chosen. That only works if
//! the fetched list outlives the window that fetched it: a `Select` whose only content arrives
//! from a network call is an empty `Select` every time the app starts.
//!
//! ## A satellite, on history's precedent
//!
//! Not a field of [`AppConfig`](crate::config::AppConfig). A fetched list is a **cache of a
//! remote fact**, not something the user edited — and the app config is user intent, written
//! through one funnel that notifies the settings audience. Routing a background refresh through
//! it would persist and broadcast a change nobody made, and wake every surface that reads a
//! setting. So this is its own file, exactly as `history.jsonl` is beside the project defs:
//! loaded once at startup as config is, written by the fetch that fills it.
//!
//! It is the **same mechanism** rather than a path invented here — `preferences`, the app's own
//! [`AppInfo`](preferences::AppInfo), and the key `"models"` beside `"config"`. A missing or
//! unreadable file is an empty [`Listings`] and never an error: the expected absence is a first
//! launch.
//!
//! ## It holds names and timestamps, and nothing else
//!
//! No key, no [`SecretRef`](crate::secret::SecretRef), no endpoint. The neighbouring module is
//! [`crate::secret`] and this one stays boring enough that nobody has to check — which is
//! asserted on the serialized bytes below rather than left to the field list, because a test on
//! the fields would pass on the day someone adds one.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use preferences::Preferences;
use serde::{Deserialize, Serialize};

use crate::ai::ProviderKind;
use crate::config::APP_INFO;

/// Key under the config dir, beside `"config"` (the `preferences` crate maps it to a file path).
const KEY: &str = "models";

/// **How old a listing may be before the surface showing it asks again.**
///
/// Stated here rather than left implicit at the poll, because it is the whole of the staleness
/// policy: model rosters move on the order of weeks, so the cost of being a day behind is one
/// missing new name, and the recovery — the Test press in AI ▸ Providers — is already built.
pub const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// One provider's answer to "what do you serve", and when it gave it.
///
/// The names are the provider's own spelling, already sorted and de-duplicated by
/// `list_models`, and deliberately **unfiltered**: the provider names every id it has, so
/// OpenAI's carries `text-embedding-3-large` beside the chat models. Tidying that with a static
/// name list here would be the prescribed-model table this whole design avoids, and it would
/// hide a new chat model on the day it ships.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Listing {
    pub models: Vec<String>,
    pub fetched: SystemTime,
}

impl Listing {
    /// An answer that has just come back.
    pub fn new(models: Vec<String>) -> Listing {
        Listing {
            models,
            fetched: SystemTime::now(),
        }
    }

    /// Whether this answer is old enough to ask again — [`STALE_AFTER`] since it came back.
    ///
    /// **A timestamp in the future is stale**, which is the only sensible reading of a clock
    /// that has moved backwards since the fetch: the alternative is an entry that is never
    /// refreshed again until the machine catches up with it.
    pub fn is_stale(&self) -> bool {
        self.fetched
            .elapsed()
            .map_or(true, |age| age >= STALE_AFTER)
    }
}

/// Every provider's last answer — the whole satellite.
///
/// Keyed by [`ProviderKind`] for the reason the roster is: a provider's identity *is* its kind,
/// so there is nothing to name and nothing to rename.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Listings(BTreeMap<ProviderKind, Listing>);

impl Listings {
    /// What `kind` last reported, or `None` if it has never answered.
    pub fn get(&self, kind: ProviderKind) -> Option<&Listing> {
        self.0.get(&kind)
    }

    /// The names `kind` last reported — empty for a provider that has never answered, which is
    /// the same thing a picker draws for one that answered with nothing.
    pub fn models(&self, kind: ProviderKind) -> &[String] {
        self.get(kind).map_or(&[], |listing| &listing.models)
    }

    /// Record what `kind` just reported, stamped now.
    pub fn set(&mut self, kind: ProviderKind, models: Vec<String>) {
        self.0.insert(kind, Listing::new(models));
    }

    /// Drop what `kind` reported — its address or its credential just changed, so the answer
    /// describes a request nobody would make now. The same retraction the probe makes, on the
    /// same line.
    pub fn forget(&mut self, kind: ProviderKind) {
        self.0.remove(&kind);
    }

    /// Whether a surface showing `kind`'s models should ask again in the background: it has
    /// never answered, or its answer is older than [`STALE_AFTER`].
    pub fn needs_refresh(&self, kind: ProviderKind) -> bool {
        self.get(kind).is_none_or(Listing::is_stale)
    }

    /// **What a picker offers for `kind`: what it reported, plus the pick in hand.**
    ///
    /// Here rather than in either picker, because there are two of them — Settings ▸ AI ▸ Chat's
    /// default and the composer footer's per-conversation pick (AS-04) — and a rule about what
    /// may be selected has to be one rule.
    ///
    /// **The current pick is always selectable**, which is the whole of the union. The list
    /// endpoint is not the chat endpoint: a proxy or a private deployment can serve
    /// `/chat/completions` and no `/models` at all, and an offline laptop serves neither. A
    /// strict picker over an empty or failed answer would strand a setup that works, and would
    /// silently retarget a conversation the first time a fetch failed.
    ///
    /// Inserted in the reported list's own (sorted) order rather than pinned to the front, so
    /// the offer reads as one list rather than as a name someone bolted on.
    pub fn offer(&self, kind: ProviderKind, chosen: &str) -> Vec<String> {
        let mut offered = self.models(kind).to_vec();
        let chosen = chosen.trim();
        if !chosen.is_empty() && !offered.iter().any(|name| name == chosen) {
            let at = offered.partition_point(|name| name.as_str() < chosen);
            offered.insert(at, chosen.to_string());
        }
        offered
    }
}

/// Load the satellite. A missing or unreadable file is an empty [`Listings`] — the expected
/// absence is a first launch, and the recovery for a corrupt one is the fetch that would have
/// happened anyway.
pub fn load() -> Listings {
    Listings::load(&APP_INFO, KEY).unwrap_or_default()
}

/// Persist the satellite. The caller holds the whole value, so this is a plain write and never
/// a load-mutate-save round trip, which would race the in-memory copy it mirrors.
pub fn save(listings: &Listings) -> Result<(), String> {
    listings
        .save(&APP_INFO, KEY)
        .map_err(|e| format!("save model listings: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aged(models: &[&str], age: Duration) -> Listing {
        Listing {
            models: models.iter().map(ToString::to_string).collect(),
            fetched: SystemTime::now() - age,
        }
    }

    /// A provider that has never answered and one whose answer has gone stale ask the same
    /// question of the surface showing them, which is what lets the refresh be one branch.
    #[test]
    fn an_absent_listing_and_an_old_one_both_ask_again() {
        let mut listings = Listings::default();
        assert!(listings.needs_refresh(ProviderKind::OpenAi));

        listings.set(ProviderKind::OpenAi, vec!["gpt-5".into()]);
        assert!(!listings.needs_refresh(ProviderKind::OpenAi));
        assert_eq!(listings.models(ProviderKind::OpenAi), ["gpt-5"]);

        listings
            .0
            .insert(ProviderKind::OpenAi, aged(&["gpt-5"], STALE_AFTER));
        assert!(listings.needs_refresh(ProviderKind::OpenAi));
        // …and it is still usable while it is being asked again, which is the whole of
        // stale-while-revalidate: the names are read from the same entry.
        assert_eq!(listings.models(ProviderKind::OpenAi), ["gpt-5"]);
    }

    /// **A clock that moved backwards leaves a timestamp in the future**, and the only sensible
    /// reading of that is "ask again" — the alternative is an entry nothing refreshes until the
    /// machine catches up with it.
    #[test]
    fn a_listing_stamped_in_the_future_is_stale() {
        let ahead = Listing {
            models: vec!["gpt-5".into()],
            fetched: SystemTime::now() + Duration::from_secs(60),
        };
        assert!(ahead.is_stale());
    }

    /// **A configured model is always selectable**, whatever the provider reported — a gateway
    /// that serves no `/models`, or a laptop with no network, must not strand a setup that
    /// works. And it lands in the list's own order rather than at the front, so the offer reads
    /// as one list.
    #[test]
    fn the_pick_in_hand_is_always_on_offer() {
        let mut listings = Listings::default();
        let kind = ProviderKind::OpenAiCompatible;

        // Nothing reported at all: the pick is the whole offer.
        assert_eq!(listings.offer(kind, "qwen3:32b"), ["qwen3:32b"]);
        // And nothing chosen either is an empty offer, not a blank entry.
        assert!(listings.offer(kind, "").is_empty());
        assert!(listings.offer(kind, "   ").is_empty());

        listings.set(kind, vec!["llama-3.3-70b".into(), "mistral-large".into()]);
        assert_eq!(
            listings.offer(kind, "qwen3:32b"),
            ["llama-3.3-70b", "mistral-large", "qwen3:32b"]
        );
        assert_eq!(
            listings.offer(kind, "gemma-3"),
            ["gemma-3", "llama-3.3-70b", "mistral-large"]
        );
        // A pick the provider already reports is not offered twice.
        assert_eq!(
            listings.offer(kind, "mistral-large"),
            ["llama-3.3-70b", "mistral-large"]
        );
    }

    /// A changed endpoint or credential retracts the answer, exactly as it retracts the probe.
    #[test]
    fn forgetting_a_provider_leaves_nothing_to_offer() {
        let mut listings = Listings::default();
        listings.set(ProviderKind::Anthropic, vec!["claude-sonnet-5".into()]);
        listings.forget(ProviderKind::Anthropic);

        assert!(listings.get(ProviderKind::Anthropic).is_none());
        assert!(listings.models(ProviderKind::Anthropic).is_empty());
        assert!(listings.needs_refresh(ProviderKind::Anthropic));
    }

    /// **The file carries names and timestamps and nothing else**, asserted on the bytes for
    /// `strata_core::ai`'s own reason: the file is what leaks, and a test on the field list
    /// would pass on the day someone adds a field.
    ///
    /// Written through `save_to`/`load_from` rather than the real config dir, so the assertion
    /// is about the format rather than about a machine that ran the suite before.
    #[test]
    fn the_serialized_satellite_is_names_and_timestamps() {
        let mut listings = Listings::default();
        listings.set(
            ProviderKind::Anthropic,
            vec!["claude-sonnet-5".into(), "claude-opus-5".into()],
        );
        listings.set(ProviderKind::OpenAiCompatible, vec!["llama-3.3-70b".into()]);

        let mut written = Vec::new();
        listings.save_to(&mut written).unwrap();
        let text = String::from_utf8(written.clone()).unwrap();

        assert!(text.contains("claude-sonnet-5"), "{text}");
        assert!(text.contains("llama-3.3-70b"), "{text}");
        // The three things a listing is not. It describes a request; it does not carry what the
        // request was made with, and the module beside this one is the secret store.
        for absent in ["key", "secret", "url", "token"] {
            assert!(
                !text.contains(absent),
                "'{absent}' reached the listings file: {text}"
            );
        }

        // And it comes back as itself, timestamps included — a round trip that dropped the
        // stamp would make every entry permanently fresh or permanently stale.
        let read = Listings::load_from(&mut written.as_slice()).unwrap();
        assert_eq!(read, listings);
    }

    /// A satellite that has never been written reads as empty rather than as an error: the
    /// expected absence is a first launch.
    #[test]
    fn an_unwritten_satellite_is_empty() {
        let empty = Listings::load_from(&mut b"{}".as_slice()).unwrap();
        assert_eq!(empty, Listings::default());
        assert!(empty.needs_refresh(ProviderKind::Groq));
    }
}
