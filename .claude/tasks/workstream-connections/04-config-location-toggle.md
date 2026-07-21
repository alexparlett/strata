# Connections 04 · Config LOCATION toggle + object-store branch

**Workstream:** Connections (W7) · **Status:** ⬜ · **DEV_TASKS:** U14 · **Depends on:** 01, P4-11

## Goal
Register tables over a remote connection from the config modal.

## Current state
Not built. P4-11 (config/register-table modal) leaves a hook for LOCATION.

## Build
- A **LOCATION** toggle in the register-table modal (P4-11): **Local** vs a **connection** (S3/GCS/HTTP).
- On a connection, resolve paths against its object store and `register_external` over the remote store.

## Acceptance
- [ ] A table can be registered over a remote connection (paths resolve against its object store).

## Freya / references
- Design: `Configure.dc.html` LOCATION. Core `register_external` + `object_store`. DEV_TASKS U14/W7. Depends on P4-11.
