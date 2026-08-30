//! **Where a secret is filed**, as data.
//!
//! The id and nothing else: minting one is arithmetic, and every operation on the keystore it
//! addresses lives in `strata_core::secret`, which is where the platform is. That split is what
//! lets a def carry the slot its secret is in — a def is this crate's, and this crate reaches no
//! keystore.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A reference to a secret in the OS keystore: "there is a secret filed under this id", or
/// absent.
///
/// This is what config carries, what a settings draft diffs, and — since the slot became a
/// **stored** fact rather than an inferred one — what a [`SourceDef`](crate::SourceDef) records
/// per secret it was saved with. It is `Clone + PartialEq + Serialize + Deserialize` for exactly
/// those reasons. It holds no part of the secret and never has: reading one is a keystore call,
/// which is what keeps the two apart.
///
/// A consumer mints one per thing-that-has-a-key and keeps it for that thing's life, so an edit
/// overwrites in place rather than stranding the old entry under an id nobody remembers.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretRef(Uuid);

/// The namespace every [derived](SecretRef::derived) reference is built in.
///
/// A fixed, arbitrary UUID, the way `new_v5` is meant to be used: it makes the derivation
/// Strata's, so two applications deriving `"pg-password:pg"` do not land on one id. It is not a
/// secret and it is not a version — changing it orphans every derived entry already in a
/// keystore.
const STRATA_SECRET_NS: Uuid = Uuid::from_u128(0x5734_7a1a_9c4f_5d2b_8e6a_0f1c_3b7d_9e42);

impl SecretRef {
    /// Mint a reference for a secret that does not have one yet.
    pub fn mint() -> Self {
        Self(Uuid::new_v4())
    }

    /// The reference a **pre-ref** def's secret was filed under, derived from that def's own
    /// identity: `Uuid::new_v5` over `"{kind}:{name}"`.
    ///
    /// **Read on load and never written again.** Deriving the slot from the def meant deriving it
    /// from things the user can change, and the entry lives somewhere no migration can reach:
    /// renaming a data source moves the slot on *every* machine, while only the machine doing the
    /// renaming can move its own keystore entry to follow. A colleague pulling the rename was
    /// left with a stranded password and a form that could not tell that from never having had
    /// one. A minted ref, recorded in the def, is the same id everywhere and survives both a
    /// rename and a change of kind.
    ///
    /// It does not reintroduce the problem derivation was for. The objection to storing one was a
    /// *minted* ref being rewritten by every colleague who entered their own password — two
    /// machines ping-ponging one id through git. A ref written once, when the secret is first
    /// filed, is never rewritten: a colleague entering their own password writes their own
    /// keystore under the id already in the file.
    pub fn derived(kind: &str, name: &str) -> Self {
        Self(Uuid::new_v5(
            &STRATA_SECRET_NS,
            format!("{kind}:{name}").as_bytes(),
        ))
    }

    /// The id this reference addresses, for the keystore entry built from it.
    pub fn slot(&self) -> Uuid {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A derived reference is the same on every machine, and a minted one never is.**
    #[test]
    fn a_derived_reference_is_a_function_of_the_identity_it_names() {
        assert_eq!(
            SecretRef::derived("postgres-password", "pg"),
            SecretRef::derived("postgres-password", "pg")
        );
        assert_ne!(
            SecretRef::derived("postgres-password", "pg"),
            SecretRef::derived("postgres-password", "warehouse"),
            "which is the whole reason a rename stranded one"
        );
        assert_ne!(SecretRef::mint(), SecretRef::mint());
    }

    /// **A reference round-trips as a bare string**, so a def carrying one reads as an id rather
    /// than as a struct.
    #[test]
    fn a_reference_is_transparent_on_the_wire() {
        let key = SecretRef::mint();
        let json = serde_json::to_string(&key).expect("serialize");
        assert!(json.starts_with('"'), "{json}");
        assert_eq!(
            serde_json::from_str::<SecretRef>(&json).expect("parse"),
            key
        );
    }
}
