//! The clauses every [`SecretProvider`](super::SecretProvider) keeps, as a body any
//! implementation can be run through.
//!
//! It is about the **shape of the answer**, not about which arm a given provider is in: a store
//! that holds the secret, one that does not, and one that cannot look at all all keep this
//! contract. What it pins is that two reads agree, that a read does not consume what it read, and
//! that a fault says what went wrong in the same words twice.
//!
//! Available to embedders under the `testing` cargo feature.

use super::{SecretProvider, SecretRequest};

/// Runs `provider` through the contract, panicking on the first clause it does not keep.
///
/// `request` is one the provider may or may not answer — either keeps the contract.
///
/// # Examples
///
/// ```
/// use strata_engine::secrets::{MemSecrets, SecretRequest};
/// use strata_model::SecretRef;
///
/// let ask = SecretRequest {
///     family: "postgres-password".into(),
///     source: "orders".into(),
///     slot: SecretRef::mint(),
///     env: &["STRATA_PGPASSWORD"],
/// };
/// strata_engine::testing::secrets::conforms(&MemSecrets::new(), &ask);
/// ```
///
/// # Panics
///
/// On any clause the provider does not keep.
pub fn conforms(provider: &dyn SecretProvider, request: &SecretRequest) {
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
