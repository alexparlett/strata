# DB-04 · The connection editor's Postgres form

**Workstream:** Database connections · **Status:** ⬜ · **Depends on:** DB-02

## Goal

The Postgres arm reaches the user through the connection editor: a fourth provider segment
whose rows are the arm's own (address, catalog name, user + password, SSL), the derived-ref
password capture, and Save's identity-move migration. Editor only — where connections are
*shown* (status, Edit/Forget, browsing) is DB-05's redesigned data-sources tree, which absorbs
what the first draft of this task put on the Connections pane.

## Current state (verified 2026-08-13)

- The editor (`crates/strata-freya/src/apps/connection/`): the window doc is mod.rs:1-47
  (window-not-modal, single-instance, no secret held); the per-provider-rows rule is
  `views/form.rs:10-11` — "a control that cannot mean anything for the chosen provider is
  **not a control**" (spec form at CONNECTIONS_SPEC.md:168), and the pill is a
  `SegmentedToggle` at form.rs:141-153. `ConnectionDraft` (model.rs:107) with
  `of`/`def`/`blocker`/`set_address`/
  `address_label`; the footer owns the one error string (blocker → Save disabled + explained).
  Save (views/footer.rs:180): upsert + persist in one guard, `engine.disconnect(old)` when the
  URL moved, whole-catalog rescan, watch own row. **A Save-time probe was built and withdrawn**
  (doc at footer.rs:163-179) — do not add a "Test connection" button; the pass is the probe.
- The password-capture pattern is `apps/settings/views/ai/keys.rs:96-125`: typed + no marker →
  mint + put + record ref; cleared → delete + drop ref; `Secret::new("")` is `None` and no
  `Secret` **is** a delete. Keystore calls go through `task::offload`.
- The pane (`views/sidebar/connections/mod.rs`): badge from `Provider::to_string()`, address,
  status glyph with `PROGRESS_HOLD`, ⋮ menu (Edit/Forget), row not clickable. Forget confirm
  (`dialogs/drop_confirm.rs`, `DropTarget::Connection` arms at 89-160) lists tables over the
  bucket + views behind them via `tables_over`/`views_over` (project.rs:744,761).
- Form components: `components::form` (`Form` > `Row` > control, AGENTS §3); the standing
  credentials note is a form row the S3/GCS arms already render.

## Build

1. **Draft + form**: `ConnectionDraft` grows the Postgres fields; the pill's fourth segment
   shows rows —
   - **ADDRESS** — one box, `host:port/database`, placeholder `localhost:5432/appdb`;
     `address_label` says which spelling this provider wants.
   - **CATALOG NAME** — the SQL identifier queries will use; helper text gives the shape
     (`pg` → `pg.public.orders`); blocker on invalid identifier / fold-collision with `strata`
     or another connection (the one rule from DB-02's shared check, same words).
   - **AUTHENTICATION** — USER box; PASSWORD box following `ai/keys.rs`'s capture rule (a
     set password renders as a placeholder marker, never the value; typing replaces, clearing
     deletes) — but against the **derived** ref
     (`SecretRef::derived("pg-password", &def.url())`), with the def recording only
     `PgPassword::Keystore` (README decision — the def never stores a ref). **The marker
     must be honest per machine** (corrected in review): `ai/keys.rs`'s marker is honest
     because its ref is minted locally when the secret is put; a committed expectation is
     not, so the row probes the local keystore **once at mount** (an editor-scoped
     `task::offload` read) and renders three states — *no password expected*, *stored on
     this machine*, and *expected, not stored on this machine* (with an "enter your
     password" affordance that puts the entry and **does not touch the def**). Clearing is
     two different gestures with two different meanings: *remove from this machine* deletes
     the local entry and leaves `PgPassword::Keystore` (other machines unaffected), while
     *this connection uses no password* is a deliberate def edit to `PgPassword::None` —
     never conflate them, because the second, made casually on a machine with no entry,
     breaks the colleague who has one. No mode pill in v1 — a password is optional
     (server-side `trust`/`peer` setups exist), so absence is a valid state, not a mode.
   - **SSL** — mode `Select` (disable/prefer/require/verify-ca/verify-full; default the
     crate's `verify-full`) and a ROOT CERT path row shown only for the two verify modes.
   - **No REGION / ENDPOINT / CLIENT OPTIONS rows** — they are object-store vocabulary.
   - The standing note's Postgres wording: the password lives in the OS keychain on this
     machine and is never written to the project file; a colleague opening the project adds
     their own.
2. **Save**: unchanged funnel. Three Postgres specifics: an identity move (address or user)
   deregisters the old URL exactly as today, **and migrates the keystore entry** through
   DB-02's `secret::migrate_derived(old, new)` — one call, the funnel owns the get → put →
   delete and its best-effort semantics (this machine may hold no entry; the expectation
   still travels and other machines re-enter against the new ref). A **catalog-name** move
   with an unchanged URL is **not** Save's problem (corrected in review — the URL-move guard
   never fires for it): DB-02's `db::connect` replaces on re-connect, deregistering the
   catalog the pools map holds for that URL, and Save's whole-catalog rescan is what
   re-connects — verify the rename lands there, and add the interaction test that renames
   `pg` → `warehouse` and asserts the old name stops resolving after the pass. And an edit
   that abandons a stored password (provider switched away from Postgres, or the deliberate
   `PgPassword::None` edit) calls `secret::forget_derived` at `def()`-assembly time.
3. **No SCHEMAS row** — schema enablement is a tree-node gesture (DB-05), where the live
   enumeration already sits and where the one picker serves New and Edit alike; the editor
   adding a second surface for the same list would be two controls that can disagree. (The
   first draft's reason — "a new connection has no schema list" — was only half true: an
   *edited* green connection has one. The one-surface argument is the real reason; record
   it, not the half-truth.)
4. **Command palette / launcher**: nothing new — *New connection…* already opens the editor,
   and the provider pill is where Postgres appears.

**Moved to DB-05 with the pane fold** (recorded here so the seam is explicit): the `PG`
badge + status display, Forget's database-connection consequence wording and its
keystore-entry deletion, and every other showing-a-connection surface. This task leaves the
existing Connections pane rendering a Postgres row however `Provider::to_string()` falls out —
functional, unpolished, and short-lived.

## Acceptance

- Round trip in the running app: create a Postgres connection against a local container,
  watch it settle green, `SELECT * FROM pg.public.t LIMIT 10` in the editor returns rows;
  break the password, ↻, it settles Failed with the auth refusal naming the user.
- The password never appears in `project.json`, the log, or the form after save (marker only);
  an identity move migrates the keystore entry (assert old ref empty, new ref set).
- Interaction tests in the editor's existing style (`apps/connection/interaction.rs`): the
  Postgres row set renders per provider, blockers block with the shared wording, no network in
  any test (the withdrawn Save-probe lesson — connect paths stay out of UI tests).
- `docs/CONNECTIONS_SPEC.md`'s editor section updated in the same change.

## Files

`crates/strata-freya/src/apps/connection/{model.rs, views/form.rs, views/footer.rs,
interaction.rs}` · `docs/CONNECTIONS_SPEC.md`.
