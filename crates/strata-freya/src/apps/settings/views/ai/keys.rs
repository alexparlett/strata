//! **Keys typed into AI ▸ Providers and not yet applied.**
//!
//! The settings draft cannot hold these, and that is the design rather than an inconvenience:
//! [`Settings`](strata_core::config::Settings) carries a
//! [`SecretRef`](strata_core::secret::SecretRef) and no secret, so a key reaching `config.json`
//! is not a mistake to be careful about — it is a program that does not compile. A pasted key
//! therefore has nowhere in the draft to live, and lives here instead: for the window's
//! lifetime, in memory, keyed by the provider it belongs to.
//!
//! At Apply it goes to the keystore and only the marker merges ([`commit`]).
//!
//! **A `String`, not a `Secret`.** The box the user pastes into is a `String` — `ValueField`
//! binds one — and wrapping it here would guard the second copy while the first sits in the
//! text field's own buffer, reallocating as it grows. `strata_core::secret`'s own note applies:
//! exposure is managed by lifetime, not by guarding one link of six. What this *does* do is
//! hold the value no longer than the window, and hand it to [`Secret`] at the moment it is
//! written.

use std::collections::BTreeMap;

use strata_core::ai::{Ai, ProviderKind};
use strata_core::secret::{Secret, SecretError, SecretRef};

/// Every key typed in this window and not yet committed.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct TypedKeys(BTreeMap<ProviderKind, String>);

impl TypedKeys {
    /// What the box for `kind` should show — empty when nothing has been typed.
    pub fn get(&self, kind: ProviderKind) -> &str {
        self.0.get(&kind).map_or("", String::as_str)
    }

    /// Record what is in the box. An empty string is kept rather than dropped: "the user
    /// cleared this" is a real edit that has to survive to Apply, where it becomes a delete —
    /// dropping it would make clearing a key indistinguishable from never touching it.
    pub fn set(&mut self, kind: ProviderKind, typed: String) {
        self.0.insert(kind, typed);
    }

    /// Whether anything was typed for `kind` at all. What decides between "write this" and
    /// "leave the stored key alone".
    pub fn touched(&self, kind: ProviderKind) -> bool {
        self.0.contains_key(&kind)
    }

    /// Forget every typed key, because they have all landed.
    ///
    /// Called once [`commit`] has returned `Ok`: the keystore holds the secrets and the draft
    /// holds their markers, so the pasted text is spent. Keeping it would make a second Apply in
    /// the same window re-`put` keys it had already stored — reachable when a failed config write
    /// leaves the window open to retry.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    fn iter(&self) -> impl Iterator<Item = (&ProviderKind, &String)> {
        self.0.iter()
    }
}

