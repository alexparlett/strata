# AS-05 · The secret store

**Workstream:** Assistant · **Status:** ✅ (one acceptance item deferred to AS-03 — see
*What is not proved yet*) · **Depends on:** — (pure mechanism; AS-03 consumes it. Can land
first or in parallel with AS-01/02.)

## Goal

One shared mechanism for secrets the app must keep: OS-keystore-backed storage (macOS
Keychain first), with **config storing a reference, never the secret**. This extends the
connections posture — "no arm of `engine::store` takes a secret" (AGENTS.md §2) — to the one
place the app now genuinely must hold one: third-party API keys for the assistant's provider
roster (AS-03). This is the shared implementation under §5 cross-task ownership: one
mechanism, owned here; consumers call it, they never grow their own.

## As built

`crates/strata-core/src/secret.rs` — `APP_ID`, `SecretRef`, `Secret`, `SecretError`,
`open_keystore`. Unit tests against `keyring_core::mock`; `crates/strata-core/tests/
secret_keystore.rs` against the real keystore. Opened once in `strata-freya`'s `main`. The
rule form is AGENTS.md §2 + `docs/reference/INVARIANTS.md`.

Five things the plan below did not spell out, settled while building:

- **`keyring-core` plus a per-target store crate, not the all-in-one `keyring`.** The
  candidate was checked against its pinned source (4.1.6 → `keyring-core` 1.0.0,
  `apple-native-keyring-store` 1.0.2; MSRV 1.88, ours is 1.97). `keyring`'s `v1` module is
  the *same three lines* of platform selection as `open_keystore`, but it installs its store
  from a `LazyLock` inside `Entry::new`, so nothing can be substituted for it and no caller
  can choose the keychain. `keyring_core::mock` is the only way to make a keystore **refuse**
  — and it is compiled in unconditionally, no feature — so proving the refusal path meant
  linking the core plus a store. That is the ecosystem's own documented shape for a client
  that wants control, not a workaround. Cost: `open_keystore`'s three `cfg` arms, and a
  platform with no arm is a **build** error rather than a runtime surprise.
- **The error taxonomy is two variants, and absence is not one of them.** `keyring-core`'s
  `Error` is `#[non_exhaustive]` with eleven variants; `secret::classify` folds them into
  `Unavailable` (the store could not be reached: `NoDefaultStore`, `NoStorageAccess`, which
  is what the macOS store maps `errSecNotAvailable` / `errSecNoSuchKeychain` / a locked
  keychain to) and `Failed` (everything else). `NoEntry` never reaches it: `get` answers
  `Ok(None)` and `delete` answers `Ok(())`, because "no key set" and "the keystore is broken"
  are different sentences on screen and a marker pointing at an item the user removed in
  Keychain Access is the first one. `Ambiguous` is the one variant not passed through
  verbatim — it formats the matching credentials with `Debug`, and a store is free to put a
  stored value in there, so it is reported as a count.
- **Empty is not a secret, which is what makes the draft rule structural.** `Secret::new`
  trims and returns `None` for a blank field, so a cleared Settings field *cannot* produce a
  `Secret` to put — the caller's only move is `delete`. The draft rule below is now a
  consequence of the types rather than a convention AS-03 has to remember.
- **The bundle id moved into the Rust constant.** `scripts/bundle-macos.sh` reads `APP_ID`
  out of `secret.rs` (a `sed`, failing hard on an empty result, in the same shape the script
  already reads the version through `version.sh`) and the `STRATA_BUNDLE_ID` environment
  override is **gone** — it was referenced nowhere, and it was the one way the identity the
  bundle claims and the identity the app files credentials under could drift apart.
- **`uuid` became a real dependency of `strata-core`** (it was a dev-dependency), for the
  reference id. Already in the graph via `strata-model`.

### In memory: zeroed on drop, not guarded (settled 2026-08-09 — do not re-litigate)

Asked during review: should the value be protected while it is in memory, `secrets`-style
(mlock, mprotect-noaccess between reads, guard pages)? **No — but it is zeroed.** `Secret`
zeroes its buffer on drop and `get` zeroes the `String` the keystore returned as soon as it
has been wrapped (`zeroize`, pure Rust, no build script). That narrows the window a freed
allocation stays readable, and it is described as exactly that and nothing more.

Guarding was rejected on three grounds, in order of weight:

