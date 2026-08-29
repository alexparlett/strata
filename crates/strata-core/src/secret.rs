//! **The secret store** — the one place Strata keeps a secret it genuinely must hold,
//! backed by the OS keystore (macOS Keychain, the Windows Credential Manager, the Secret
//! Service elsewhere).
//!
//! The rule this module makes *structural* is: **config stores a reference, never the secret.**
//! [`SecretRef`] is the entire config-side vocabulary, and [`Secret`] has no serde path, no
//! `Display`, and a redacting `Debug` — so a provider key reaching `config.json` is not a mistake to
//! be careful about, it is a program that does not compile.
//!
//! **Why not the config file.** The agent-access bearer token is a plain string there, tolerable
//! because we mint it locally for our own loopback server. A provider key is a billing credential
//! for somebody else's service, so "stored like the token" was the wrong precedent to extend.
//! (Migrating the token onto this store needs a config upgrade path and is not worth one yet.)
//!
//! **Nothing here is async, and every call blocks.** A keystore read is a synchronous platform call
//! that can wait on a lock, a prompt or a daemon, so a caller on the render thread goes through
//! `strata_freya::task::offload` like every other blocking read.
//!
//! **In memory it is zeroed, not guarded.** [`Secret`] zeroes its buffer on drop and
//! [`SecretRef::get`] zeroes the string the keystore handed back — the two copies this module owns.
//! That is not a claim the key is protected in memory, and a guarded allocation could not make one
//! either: a pasted key also exists in the text field's own reallocating `String`, the settings
//! draft, `security-framework`'s buffer, and later the HTTP and TLS write buffers. Guarding one
//! link of six buys a feeling rather than a property, and mlock-style crates want libsodium linked
//! into a bundle that must stay self-contained. What reduces exposure is lifetime: read a key per
//! use, never cache one.
//!
//! **Why `keyring-core` plus the platform stores rather than the `keyring` all-in-one.** `keyring`
//! installs its store from a `LazyLock` inside `Entry::new`, so a process can never observe the
//! install, choose the keychain, or substitute anything. That last one matters most:
//! `keyring_core::mock` is the only way to make a keystore *refuse*, and proving a refusal surfaces
//! as a typed error rather than a silent fallback is the point of the taxonomy below.
//!
//! **Signing.** Keychain access is per code signature, so an item written by a `cargo run` dev
//! binary is not readable by the signed `.app` without a prompt. macOS behaving correctly, not a
//! bug: a dev build and a release build hold separate credentials by design.

use std::fmt;

use keyring_core::{Entry, Error};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroize;

/// The app's identity, and the keystore *service* every Strata credential is filed under.
///
/// This is the macOS bundle identifier as well, and it is written **here and nowhere else**:
/// `scripts/bundle-macos.sh` reads this constant to stamp `CFBundleIdentifier`, so the
/// Keychain items the app writes cannot end up filed under an identity the bundle does not
/// claim. Changing it orphans every credential already stored, exactly as it orphans the
/// quarantine record and the notarization ticket.
pub const APP_ID: &str = "com.alexparlett.strata";

/// The Apple Developer team a release build is signed by.
///
/// It is an *identity*, not a credential — it is readable out of any distributed bundle's
/// signature, and only the private key can sign for it — which is why it belongs in source
/// rather than beside that key in the repo's Actions secrets.
///
/// It is written here because it is the anchor the updater verifies a downloaded bundle
/// against (UP-02): a staged update is installed only if its signature verifies strictly,
/// claims [`APP_ID`], and names *this* team. That is what makes an archive fetched over an
/// untrusted network safe to swap in, and it is the only check there is — a file the app
/// downloads itself is never quarantined, so Gatekeeper never assesses it.
/// `scripts/bundle-macos.sh` reads this constant the way it reads [`APP_ID`] and fails a
/// signed build whose signature names a different team, so the app cannot ship signed by a
/// team its own updater would refuse.
///
/// **Which team.** This is the team of the **Developer ID Application** certificate, which is
/// not the team of the Apple Development certificate sitting in the same keychain: an
/// individual account has a personal team as well as the paid program's, and only the second
/// signs something Apple will notarize. The value is that certificate's
/// `organizationalUnitName`, which is what a distributed bundle carries as its
/// `TeamIdentifier`. Taking it from the wrong certificate is exactly the slip the build's
/// cross-check exists to catch, and did.
pub const TEAM_ID: &str = "397J3SJ3D4";

