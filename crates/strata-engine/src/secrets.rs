//! Secret storage.
//!
//! Opening a connection that expects a secret requires reading it from somewhere.
//! [`KeystoreSecrets`] reads the operating system keystore, [`EnvSecrets`] reads the process
//! environment and [`MemSecrets`] holds values in memory; [`ChainSecrets`] asks several in turn.
//! Any other source is a [`SecretProvider`] implementation, set with
//! [`EngineBuilder::with_secrets`](crate::EngineBuilder::with_secrets).
//!
//! Secrets are read once per use and never cached.

use std::collections::HashMap;
use std::sync::Arc;
use std::{env, fmt};

use strata_core::secret::{Secret, SecretRef};

/// What one secret is wanted for.
///
/// A request rather than a bare key, because where a secret may be found is the asking source's
/// vocabulary: the keystore slot is derived from the family and the connection, and a source that
/// has a conventional environment variable states its own
/// ([`env`](Self::env) — `PGPASSWORD`, `MYSQL_PWD`). A provider answers from whichever of them it
/// reads and ignores the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretRequest {
    /// Which slot this is: `"{kind}-{key}"`, one per secret-typed key a source declares.
    pub family: String,
    /// The connection the secret belongs to, by its own name — which is what a fix asks the
    /// user to open, and what moving the name has to move the entry to.
    pub connection: String,
    /// The environment variables this source conventionally reads, in the order it reads them.
    pub env: &'static [&'static str],
}

impl SecretRequest {
    /// The keystore slot this request addresses.
    ///
    /// Derived from the family and the connection rather than stored, so the committed
    /// `project.json` carries no machine-local id and each machine's keystore holds its own entry.
    pub fn key(&self) -> SecretRef {
        SecretRef::derived(&self.family, &self.connection)
    }

    /// Both places this secret could have come from, for the sentence a miss produces.
    pub fn fixes(&self) -> String {
        match self.env {
            [] => format!("Open the connection '{}' and enter it.", self.connection),
            [one] => format!(
                "Open the connection '{}' and enter it, or set {one}.",
                self.connection
            ),
            many => format!(
                "Open the connection '{}' and enter it, or set one of {}.",
                self.connection,
                many.join(", ")
            ),
        }
    }
}

/// Provides secrets such as database passwords.
///
/// Implementations are called from a blocking context and may wait on the operating system or on
/// the user.
pub trait SecretProvider: Send + Sync {
    /// Return the secret `request` asks for, or `None` if there is none
    ///
    /// A request nothing was stored for is `Ok(None)` rather than an error.
    ///
    /// # Errors
    ///
    /// Returns the reason the store could not be read, in words suitable for display.
    fn secret(&self, request: &SecretRequest) -> Result<Option<Secret>, String>;
}

impl<T: SecretProvider + ?Sized> SecretProvider for Arc<T> {
    fn secret(&self, request: &SecretRequest) -> Result<Option<Secret>, String> {
        (**self).secret(request)
    }
}

impl<T: SecretProvider + ?Sized> SecretProvider for Box<T> {
    fn secret(&self, request: &SecretRequest) -> Result<Option<Secret>, String> {
        (**self).secret(request)
    }
}

/// Reads secrets from the operating system keystore.
///
/// Requires [`open_keystore`](strata_core::secret::open_keystore) to have been called; until it
/// has, every read fails.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeystoreSecrets;

impl SecretProvider for KeystoreSecrets {
    fn secret(&self, request: &SecretRequest) -> Result<Option<Secret>, String> {
        request.key().get().map_err(|e| e.to_string())
    }
}

/// Reads secrets from the process environment, under the names the asking source conventionally
/// uses ([`SecretRequest::env`]).
///
/// Ambient credential the user set, never something Strata stores: the no-plaintext rule governs
/// what is *persisted*, and this persists nothing. An empty variable is no answer, so a set-but-
/// blank var does not shadow a keystore entry behind it.
#[derive(Clone, Copy, Debug, Default)]
pub struct EnvSecrets;

impl SecretProvider for EnvSecrets {
    fn secret(&self, request: &SecretRequest) -> Result<Option<Secret>, String> {
        Ok(request
            .env
            .iter()
            .find_map(|name| env::var(name).ok())
            .and_then(|value| Secret::new(&value)))
    }
}

/// Asks each provider in turn and takes the first answer.
///
/// Order is precedence: the app asks the keystore before the environment, so what a user entered
/// beats what happens to be set in the process; a headless tool may ask the environment alone. A
/// provider that *faults* stops the chain, because "the keystore is locked" is a fact the user
/// needs rather than a reason to quietly use something else.
///
/// # Example
///
/// ```
/// # use strata_engine::secrets::{ChainSecrets, EnvSecrets, KeystoreSecrets};
/// let secrets = ChainSecrets::new().then(KeystoreSecrets).then(EnvSecrets);
/// ```
#[derive(Default)]
pub struct ChainSecrets(Vec<Arc<dyn SecretProvider>>);

impl ChainSecrets {
    /// Creates an empty chain, which answers `None` to everything.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `provider` after the ones already added.
    #[must_use]
    pub fn then(mut self, provider: impl SecretProvider + 'static) -> Self {
        self.0.push(Arc::new(provider));
        self
    }
}

impl fmt::Debug for ChainSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChainSecrets")
            .field("providers", &self.0.len())
            .finish()
    }
}

