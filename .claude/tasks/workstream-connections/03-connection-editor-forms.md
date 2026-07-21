# Connections 03 · Editor forms (S3 / GCS / HTTP)

**Workstream:** Connections (W7) · **Status:** ⬜ · **Depends on:** 01

## Goal
Per-provider connection editor forms.

## Current state
Not built.

## Build (to `Connections.dc.html`)
- Provider tabs/segment (S3 / GCS / HTTP); per-provider fields (endpoint, region, bucket; GCS
  service-account **path**; HTTP base URL/headers). Credentials by reference only.
- Validate + save into the connection model (task 01). strata-forms is available for the draft.

## Acceptance
- [ ] Each provider's form validates + saves a connection; no secret is stored inline.

## Freya / references
- Design: `Connections.dc.html` (+ the conn VM in `strata-windows.js`). `docs/CONNECTIONS_SPEC.md`. DEV_TASKS W7.
