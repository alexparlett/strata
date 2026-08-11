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

    /// Drop a pending edit entirely, leaving whatever is stored alone.
    ///
    /// Toggling a provider off records a pending removal; toggling it back on before Apply has to
    /// undo that, or a stray press queues the deletion of a key that is still good.
    pub fn forget(&mut self, kind: ProviderKind) {
        self.0.remove(&kind);
    }

    /// Whether anything at all is typed — a pending key, or a pending *removal*.
    ///
    /// What makes a credential edit count as a change. These live outside the settings draft (a
    /// secret has nowhere in it to live), so a window whose only edit is a pasted key is not
    /// "dirty" by the draft's own reckoning, and Apply would stay disabled with the key
    /// unsaveable. See `SettingsCtx::dirty`.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Forget the keys `committed` holds — and only while they are still what was committed.
    ///
    /// Called once [`commit`] has returned `Ok`: the keystore holds those secrets and the draft
    /// holds their markers, so their pasted text is spent. Keeping it would make a second Apply
    /// in the same window re-`put` keys it had already stored — reachable when a failed config
    /// write leaves the window open to retry.
    ///
    /// **The comparison is the whole method.** Apply runs `commit` on a worker and the window
    /// stays live while it does, so a key typed into another provider mid-flight is in this map
    /// but was never in the snapshot, never stored, and never asked about. Clearing wholesale
    /// would drop it silently — and the box would empty itself in front of the user, since it
    /// mirrors this state. An entry that has changed since the snapshot is a *new* edit and
    /// stays, so the next Apply commits it.
    pub fn forget_committed(&mut self, committed: &TypedKeys) {
        for (kind, landed) in committed.iter() {
            if self.0.get(kind).is_some_and(|current| current == landed) {
                self.0.remove(kind);
            }
        }
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
    use strata_core::secret::SecretRef;

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

    /// **Replace records an empty entry, and that is the whole mechanism.**
    ///
    /// It opens the input, it makes the window dirty so Apply is reachable, and it is what
    /// `commit` turns into a delete — the three things a flag that merely changed what was drawn
    /// would each have left undone, which is how "leave empty to remove" came to do nothing.
    ///
    /// The delete itself is `SecretRef::delete`'s and needs a keystore, so it is asserted where
    /// one can be installed (`strata_core::secret`'s own tests, over `keyring_core::mock`). What
    /// is pinned here is the decision that reaches it.
    #[test]
    fn replacing_records_a_pending_removal() {
        let kind = ProviderKind::Anthropic;
        let mut keys = TypedKeys::default();
        keys.set(kind, String::new());

        assert!(
            keys.touched(kind),
            "commit acts on touched, not on non-empty"
        );
        assert_eq!(keys.get(kind), "", "and an empty value is the delete");
        assert!(!keys.is_empty(), "so Apply is reachable to carry it out");
    }

    /// **Toggling a provider off and back on leaves its key alone.**
    ///
    /// Off records a pending removal so Apply can carry it out; on has to undo that, or a stray
    /// press queues the deletion of a key that is still perfectly good — and leaves the provider
    /// enabled and credential-less, which is the one state Apply refuses.
    #[test]
    fn a_pending_removal_is_undone_by_re_enabling() {
        let kind = ProviderKind::Anthropic;
        let mut keys = TypedKeys::default();

        keys.set(kind, String::new());
        assert!(keys.touched(kind), "off queues the removal");

        keys.forget(kind);
        assert!(!keys.touched(kind), "on takes it back");

        // And `commit` then has nothing to say about this provider at all, so the stored key is
        // untouched rather than deleted.
        let mut ai = Ai {
            providers: [(
                kind,
                ProviderSetup {
                    enabled: true,
                    base_url: String::new(),
                    key: Some(SecretRef::mint()),
                },
            )]
            .into_iter()
            .collect(),
            ..Ai::default()
        };
        let before = ai.clone();
        commit(&keys, &mut ai).expect("nothing pending is nothing to do");
        assert_eq!(ai, before, "the stored key survives the round trip");
    }

    /// **A pasted key survives being toggled around**, because only an *empty* pending entry is a
    /// removal — one carrying a key is an edit the user still wants.
    #[test]
    fn a_pending_key_is_not_a_pending_removal() {
        let kind = ProviderKind::Anthropic;
        let mut keys = TypedKeys::default();
        keys.set(kind, "sk-pasted".into());

        // What `toggle` inspects before dropping anything.
        assert!(!keys.get(kind).trim().is_empty(), "not a removal");
        assert_eq!(keys.get(kind), "sk-pasted");
    }

    /// **A credential edit is a change, though it is not in the draft.**
    ///
    /// These live outside `Settings` because a secret has nowhere in it to live, so the draft
    /// compares equal to its seed when a pasted key is the only edit — and the footer, which
    /// gates Apply on that comparison, left the key unsaveable. It saved at all only when some
    /// other setting happened to change in the same sitting.
    #[test]
    fn a_pending_key_or_removal_counts_as_an_edit() {
        let mut keys = TypedKeys::default();
        assert!(keys.is_empty(), "nothing typed is nothing to apply");

        keys.set(ProviderKind::Anthropic, "sk-pasted".into());
        assert!(!keys.is_empty(), "a pasted key is an edit");

        // Pressing Replace and leaving the box alone: a pending *removal*, and every bit as much
        // an edit as a pending key — `commit` turns it into a delete.
        let mut removal = TypedKeys::default();
        removal.set(ProviderKind::Anthropic, String::new());
        assert!(!removal.is_empty(), "a pending removal is an edit too");
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

        // What `apply` does once `commit` returns `Ok`: the snapshot it handed the worker.
        let committed = keys.clone();
        keys.forget_committed(&committed);

        assert!(!keys.touched(kind), "the retry has nothing to store");
        let mut ai = Ai::default();
        commit(&keys, &mut ai).expect("nothing left to commit");
        assert!(ai.providers.is_empty(), "and nothing to write it into");
    }

    /// **A key typed while an Apply was in flight is not swept up by it.**
    ///
    /// `apply` snapshots the typed keys, hands them to a worker, and *awaits* — keeping the
    /// window live on purpose. So the user can type into another provider before it returns, and
    /// that keystroke is in this map without ever having been in the snapshot, stored, or asked
    /// about. A blanket clear dropped it silently, emptying the box in front of them.
    #[test]
    fn a_key_typed_during_an_apply_survives_it() {
        let mut keys = TypedKeys::default();
        keys.set(ProviderKind::Anthropic, "sk-first".into());

        // What the worker was given.
        let committed = keys.clone();

        // …and what the user typed while it was away.
        keys.set(ProviderKind::Groq, "sk-typed-mid-flight".into());

        keys.forget_committed(&committed);

        assert!(
            !keys.touched(ProviderKind::Anthropic),
            "the committed key is spent"
        );
        assert_eq!(
            keys.get(ProviderKind::Groq),
            "sk-typed-mid-flight",
            "the one that arrived mid-flight was never committed, so it stays"
        );
    }

    /// The same rule on one provider: retyping a key while its own Apply is in flight is a *new*
    /// edit, and the next Apply has to commit it rather than find it gone.
    #[test]
    fn a_key_retyped_during_its_own_apply_is_kept() {
        let kind = ProviderKind::Anthropic;
        let mut keys = TypedKeys::default();
        keys.set(kind, "sk-old".into());
        let committed = keys.clone();

        keys.set(kind, "sk-corrected".into());
        keys.forget_committed(&committed);

        assert_eq!(
            keys.get(kind),
            "sk-corrected",
            "what is in the box now is not what was stored"
        );
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
