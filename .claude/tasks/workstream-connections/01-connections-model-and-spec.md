# Connections 01 · Model + spec (project-scoped, no stored secrets)

**Workstream:** Connections (W7) · **Status:** ✅ · **Depends on:** — · **Unblocks:** 02, 03, 04

## Goal
The connection data model + the rule that Strata never stores secrets.

## Current state
Built. Model, persistence, object-store registration and the project-store row all land here; the
surfaces (rail button, pane, editor forms, the Configure LOCATION toggle) are 02–04.

## What was built

- **`strata_model::ConnectionDef`** (`crates/strata-model/src/connection.rs`) — a **bucket plus a
  tagged provider**, where the provider *is* its own settings (`Provider::{S3(S3Store),
  Gcs(GcsStore), Http}`). Same argument as `SourceFormat`: a region means nothing to the HTTP
  store, and a def carrying every provider's fields has states where they disagree.
  - The bucket is the **authority alone** (`acme-lake`), not the scheme-qualified string —
    `ConnectionDef::url()` derives `s3://acme-lake` from the provider, so an `s3://` bucket under a
    GCS provider cannot be written down. That URL *is* the object-store registry key, and it is
    therefore the connection's **identity everywhere**: the pass's outcome, the store's landing
    methods and the work list all address a connection by `url()`, never by `bucket` — two
    providers can share a bucket (`s3://lake`, `gs://lake`) and are two connections.
  - Auth carries its own reference: `S3Auth::Profile { name }`, `GcsAuth::ServiceAccount { path }`.
    A profile named on an Ambient connection is not a state worth having.
  - **No arm anywhere takes a secret**, and that absence is the enforcement: an access-key field
    cannot be added without adding a variant that says so out loud.
- **Persistence** — `ProjectDefs::connections`, in the **committed `project.json`**, sorted by
  bucket like every other section. This closes `CONNECTIONS_SPEC.md` §5's open question against
  splitting the per-machine fields into `session.json`: a profile *name* and a key *file path* hold
  nothing a colleague may not have.
- **`strata_core::engine::store`** — `connect(ctx, def)` builds the store and registers it per
  bucket. All-or-nothing: it probes the credential chain **before** registering, and on `Err`
  deregisters whatever an earlier pass registered, so a connection is never both refused and live
  and its `Reg` row means what it says. The S3 arm wraps `aws-config`'s resolved credentials in an
  `object_store::CredentialProvider` (`SdkCredentials`, the datafusion-cli pattern), resolving
  **per request** so short-lived credentials refresh themselves. Refuses a blank region
  (`AmazonS3Builder` would silently assume `us-east-1`), a blank profile name, a blank SA path, and
  a bucket carrying a path.
  - **Ambient and Named profile are two providers**, and this is the one thing here most worth not
    re-deriving. `ConfigLoader::profile_name` configures the default chain's *Profile* arm; it does
    not move that arm in front of `Environment`. Built that way (as it first was), a Strata
    launched from a shell exporting `AWS_ACCESS_KEY_ID` signs as the environment identity while the
    pane shows the chosen profile, and a misspelled profile name still registers green. Ambient is
    `aws_config::defaults(...)`; Named profile is `ProfileFileCredentialsProvider` alone.
- **`Engine::connect`** — the facade method, on the engine's own runtime.
- **`register::register_pass`** grew a **first phase**: connections, then tables, then views. A
  table registered before its bucket's store fails on a def that is perfectly correct, and the
  diagnosis lands on the wrong row. `RegOutcome::Connection { bucket, result }` reports it.
- **The project store** — `ConnRow { def, reg: Reg<()> }` on `ProjectState::connections`, its own
  `ProjChan::Connections`, `connection_registered` / `connection_failed` / `reload_connections`.
  `Reg<()>` is honest: connecting *registers* a store, it does not infer anything, so the three
  states are the whole value. A whole-catalog ↻ re-connects; a single table's Refresh does not.
- **New dependencies** (`strata-core`): `object_store` with `aws`/`gcp`/`http` (feature-unified onto
  DataFusion's own copy, not a second one), `aws-config` + `aws-credential-types`. These raise the
  workspace's effective MSRV to **rustc 1.94.1**.

## Acceptance
- [x] A connection can be defined + persisted with no secret material stored; the engine can build
      an object store from it.

## What this left the other tasks

- **02 (pane):** the status dot is `ConnRow::reg` — green `Ready`, amber `Failed(why)` with the
  reason as the tooltip, exactly like a catalog row's triangle. **Forget** needs a
  `disconnect` — DataFusion has `deregister_object_store`, and `register_pass` is deliberately
  additive (it never deregisters), so the removal gesture owns that call, as the drop confirm owns
  `Engine::deregister`. Also 02's call: whether a refused connection *also* earns a Problems ▸
  Project row (`ProjectState::registration_faults` deliberately still covers tables and views only
  — see its doc comment). The pass already logs every connection outcome to the event drawer, so a
  failure is not silent in the meantime.
- **03 (editor forms):** the form owns adding/stripping the scheme prefix and the per-provider Save
  validation. Core's refusals are the backstop, not the field-level errors. Add/Edit/Forget need
  store mutators (`upsert_connection` / `remove_connection`) — not built here, nothing referenced
  them.
- **04 (Configure LOCATION):** the connection dropdown reads `ProjectState::connections` filtered
  by `Provider` variant; a table's source path is entered relative to `ConnectionDef::url()`. The
  provider's **label** (`S3` / `GCS` / `HTTP`) is left unwritten on purpose — it is a name 02's row
  badge and 04's picker have to agree on, so it wants one home, and shipping an accessor nothing
  called would have been pre-work.

## Freya / references
- `docs/CONNECTIONS_SPEC.md` (§5 now records the as-built persistence; §3 the bridge and the status
  probe). Design: `Connections.dc.html` (note "the JSON is never read into or stored by Strata").
  Core DataFusion `object_store`. DEV_TASKS W7. Invariants: `docs/reference/INVARIANTS.md`
  ("A connection registers a bucket…"), model: `docs/reference/ENGINE.md`.