1. **It would guard one link of six.** A pasted key exists in the text field's own `String`
   (which reallocates as it grows, leaving prefixes in freed heap), in the settings draft, in
   `security-framework`'s buffer on the way to `securityd`, and later in the HTTP header and
   TLS write buffers of whatever sends it. None of those layers is ours to guard, so an
   mprotect'd `Secret` would buy a feeling rather than a property — and a security measure
   that reads as stronger than it is, is worse than none.
2. **The threats it addresses are already handled on our platform.** mlock/mprotect defend
   against swap files, core dumps and cross-process reads. macOS encrypts swap, writes no
   core file by default, and refuses `task_for_pid` to anything without root or a debugger
   entitlement — and an attacker holding that can drive the Keychain as us regardless.
3. **`secrets` 1.3.0 links libsodium.** Its `build.rs` probes pkg-config and otherwise falls
   back to `cargo:rustc-link-lib=dylib=sodium`, so it needs a (universal) C library on every
   build machine and on CI's bare `macos-latest` — against the self-contained bundle rule
   (AGENTS.md §7). Its init also sets `RLIMIT_CORE` to 0 for the whole process in release
   builds, which is not a side effect to inherit from a library.

**What actually reduces exposure here is lifetime**, and that is a design rule rather than a
crate: read a key per use rather than caching one (AS-02 already resolves the reference per
send), and never let one reach a buffer that outlives the call. Reopen this only with a
change that closes the *chain* — a secret-aware text input and a client that takes a guarded
buffer — not with a better allocator for one link of it.

### The keystore's own facts, from the pinned source

- The macOS store is the **User (login)** keychain by default; `Entry::new` builds nothing
  and reads nothing, so it can neither block nor prompt — the platform call is in the
  operation.
- `errSecItemNotFound` → `NoEntry`; `errSecNotAvailable`, `errSecNoSuchKeychain`,
  `errSecReadOnly`, `errSecInvalidKeychain`, a write-permissions error → `NoStorageAccess`;
  anything else → `PlatformFailure`.
- Neither the service nor the account may be empty (empty attributes are wildcards in
  Keychain Services). `APP_ID` and a UUID are both non-empty by construction.
- `apple-native-keyring-store`'s `protected` feature is the sandboxed data-protection store
  and wants a provisioning profile we do not ship; `keychain` (the file-based legacy store)
  is the right one for an unsandboxed app, and it is what the `keychain` feature selects.

## Why not the config file