/// A reference to a secret in the OS keystore: "there is a secret filed under this id", or
/// absent.
///
/// This is what config carries and what a settings draft diffs — it is `Clone + PartialEq +
/// Serialize + Deserialize` for exactly that reason, so it rides `settings_merge!` like any
/// other field. It holds no part of the secret and never has: reading one is a keystore
/// call, which is what keeps the two apart.
///
/// A consumer mints one per thing-that-has-a-key and keeps it for that thing's life, so an
/// edit overwrites in place ([`SecretRef::put`]) rather than stranding the old entry under
/// an id nobody remembers.
///
/// **Or derives one, for a secret whose def is shared** — see [`SecretRef::derived`].
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretRef(Uuid);

/// The namespace every [derived](SecretRef::derived) reference is built in.
///
/// A fixed, arbitrary UUID, the way `new_v5` is meant to be used: it makes the derivation
/// Strata's, so two applications deriving `"pg-password:postgres://…"` do not land on one id.
/// It is not a secret and it is not a version — changing it orphans every derived entry
/// already in a keystore, exactly as changing [`APP_ID`] does.
const STRATA_SECRET_NS: Uuid = Uuid::from_u128(0x5734_7a1a_9c4f_5d2b_8e6a_0f1c_3b7d_9e42);

/// A secret in memory: a pasted key on its way to the keystore, or one just read back.
///
/// Deliberately **not** serializable, not `Display`, and `Debug`-redacted. The type is the
/// enforcement: a draft field holding one cannot be persisted by accident, and a
/// `tracing::debug!("{draft:?}")` cannot print it. It also zeroes its buffer on drop, which
/// is a window narrowed and not a guarantee — see the module docs for why guarding the
/// memory would not be one either.
///
/// Empty is not a secret ([`Secret::new`] returns `None`), which is what makes the settings
/// draft rule fall out of the types: a cleared field yields no `Secret`, and no `Secret` is
/// a [`SecretRef::delete`].
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

/// Why a keystore call did not do what was asked.
///
/// Two variants because the user can act on the difference: an unavailable keystore is
/// something they can unlock or allow, and anything else is something to report. Both carry
/// the platform's own words, and the `Display` is the sentence a surface renders — Settings
/// at Apply, the assistant's client construction as its config-error path. Neither is ever
/// answered by writing the secret somewhere else, which is the failure this whole module
/// exists to prevent.
///
/// **Absence is not in here.** A missing credential is a normal answer ([`SecretRef::get`]
/// returns `Ok(None)`), because a config marker pointing at an entry the user deleted from
/// Keychain Access means "no key set", not "the keystore is broken" — and the two lead to
/// different sentences on screen.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SecretError {
    /// The keystore could not be reached at all: locked, absent, or access refused.
    Unavailable(String),
    /// The keystore answered and the operation still failed.
    Failed(String),
}

/// Open the OS keystore for this process. Call once, at startup, before anything reads a
/// secret.
///
/// Explicit rather than lazy on first use: the process-wide default store is
/// `keyring-core`'s own registry, and a module that installed itself on first touch could
/// never be handed a different store — which is what the failure tests need. A caller that
/// forgets is not silently fine either; every call then answers
/// [`SecretError::Unavailable`].
///
/// On macOS this is the **User** (login) keychain, which is the one that unlocks with the
/// session and syncs with nothing.
pub fn open_keystore() -> Result<(), SecretError> {
    #[cfg(target_os = "macos")]
    let store = apple_native_keyring_store::keychain::Store::new().map_err(classify)?;
    #[cfg(target_os = "windows")]
    let store = windows_native_keyring_store::Store::new().map_err(classify)?;
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    let store = zbus_secret_service_keyring_store::Store::new().map_err(classify)?;
    keyring_core::set_default_store(store);
    Ok(())
}

impl SecretRef {
    /// Mint a reference for a secret that does not have one yet.
    pub fn mint() -> Self {
        Self(Uuid::new_v4())
    }