impl SecretProvider for ChainSecrets {
    fn secret(&self, request: &SecretRequest) -> Result<Option<Secret>, String> {
        for provider in &self.0 {
            if let Some(secret) = provider.secret(request)? {
                return Ok(Some(secret));
            }
        }
        Ok(None)
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
/// let key = SecretRef::derived("postgres-password", "postgres://localhost:5432/orders");
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
    fn secret(&self, request: &SecretRequest) -> Result<Option<Secret>, String> {
        Ok(self.0.get(&request.key()).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider that cannot look, ever — a locked keystore or an unreachable secrets manager,
    /// on a machine where neither can be arranged.
    struct Broken;

    impl SecretProvider for Broken {
        fn secret(&self, _request: &SecretRequest) -> Result<Option<Secret>, String> {
            Err("the store is locked".into())
        }
    }

    fn request() -> SecretRequest {
        SecretRequest {
            family: "postgres-password".into(),
            connection: "orders".into(),
            env: &["STRATA_TEST_PGPASSWORD"],
        }
    }

    /// The clauses every [`SecretProvider`] keeps, whatever it reads from. The request is one the
    /// provider may or may not answer — the contract is about the shape of the answer, not about
    /// which arm a given provider is in.
    fn conforms(provider: &dyn SecretProvider, request: &SecretRequest) {
        let first = provider.secret(request);
        let second = provider.secret(request);
        assert_eq!(
            first.is_ok(),
            second.is_ok(),
            "two reads of one request must agree"
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
        let ask = request();
        conforms(&KeystoreSecrets, &ask);
        conforms(&EnvSecrets, &ask);
        conforms(&MemSecrets::new(), &ask);
        conforms(
            &MemSecrets::new().with(ask.key(), Secret::new("hunter2").unwrap()),
            &ask,
        );
        conforms(&Broken, &ask);
        conforms(
            &ChainSecrets::new().then(MemSecrets::new()).then(EnvSecrets),
            &ask,
        );
    }

    #[test]
    fn absence_is_ok_none_and_a_fault_is_err() {
        assert_eq!(MemSecrets::new().secret(&request()), Ok(None));
        assert!(Broken.secret(&request()).is_err());
    }

    #[test]
    fn a_filed_secret_comes_back_under_its_own_request_and_no_other() {
        let ask = request();
        let other = SecretRequest {
            connection: "events".into(),
            ..request()
        };
        let held = MemSecrets::new().with(ask.key(), Secret::new("hunter2").unwrap());
        assert_eq!(
            held.secret(&ask).unwrap().map(|s| s.expose().to_string()),
            Some("hunter2".to_string())
        );
        assert_eq!(held.secret(&other), Ok(None));
    }

    /// **The family is per key, so one connection's two secrets are two slots.** A source that
    /// declares an access key and a secret key files them separately, and neither answers for the
    /// other.
    #[test]
    fn two_keys_of_one_connection_are_two_slots() {
        let id = SecretRequest {
            family: "s3-access_key_id".into(),
            connection: "acme-lake".into(),
            env: &[],
        };
        let secret = SecretRequest {
            family: "s3-secret_access_key".into(),
            ..id.clone()
        };
        assert_ne!(id.key(), secret.key());
        let held = MemSecrets::new().with(id.key(), Secret::new("AKIA").unwrap());
        assert_eq!(held.secret(&secret), Ok(None));
    }

    /// A handle an embedder already shares reaches the seam without the engine asking for an
    /// `Arc` in its own signatures.
    #[test]
    fn a_shared_handle_is_a_provider_too() {
        let ask = request();
        let shared = Arc::new(MemSecrets::new().with(ask.key(), Secret::new("hunter2").unwrap()));
        conforms(&shared, &ask);
        let boxed: Box<dyn SecretProvider> = Box::new(MemSecrets::new());
        conforms(&boxed, &ask);
    }

    /// The chain takes the first **answer**, and a fault stops it: "the keystore is locked" is a
    /// fact the user needs, not a reason to quietly use something else.
    #[test]
    fn a_chain_takes_the_first_answer_and_stops_at_a_fault() {
        let ask = request();
        let held = MemSecrets::new().with(ask.key(), Secret::new("first").unwrap());
        let second = MemSecrets::new().with(ask.key(), Secret::new("second").unwrap());
        let chain = ChainSecrets::new().then(held).then(second);
        assert_eq!(
            chain.secret(&ask).unwrap().map(|s| s.expose().to_string()),
            Some("first".to_string())
        );
        assert_eq!(ChainSecrets::new().secret(&ask), Ok(None));
        assert!(ChainSecrets::new()
            .then(Broken)
            .then(MemSecrets::new().with(ask.key(), Secret::new("unreached").unwrap()))
            .secret(&ask)
            .is_err());
    }

    /// A miss names **both** places the secret could have come from, because a colleague pulling
    /// the project has neither.
    #[test]
    fn a_miss_names_the_keystore_and_the_variable() {
        let fixes = request().fixes();
        assert!(fixes.contains("'orders'"), "{fixes}");
        assert!(fixes.contains("STRATA_TEST_PGPASSWORD"), "{fixes}");
        let bare = SecretRequest {
            env: &[],
            ..request()
        };
        assert!(!bare.fixes().contains(" or set "), "{}", bare.fixes());
    }

    /// An environment variable set to nothing is not an answer, so it cannot shadow a keystore
    /// entry sitting behind it in the chain.
    #[test]
    fn a_blank_variable_is_no_answer() {
        let ask = SecretRequest {
            env: &["STRATA_TEST_BLANK_SECRET"],
            ..request()
        };
        // SAFETY: single-threaded test, and the variable is this test's own name.
        unsafe { env::set_var("STRATA_TEST_BLANK_SECRET", "") };
        assert_eq!(EnvSecrets.secret(&ask), Ok(None));
        unsafe { env::remove_var("STRATA_TEST_BLANK_SECRET") };
    }
}
