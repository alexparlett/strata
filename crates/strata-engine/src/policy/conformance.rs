//! The clauses every [`PolicyProvider`] keeps, as a body any implementation can be run through.
//!
//! It is about the **shape of the answer**, not about which arm a given provider is in: a
//! provider that denies everything and one that allows everything both keep this contract. What
//! it pins is that a provider decides the same way twice, that a fault says what went wrong, and
//! that the fine phase never widens what the coarse phase denied — which is the fail-closed half,
//! and the one a third-party provider has to prove.
//!
//! Available to embedders under the `testing` cargo feature.

use futures::executor::block_on;

use super::{Admit, GrantFamily, PolicyProvider, Principal, TargetFacts};

/// Runs `provider` through the contract, panicking on the first clause it does not keep.
///
/// `who` is a caller the provider may or may not allow — either answer keeps the contract.
///
/// # Examples
///
/// ```
/// use strata_engine::{Capability, CapabilityPolicyProvider, Principal};
///
/// strata_engine::testing::policy::conforms(
///     &CapabilityPolicyProvider::new(Capability::read_only()),
///     &Principal::new(Capability::full()),
/// );
/// ```
///
/// # Panics
///
/// On any clause the provider does not keep.
pub fn conforms(provider: &dyn PolicyProvider, who: &Principal) {
    let targets = [
        TargetFacts::workspace(),
        TargetFacts::remote("postgres", "postgres://acme/orders"),
    ];
    for family in GrantFamily::ALL {
        let coarse = block_on(provider.admit(who, family));
        assert_eq!(
            coarse,
            block_on(provider.admit(who, family)),
            "two asks about {family:?} must agree"
        );
        if let Err(why) = &coarse {
            assert!(!why.trim().is_empty(), "a fault must say what went wrong");
        }
        for target in &targets {
            let fine = block_on(provider.permit(who, family, target));
            assert_eq!(
                fine,
                block_on(provider.permit(who, family, target)),
                "two asks about {family:?} at {target:?} must agree"
            );
            if matches!(coarse, Ok(Admit::Deny(_))) {
                assert!(
                    !matches!(fine, Ok(Admit::Allow)),
                    "{family:?} is denied coarsely, so the arm must not permit it at {target:?}"
                );
            }
        }
    }
}