    /// The reference for a secret belonging to a thing that is **described in a shared file** —
    /// derived from that description rather than minted, so the description carries no
    /// machine-local id.
    ///
    /// `Uuid::new_v5` over `"{kind}:{name}"`: deterministic, so the same def addresses the same
    /// keystore slot on every machine, while each machine's keystore holds its own entry (or
    /// none). `kind` is the family of secret (`"postgres-password"`), `name` the thing's own
    /// identity within it (a data source's name).
    ///
    /// **The contract, which is not optional.** A minted [`SecretRef`] in a *committed* file
    /// would be rewritten by every colleague who entered their own password — two machines
    /// ping-ponging one id through git forever. So a derived ref exists precisely because the
    /// def must never store it: it stores the *expectation* that there is a secret, and the ref
    /// is recomputed from the identity whenever one is
    /// needed. Two consequences ride with it, and whoever moves the identity owes both:
    /// migrate the entry ([`migrate_derived`]), and accept that a machine with no entry is a
    /// normal state that must be reported rather than treated as a fault.
    ///
    /// `mint`'s consumers are untouched: an app-config secret is not shared, so nothing about
    /// the assistant's provider keys changes.
    pub fn derived(kind: &str, name: &str) -> Self {
        Self(Uuid::new_v5(
            &STRATA_SECRET_NS,
            format!("{kind}:{name}").as_bytes(),
        ))
    }

    /// Store `secret` under this reference, replacing whatever was there.
    ///
    /// There is no "put an empty secret": clearing a key is [`SecretRef::delete`], and
    /// [`Secret::new`] is what makes the caller face that fork.
    pub fn put(&self, secret: &Secret) -> Result<(), SecretError> {
        self.entry()?
            .set_password(secret.expose())
            .map_err(classify)
    }

    /// Read the secret filed under this reference. `Ok(None)` means there is none — either
    /// it was never written or it has been removed, which are the same answer to everyone
    /// who asks.
    pub fn get(&self) -> Result<Option<Secret>, SecretError> {
        match self.entry()?.get_password() {
            Ok(mut value) => {
                let secret = Secret::new(&value);
                value.zeroize();
                Ok(secret)
            }
            Err(Error::NoEntry) => Ok(None),
            Err(err) => Err(classify(err)),
        }
    }

    /// Remove the secret filed under this reference. Removing one that is not there
    /// succeeds: a cleared field must not fail to clear because it was already clear.
    pub fn delete(&self) -> Result<(), SecretError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(err) => Err(classify(err)),
        }
    }

    /// The keystore entry for this reference. Builds nothing in the store and reads
    /// nothing from it — the platform call happens in the operation, not here.
    fn entry(&self) -> Result<Entry, SecretError> {
        Entry::new(APP_ID, &self.0.to_string()).map_err(classify)
    }
}

/// Move the secret filed under `old` to `new` — what an edit that changes a
/// [derived](SecretRef::derived) identity owes, since the ref moves with it.
///
/// Get, put, delete, in that order: a failure anywhere leaves the old entry readable, which is
/// the recoverable direction (the user re-enters a password at worst; a delete-first order that
/// failed halfway would lose one). **Best-effort about absence**, because absence is the
/// ordinary case rather than an error — this machine may simply never have had the secret the
/// def expects, and an edit must not fail for that. What it does not swallow is a keystore that
/// *refuses*: that reaches the caller, because it means the migration did not happen and the
/// old entry is still where it was.
///
/// Beside the type rather than in whichever surface needed it first, so an identity move from
/// the data source editor, a future palette gesture or a project merge all clean up the same way.
pub fn migrate_derived(old: &SecretRef, new: &SecretRef) -> Result<(), SecretError> {
    if old == new {
        return Ok(());
    }
    let Some(secret) = old.get()? else {
        return Ok(());
    };
    new.put(&secret)?;
    old.delete()
}

impl Secret {
    /// Wrap a pasted value, trimmed. `None` for anything that is only whitespace: an empty
    /// key is not a key, and letting one through would have every consumer re-check for a
    /// blank string that the keystore happily round-trips.
    pub fn new(value: &str) -> Option<Self> {
        let value = value.trim();
        (!value.is_empty()).then(|| Self(value.to_string()))
    }

