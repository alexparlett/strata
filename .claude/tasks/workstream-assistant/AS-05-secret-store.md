# AS-05 · The secret store

**Workstream:** Assistant · **Status:** ⬜ · **Depends on:** — (pure mechanism; AS-03
consumes it. Can land first or in parallel with AS-01/02.)

## Goal

One shared mechanism for secrets the app must keep: OS-keystore-backed storage (macOS
Keychain first), with **config storing a reference, never the secret**. This extends the
connections posture — "no arm of `engine::store` takes a secret" (AGENTS.md §2) — to the one
place the app now genuinely must hold one: third-party API keys for the assistant's provider
roster (AS-03). This is the shared implementation under §5 cross-task ownership: one
mechanism, owned here; consumers call it, they never grow their own.

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

## What is NOT this task

- **Migrating `AgentAccess::token`** — recorded follow-on, deliberately out of scope: the
  token is locally minted and low-value, and migrating it means a config upgrade path. When
  it happens it happens here (this file's owner), as its own change.
- No secret *generation*, no passphrase UI, no cross-device sync. Put/get/delete, one
  keystore, references in config.
- Connections stay secret-free (profile name and key file path, never a key) — this store
  does not change that rule and must not be offered there.

## Acceptance

- Round trip: `put` → restart (new process) → `get` returns the secret; `delete` removes it
  and a subsequent `get` is the typed absence, not a panic.
- The config file's bytes never contain the secret; the marker round-trips through
  `write_config` and `settings_merge!`.
- Keystore-refused surfaces as the typed error at the call site (simulate per what the
  pinned `keyring` source offers — a mock/in-memory backend if it has one, a locked-keychain
  test only if CI's runner proves to allow it; decide against the runner, not this file).
- The bundled, signed app round-trips its own secret (manual check on the DMG build; record
  the cross-signature behavior observed).