/// **Land every typed key in the keystore, and put its marker in `ai`.**
///
/// Called from Apply, *before* the draft merges, so what `write_config` commits already carries
/// the right markers. Three cases per entry, and the third is why the blank string is kept:
///
/// - typed something, no marker yet → mint a [`SecretRef`], store, record it
/// - typed something over an existing marker → overwrite **in place**, so the old keystore entry
///   is replaced rather than stranded under an id nobody remembers
/// - cleared the box → delete the entry and drop the marker ([`Secret::new`] answers a blank
///   string with `None`, so "cleared" and "delete" are the same branch by construction)
///
/// Returns the first failure. A keystore that refuses is reported, never answered by writing the
/// secret somewhere else — the whole point of `strata_core::secret`.
pub fn commit(keys: &TypedKeys, ai: &mut Ai) -> Result<(), SecretError> {
    for (kind, typed) in keys.iter() {
        // **Looking up the existing marker must not create anything.** `entry().or_default()`
        // reads *and* inserts, so a provider the user typed into and then cleared again would
        // gain an empty `ProviderSetup` in the committed config purely by being asked about.
        // The insert belongs below, where a marker is actually being stored.
        let slot = ai.setup(*kind).and_then(|setup| setup.key.clone());
        let had_marker = slot.is_some();

        let marker = match (Secret::new(typed), slot) {
            (Some(secret), Some(existing)) => {
                existing.put(&secret)?;
                Some(existing)
            }
            (Some(secret), None) => {
                let minted = SecretRef::mint();
                minted.put(&secret)?;
                Some(minted)
            }
            (None, Some(existing)) => {
                existing.delete()?;
                None
            }
            (None, None) => None,
        };

        // Typed into and cleared again, with nothing stored to begin with: no marker to record,
        // so nothing to make a row for either.
        if marker.is_none() && !had_marker {
            continue;
        }
        ai.providers.entry(*kind).or_default().key = marker;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_core::ai::ProviderSetup;

    /// **A cleared box is an edit, not an absence.** Dropping the empty string would make
    /// "delete my key" indistinguishable from "never touched it", and the stored key would
    /// survive an Apply the user believed removed it.
    #[test]
    fn clearing_a_key_is_remembered_as_an_edit() {
        let kind = ProviderKind::Anthropic;
        let mut keys = TypedKeys::default();
        assert!(!keys.touched(kind));

        keys.set(kind, String::new());
        assert!(keys.touched(kind), "an empty box is still an edit");
        assert_eq!(keys.get(kind), "");
    }

    /// A provider nobody typed into reads as empty rather than missing, so no caller branches on
    /// presence to draw a box.
    #[test]
    fn an_untyped_provider_reads_as_empty() {
        assert_eq!(TypedKeys::default().get(ProviderKind::Groq), "");
    }

    /// **A committed key is no longer typed, so a retry writes no keys.**
    ///
    /// A successful Apply closes the window, so the way to reach a second Apply is the way the
    /// window is designed to stay open: `write_config` failed and the user retries. That retry
    /// has to be a config write, not a second round of keystore writes — on macOS a repeat
    /// Keychain prompt for a key entered once.
    #[test]
    fn a_committed_key_is_not_offered_to_the_next_apply() {
        let kind = ProviderKind::Anthropic;
        let mut keys = TypedKeys::default();
        keys.set(kind, "sk-typed-once".into());
        assert!(keys.touched(kind));

        // What `apply` does once `commit` returns `Ok`.
        keys.clear();

        assert!(!keys.touched(kind), "the retry has nothing to store");
        let mut ai = Ai::default();
        commit(&keys, &mut ai).expect("nothing left to commit");
        assert!(ai.providers.is_empty(), "and nothing to write it into");
    }

    /// **Asking about a provider's key must not create a row for it.**
    ///
    /// Looking the existing marker up used `entry().or_default()`, which reads *and* inserts — so
    /// a provider the user typed into and then cleared again gained an empty `ProviderSetup` in
    /// the committed config purely by being asked about, and it would persist to disk as a
    /// provider they never enabled.
    #[test]
    fn a_cleared_key_on_an_untouched_provider_creates_no_entry() {
        let mut keys = TypedKeys::default();
        keys.set(ProviderKind::Groq, String::new());

        let mut ai = Ai::default();
        commit(&keys, &mut ai).expect("clearing a key that was never stored is not an error");
        assert!(
            ai.providers.is_empty(),
            "no key, no marker, and so no row: {ai:?}"
        );
    }

    /// **An untouched provider gets no keystore call at all**, which is what lets Apply run on a
    /// draft that never opened the AI pane without asking the OS for anything — and, on macOS,
    /// without a Keychain prompt for a key nobody typed.
    #[test]
    fn a_draft_that_typed_nothing_commits_nothing() {
        let mut ai = Ai {
            providers: [(
                ProviderKind::Anthropic,
                ProviderSetup {
                    enabled: true,
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Ai::default()
        };
        let before = ai.clone();

        commit(&TypedKeys::default(), &mut ai).expect("nothing typed is nothing to do");
        assert_eq!(ai, before, "an empty draft must not rewrite the roster");
    }
}