    /// The secret itself. Named to be conspicuous at the call site.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Hand-written rather than `#[derive(ZeroizeOnDrop)]`: the derive is the whole reason to
/// pull in a proc macro, and this is one line of it.
impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(why) => write!(f, "The OS keystore is not available: {why}"),
            Self::Failed(why) => write!(f, "The OS keystore could not complete the request: {why}"),
        }
    }
}

impl std::error::Error for SecretError {}

/// Sort a `keyring-core` error into the two things a surface can say about it.
///
/// Three variants are deliberately **not** passed through verbatim. `NoStorageAccess` and
/// `PlatformFailure` are unwrapped to the platform's own sentence, because their own
/// `Display` restates the category ("Couldn't access platform storage: …", "Platform
/// failure: …") that [`SecretError`]'s `Display` has already said — rendered whole they
/// stutter. `Ambiguous` formats the matching credentials with `Debug`, and a store is free to
/// put a stored value in there, so it is reported as a count. (`NoEntry` never arrives here —
/// it is an answer, not a failure, and both callers that can see it handle it first.)
fn classify(err: Error) -> SecretError {
    match err {
        Error::NoDefaultStore => {
            SecretError::Unavailable("no keystore was opened for this process".to_string())
        }
        Error::NoStorageAccess(why) => SecretError::Unavailable(why.to_string()),
        Error::PlatformFailure(why) => SecretError::Failed(why.to_string()),
        Error::Ambiguous(matches) => SecretError::Failed(format!(
            "{} credentials match this reference",
            matches.len()
        )),
        other => SecretError::Failed(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyring_core::mock;
    use std::sync::Once;

    /// One in-memory store for this whole test binary, installed on first use.
    ///
    /// The default store is process-wide, so the tests share it rather than each installing
    /// one and dropping the previous one's credentials on the floor. Every test mints its
    /// own [`SecretRef`], so sharing costs them nothing. The *real* keystore is exercised by
    /// `tests/secret_keystore.rs`, which is a separate binary for this exact reason:
    /// [`open_keystore`] and a mock cannot both be the default store in one process.
    fn mocked() {
        static INSTALLED: Once = Once::new();
        INSTALLED.call_once(|| keyring_core::set_default_store(mock::Store::new().unwrap()));
    }

    #[test]
    fn a_secret_round_trips_and_a_deleted_one_is_absent() {
        mocked();
        let key = SecretRef::mint();
        assert_eq!(key.get(), Ok(None), "nothing is stored under a fresh ref");

        key.put(&Secret::new("sk-test-value").unwrap()).unwrap();
        assert_eq!(
            key.get().unwrap().as_ref().map(Secret::expose),
            Some("sk-test-value")
        );

        key.delete().unwrap();
        assert_eq!(key.get(), Ok(None), "a deleted secret reads as absent");
        key.delete().expect("deleting an absent secret succeeds");
    }

    #[test]
    fn a_replaced_secret_overwrites_in_place() {
        mocked();
        let key = SecretRef::mint();
        key.put(&Secret::new("first").unwrap()).unwrap();
        key.put(&Secret::new("second").unwrap()).unwrap();
        assert_eq!(
            key.get().unwrap().as_ref().map(Secret::expose),
            Some("second")
        );
    }

    /// The failure this module exists for: a keystore that refuses must reach the caller as
    /// an error it can render, never as an absence (which reads as "no key set") and never
    /// as a value from somewhere else.
    #[test]
    fn a_refusing_keystore_is_a_typed_error() {
        mocked();
        let key = SecretRef::mint();
        key.put(&Secret::new("value").unwrap()).unwrap();

        let refuse = |err| {
            let entry = Entry::new(APP_ID, &key.0.to_string()).unwrap();
            let cred: &mock::Cred = entry.as_any().downcast_ref().unwrap();
            cred.set_error(err);
        };

        refuse(Error::NoStorageAccess("the keychain is locked".into()));
        assert_eq!(
            key.get(),
            Err(SecretError::Unavailable(
                "the keychain is locked".to_string()
            ))
        );

        refuse(Error::NotSupportedByStore("read-only store".to_string()));
        assert_eq!(
            key.put(&Secret::new("value").unwrap()),
            Err(SecretError::Failed(
                "Unsupported: read-only store".to_string()
            ))
        );

        refuse(Error::NoStorageAccess("the keychain is locked".into()));
        assert!(matches!(key.delete(), Err(SecretError::Unavailable(_))));
    }

    /// What a caller sees when [`open_keystore`] was never run or failed: an error it can
    /// render, not a panic and not "no key set".
    ///
    /// Asserted at [`classify`] rather than by calling `get` with no store installed, and the
    /// reason is a real hazard rather than convenience: the default store is process-wide and
    /// [`mocked`] installs one for the whole binary, so a test that depended on *no* store
    /// being installed would pass or fail on test ordering. This pins the same mapping
    /// without the race.
    #[test]
    fn no_store_is_classified_as_unavailable() {
        assert_eq!(
            classify(Error::NoDefaultStore),
            SecretError::Unavailable("no keystore was opened for this process".to_string())
        );
    }

    /// The marker is what config carries, so it has to survive the config file and compare
    /// by value for `Settings::merge_onto`. The secret has no such path at all, which is a
    /// property of the types rather than of this test: `Secret` derives no `Serialize`.
    #[test]
    fn the_marker_round_trips_through_serde() {
        let key = SecretRef::mint();
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, format!("\"{}\"", key.0));
        assert_eq!(serde_json::from_str::<SecretRef>(&json).unwrap(), key);
        assert_ne!(key, SecretRef::mint());
    }

    /// **A derived reference is the same reference every time, on every machine** — which is
    /// the whole property, because the def that addresses it is committed and shared and
    /// carries no id of its own.
    ///
    /// Pinned as a *literal*, not merely as "two calls agree": a change to the namespace or to
    /// the `"{kind}:{name}"` spelling would still pass a self-consistency check while orphaning
    /// every password already in a keystore.
    #[test]
    fn a_derived_reference_is_stable_across_calls_and_machines() {
        let url = "postgres://reader@db.internal:5432/analytics";
        let key = SecretRef::derived("pg-password", url);
        assert_eq!(key, SecretRef::derived("pg-password", url));
        assert_eq!(
            key.0.to_string(),
            "c9251ca2-1ae3-5c70-9612-712916f594c3",
            "the derivation is what makes one def address one slot everywhere"
        );

        assert_ne!(
            key,
            SecretRef::derived(
                "pg-password",
                "postgres://writer@db.internal:5432/analytics"
            )
        );
        assert_ne!(key, SecretRef::derived("other-kind", url));
        assert_ne!(key, SecretRef::mint());
    }

    /// An identity move carries the secret with it: the new ref reads what the old one held,
    /// and the old entry is gone. Absence is not a failure — a machine that never held this
    /// data source's password must still be able to edit the source.
    #[test]
    fn migrating_a_derived_secret_moves_the_entry_and_tolerates_absence() {
        mocked();
        let old = SecretRef::derived("pg-password", "postgres://reader@a:5432/x");
        let new = SecretRef::derived("pg-password", "postgres://reader@b:5432/x");
        old.put(&Secret::new("hunter2").unwrap()).unwrap();

        migrate_derived(&old, &new).expect("migrates");
        assert_eq!(
            new.get().unwrap().as_ref().map(Secret::expose),
            Some("hunter2")
        );
        assert_eq!(
            old.get(),
            Ok(None),
            "the old slot is not left holding a copy"
        );

        let absent = SecretRef::derived("pg-password", "postgres://reader@c:5432/x");
        migrate_derived(&absent, &new).expect("nothing to move");
        assert_eq!(
            new.get().unwrap().as_ref().map(Secret::expose),
            Some("hunter2"),
            "and it did not overwrite the destination with nothing"
        );
        migrate_derived(&new, &new).expect("a no-op move");
        assert_eq!(
            new.get().unwrap().as_ref().map(Secret::expose),
            Some("hunter2")
        );
    }

    #[test]
    fn an_empty_secret_is_not_a_secret() {
        assert_eq!(Secret::new("  sk-padded  ").unwrap().expose(), "sk-padded");
        assert!(Secret::new("").is_none());
        assert!(Secret::new("   ").is_none());
    }

    #[test]
    fn a_secret_does_not_print_itself() {
        let secret = Secret::new("sk-test-value").unwrap();
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
    }
}
