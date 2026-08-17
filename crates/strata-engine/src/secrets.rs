//! Secret storage.
//!
//! Opening a connection that expects a password requires reading it from somewhere.
//! [`KeystoreSecrets`] reads the operating system keystore and [`MemSecrets`] holds values in
//! memory; any other source is a [`SecretProvider`] implementation, set with
//! [`EngineBuilder::with_secrets`](crate::EngineBuilder::with_secrets).
//!
//! Secrets are read once per use and never cached.

use std::collections::HashMap;
use std::sync::Arc;

use strata_core::secret::{Secret, SecretRef};

/// Provides secrets such as database passwords.
///
/// Implementations are called from a blocking context and may wait on the operating system or on
/// the user.
pub trait SecretProvider: Send + Sync {
    /// Return the secret stored under `key`, or `None` if there is none
    ///
    /// A key nothing was stored under is `Ok(None)` rather than an error.
    ///
    /// # Errors
    ///
    /// Returns the reason the store could not be read, in words suitable for display.
    fn secret(&self, key: &SecretRef) -> Result<Option<Secret>, String>;
}

impl<T: SecretProvider + ?Sized> SecretProvider for Arc<T> {
    fn secret(&self, key: &SecretRef) -> Result<Option<Secret>, String> {
        (**self).secret(key)
    }
}

impl<T: SecretProvider + ?Sized> SecretProvider for Box<T> {
    fn secret(&self, key: &SecretRef) -> Result<Option<Secret>, String> {
        (**self).secret(key)
    }
}

/// Reads secrets from the operating system keystore.
///
/// Requires [`open_keystore`](strata_core::secret::open_keystore) to have been called; until it
/// has, every read fails.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeystoreSecrets;

impl SecretProvider for KeystoreSecrets {
    fn secret(&self, key: &SecretRef) -> Result<Option<Secret>, String> {
        key.get().map_err(|e| e.to_string())
    }
}

/// Holds secrets in memory.
///
/// For an embedder that already has the values, such as a command-line tool reading them from the
/// environment. Nothing is persisted.
///
/// # Example
///
/// ```
/// # use strata_core::secret::{Secret, SecretRef};
/// # use strata_engine::secrets::MemSecrets;
/// let key = SecretRef::derived("pg-password", "postgres://localhost/orders");
/// let secrets = MemSecrets::new().with(key, Secret::new("hunter2").unwrap());
/// ```
#[derive(Clone, Debug, Default)]
pub struct MemSecrets(HashMap<SecretRef, Secret>);

impl MemSecrets {
    /// Creates an empty set
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores `secret` under `key`, replacing any previous value
    pub fn with(mut self, key: SecretRef, secret: Secret) -> Self {
        self.0.insert(key, secret);
        self
    }
}

impl SecretProvider for MemSecrets {
    fn secret(&self, key: &SecretRef) -> Result<Option<Secret>, String> {
        Ok(self.0.get(key).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider that cannot look, ever — a locked keystore or an unreachable secrets manager,
    /// on a machine where neither can be arranged.
    struct Broken;

    impl SecretProvider for Broken {
        fn secret(&self, _key: &SecretRef) -> Result<Option<Secret>, String> {
            Err("the store is locked".into())
        }
    }

    /// The clauses every [`SecretProvider`] keeps, whatever it reads from. `key` is one the
    /// provider may or may not hold — the contract is about the shape of the answer, not about
    /// which arm a given provider is in.
    fn conforms(provider: &dyn SecretProvider, key: &SecretRef) {
        let first = provider.secret(key);
        let second = provider.secret(key);
        assert_eq!(
            first.is_ok(),
            second.is_ok(),
            "two reads of one key must agree"
        );
        match (first, second) {
            (Ok(a), Ok(b)) => assert!(
                a.map(|s| s.expose().to_string()) == b.map(|s| s.expose().to_string()),
                "a read must not consume what it read"
            ),
            (Err(a), Err(b)) => {
                assert!(!a.trim().is_empty(), "a fault must say what went wrong");
                assert_eq!(a, b, "a fault must not change its wording between reads");
            }
            _ => unreachable!("the ok-ness was asserted equal above"),
        }
    }

    #[test]
    fn every_provider_answers_the_same_way_twice() {
        let key = SecretRef::derived("pg-password", "postgres://acme/orders");
        conforms(&KeystoreSecrets, &key);
        conforms(&MemSecrets::new(), &key);
        conforms(
            &MemSecrets::new().with(key.clone(), Secret::new("hunter2").unwrap()),
            &key,
        );
        conforms(&Broken, &key);
    }

    #[test]
    fn absence_is_ok_none_and_a_fault_is_err() {
        let key = SecretRef::derived("pg-password", "postgres://acme/orders");
        assert_eq!(MemSecrets::new().secret(&key), Ok(None));
        assert!(Broken.secret(&key).is_err());
    }

    #[test]
    fn a_filed_secret_comes_back_under_its_own_key_and_no_other() {
        let key = SecretRef::derived("pg-password", "postgres://acme/orders");
        let other = SecretRef::derived("pg-password", "postgres://acme/events");
        let held = MemSecrets::new().with(key.clone(), Secret::new("hunter2").unwrap());
        assert_eq!(
            held.secret(&key).unwrap().map(|s| s.expose().to_string()),
            Some("hunter2".to_string())
        );
        assert_eq!(held.secret(&other), Ok(None));
    }

    /// A handle an embedder already shares reaches the seam without the engine asking for an
    /// `Arc` in its own signatures.
    #[test]
    fn a_shared_handle_is_a_provider_too() {
        let key = SecretRef::derived("pg-password", "postgres://acme/orders");
        let shared = Arc::new(MemSecrets::new().with(key.clone(), Secret::new("hunter2").unwrap()));
        conforms(&shared, &key);
        let boxed: Box<dyn SecretProvider> = Box::new(MemSecrets::new());
        conforms(&boxed, &key);
    }
}
