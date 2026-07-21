# Connections 01 · Model + spec (project-scoped, no stored secrets)

**Workstream:** Connections (W7) · **Status:** ⬜ · **Depends on:** — · **Unblocks:** 02, 03, 04

## Goal
The connection data model + the rule that Strata never stores secrets.

## Current state
Not built. `docs/CONNECTIONS_SPEC.md` is the spec.

## Build
- A **project-scoped** connection type (S3 / GCS / HTTP): endpoint/region/bucket + credential
  **references** (a key-file path, env var, or profile) — **never** the secret itself.
- Persist connections in the **project** (`project.json`), not the session; surfaced via the project store.
- Wire the object-store registration in `strata-core` (DataFusion `object_store`) keyed by connection.

## Acceptance
- [ ] A connection can be defined + persisted with no secret material stored; the engine can build an
      object store from it.

## Freya / references
- `docs/CONNECTIONS_SPEC.md`. Design: `Connections.dc.html` (note "the JSON is never read into or
  stored by Strata"). Core DataFusion `object_store`. DEV_TASKS W7.
