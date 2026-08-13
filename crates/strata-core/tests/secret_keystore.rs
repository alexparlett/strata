//! The secret store against the **real** OS keystore (AS-05).
//!
//! A separate binary from `secret`'s unit tests on purpose: `keyring-core`'s default store
//! is process-wide, so the in-memory mock those tests install and the platform store
//! [`open_keystore`] opens cannot both exist in one process. The unit tests own the failure taxonomy
//! (a keystore that refuses, one that was never opened); this owns the one thing a mock can
//! never answer — that the platform call actually works.
//!
//! Deliberately **not** `#[ignore]`d, for the reason the MinIO test is not: an ignored test
//! is one nobody runs, and "there is no keystore here" must fail rather than look like "the
//! code is fine". There is no runtime to install for this one — a Mac has a login keychain
//! or it has a real problem.
//!
//! **What it cannot cover, and why.** macOS grants keychain access per code signature, so
//! reading an item back in a *different* binary prompts the user — which in a test run means
//! a dialog nobody is there to answer. So this reads only what it just wrote, in the process
//! that wrote it, and every run mints a fresh reference so it can never meet an item another
//! build left behind. Persistence across a restart, and the signed `.app` reading its own
//! items, are manual checks recorded in
//! `.claude/tasks/workstream-assistant/AS-05-secret-store.md`.

use strata_core::secret::{open_keystore, Secret, SecretRef};

/// Removes the credential however the test ends, so a failed assertion does not leave an
/// item in the developer's login keychain for good.
struct Cleanup(SecretRef);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.0.delete();
    }
}

#[test]
fn a_secret_round_trips_through_the_os_keystore() {
    open_keystore().expect("the OS keystore could not be opened");

    let key = SecretRef::mint();
    let cleanup = Cleanup(key.clone());

    assert!(
        key.get().unwrap().is_none(),
        "a freshly minted reference names nothing"
    );

    key.put(&Secret::new("strata-as-05-first").unwrap())
        .unwrap();
    assert_eq!(
        key.get().unwrap().as_ref().map(Secret::expose),
        Some("strata-as-05-first")
    );

    key.put(&Secret::new("strata-as-05-second").unwrap())
        .unwrap();
    assert_eq!(
        key.get().unwrap().as_ref().map(Secret::expose),
        Some("strata-as-05-second")
    );

    cleanup.0.delete().unwrap();
    assert!(
        key.get().unwrap().is_none(),
        "a deleted secret reads as absent, not as an error"
    );
}
