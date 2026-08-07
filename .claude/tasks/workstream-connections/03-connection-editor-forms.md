# Connections 03 · Editor forms (S3 / GCS / HTTP)

**Workstream:** Connections (W7) · **Status:** ⬜ · **Depends on:** 01

## Goal
Per-provider connection editor forms.

## Current state
Not built. The model is `strata_model::ConnectionDef` (Connections 01) and it is what the form
edits — `Provider::{S3(S3Store), Gcs(GcsStore), Http}`, with the auth reference carried *inside*
the auth variant (`S3Auth::Profile { name }`, `GcsAuth::ServiceAccount { path }`).

## Build (to `Connections.dc.html`)
- Provider tabs/segment (S3 / GCS / HTTP); per-provider fields (endpoint, region, bucket; GCS
  service-account **path**; HTTP base URL/headers). Credentials by reference only.
- Validate + save into the connection model (task 01). `components::form` is the row vocabulary
  (AGENTS.md §3 — a settings-style surface is built from it, never its own rows).
- The store mutators the save needs: `upsert_connection` / `remove_connection` on `ProjectState`,
  persisted through `persisted_defs` like every other def mutation. Connections 01 left none —
  nothing referenced them. **Replace on `ConnectionDef::url()`, insert at the bucket-sorted
  slot**: the two are different keys and only one of them is identity, so an upsert matching on
  bucket would let saving a `gs://lake` connection silently replace the `s3://lake` one it sorts
  beside.

## What Connections 01 handed over

- **The def stores the authority alone**; the scheme comes from the provider
  (`ConnectionDef::url()`). So the form owns adding and stripping the prefix — the non-editable
  `https://` chip for HTTP, nothing shown for S3/GCS since the picker already states the scheme.
- **Core's refusals are the backstop, not the field errors.** `engine::store::connect` refuses a
  blank region, a blank profile name, a blank SA path and a bucket carrying a path — those are the
  same rules the form has to enforce at Save, and the wording there is written for a catalog row,
  not for a field. Keep them in agreement; don't route the form's validation through the engine.
- **Switching provider must sanitise the auth mode** (spec §1) — which the type already forces:
  `S3Auth` and `GcsAuth` are different types, so there is no invalid pair to guard against.
- **An edit that changes the bucket or the provider changes the connection's identity**, and
  `engine::store::connect` cannot clean up after it — it only ever sees the new def, so the store
  registered under the *old* `url()` survives. The edit gesture owns deregistering the old URL,
  the same call Forget needs (02).

## Acceptance
- [ ] Each provider's form validates + saves a connection; no secret is stored inline.

## Freya / references
- Design: `Connections.dc.html` (+ the conn VM in `strata-windows.js`). `docs/CONNECTIONS_SPEC.md`. DEV_TASKS W7.