The AA-03 bearer token lives in app config as a plain string
(`AgentAccess::token`, `strata-core/src/config.rs`) — tolerable because it is locally minted
by us, for our own local server, and worthless anywhere else. Provider API keys are billing
credentials for third-party services; a plaintext profile file is the wrong home for those,
and "stored like the token" (this task's own earlier precedent in AS-03) was the wrong
precedent to extend.

## Shape

- **A `strata-core` module beside config** (e.g. `strata_core::secret`). The API is
  synchronous (`put` / `get` / `delete`) and blocking — callers off the render thread go
  through `task::offload` like every blocking read (AGENTS.md §2, "nothing blocking runs on
  the render thread").
- **The config-side vocabulary is a serde marker type** (e.g. `SecretRef`): "a secret is
  stored under this id" or absent. It is what `settings_merge!` carries and what the config
  file serializes — the secret itself never has a serde path at all, so leaking it through
  config is unrepresentable, not merely avoided.
- **Store identity**: `(service, account)` — service is one constant derived from the app
  identifier (align with the bundle id `scripts/bundle-macos.sh` stamps; one constant, not a
  string in two places), account is the reference id.
- **Crate: `keyring`** (macOS Keychain / Windows Credential Manager / Secret Service) —
  the candidate, not yet the decision: **verify from its pinned source** (platform feature
  flags, MSRV, what a locked or absent keystore returns) before building on it, exactly the
  workstream's standing caution for `genai`. The pin is a workspace dependency like any other.
- **Failure is typed and loud.** A keystore that refuses or is unavailable is an error the
  calling surface renders (Settings shows it at Apply; AS-02's client construction surfaces
  it as its config-error path). Never a silent plaintext fallback into config — that would be
  the exact failure this task exists to prevent.
- **The draft rule**: a pasted secret lives in the Settings draft's memory only. Apply writes
  the store, then commits the marker through `write_config`. Clearing the field deletes the
  store entry and removes the marker in the same Apply.

## Signing and the bundle

Keychain access is per-code-signature: a `cargo run` dev binary and the bundled, signed
`.app` are different principals to the Keychain, so an item written by one may prompt or
refuse under the other. That is expected macOS behavior, not a bug — but it must be *known*
behavior: verify what the bundled app actually does (`scripts/bundle-macos.sh`, the signing
rungs in `docs/RELEASING.md`) and record the answer in this file when it lands. The app
bundle stays self-contained (AGENTS.md §7); if an entitlement is needed it ships in the
bundle script in the same change.

**No entitlement is needed.** The `keychain` store is the unsandboxed legacy Keychain
Services store; entitlements (`keychain-access-groups`) are the sandboxed/data-protection
store's, which is the `protected` feature we deliberately do not enable. `bundle-macos.sh`
gains nothing but the `CFBundleIdentifier` read.

**What the signing rung changes, and why it matters to a tester.** Keychain access is granted
against the item's ACL, which records the creating application's *designated requirement*. A
Developer ID signature gives a stable one (the bundle identifier plus the team certificate),
so the app keeps reading its own items across updates. An **ad-hoc** signature (`codesign -s -`,
what `bundle-macos.sh` falls back to with no certificate) has no such anchor: the requirement
is pinned to the binary's own hash, so **every ad-hoc build is a new principal** and a tester
would be re-prompted for keychain access after each one. That is a property of the signing
rung, not of this store, and it is the strongest practical argument for the Developer ID rung
in `docs/RELEASING.md`. Recorded here from Apple's model, **not** yet observed — see below.

## What is not proved yet

Two of the acceptance items below are marked `manual` rather than done, and neither can be
closed by this change:

- **The bundled, signed app round-trips its own secret.** Nothing in the app writes a secret
  until AS-03 ships the roster field, so there is no gesture on the DMG build to exercise.
  Owner stays this file; the check belongs to whichever of AS-03 / the first release build
  comes first, and the cross-signature note above is what it should confirm or correct.
- **A cross-process (restart) round trip.** Deliberately not automated: macOS grants keychain
  access per code signature, so a second binary reading the first's item prompts, and in a
  test run that is a dialog nobody is there to answer — a hang, not a failure. `cargo test`
  must not be able to hang. The automated test therefore reads only what it just wrote, in
  the process that wrote it, and mints a fresh reference every run so it can never meet an
  item another build left behind. **The runner was decided against, as this file asked**: a
  locked-keychain test would need the keychain locked *and* the process unable to prompt,
  which GitHub's macOS runner does not offer as a state you can ask for. `keyring_core::mock`
  answers the same question exactly (`NoStorageAccess` is what a locked keychain maps to) and
  answers it on every platform.

## What is NOT this task

- **Migrating `AgentAccess::token`** — recorded follow-on, deliberately out of scope: the
  token is locally minted and low-value, and migrating it means a config upgrade path. When
  it happens it happens here (this file's owner), as its own change.
- No secret *generation*, no passphrase UI, no cross-device sync. Put/get/delete, one
  keystore, references in config.
- Connections stay secret-free (profile name and key file path, never a key) — this store
  does not change that rule and must not be offered there.

## Acceptance

- ✅ Round trip through the real keystore: `put` → `get` returns the secret, a second `put`
  replaces it, `delete` removes it and a subsequent `get` is `Ok(None)` — not a panic and not
  an error (`tests/secret_keystore.rs`). The *restart* half is `manual`, for the reason in
  *What is not proved yet*.
- ✅ The secret has no serde path at all: `Secret` derives no `Serialize`, so a key cannot
  reach the config file even by mistake. The marker round-trips through serde and compares by
  value, which is what `settings_merge!` needs of a field.
  **Owed by AS-03:** the marker's first actual `Settings` field, and with it the
  `write_config` leg of this line. Building one here would be building AS-03's config early
  (AGENTS.md §5) — the field is the roster entry's, not this task's.
- ✅ Keystore-refused surfaces as the typed error at the call site: `keyring_core::mock` is
  driven to refuse a read, a write and a delete, and each answers `SecretError` rather than an
  absence or a fallback. The runner was decided against for a locked-keychain test — reasoning
  in *What is not proved yet*.
- `manual` The bundled, signed app round-trips its own secret. Not exercisable until AS-03
  ships a gesture that writes one; see *What is not proved yet*.
