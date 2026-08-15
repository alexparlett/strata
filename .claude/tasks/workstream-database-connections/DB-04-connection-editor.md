# DB-04 · The connection editor's Postgres form

**Workstream:** Database connections · **Status:** ✅ · **Depends on:** DB-02

## Built (2026-08-13)

The picker offers `ProviderId::ALL`, `ConnectionDraft::of`'s clamp is gone, and the arm's rows are
URL · DATABASE · CATALOG · USER · PASSWORD · SSL MODE (+ ROOT CERTIFICATE for the two verifying
modes). Spec updated; `docs/CONNECTIONS_SPEC.md` "The connection editor".

**The address is split, and each row means one thing** (settled with Alex after the first build,
in two passes). The Build below put `host:port/database` in one box called SERVER beside a
CATALOG NAME box, which made two rows read as "the database" and only one of them be. So: the
address box becomes **URL** (`host:port`) and **DATABASE** (`appdb`) — a *form* split over the
one stored `ConnectionDef::address`, via `ConnectionDraft::pg_server`/`pg_database`, so the def
and `parse_pg_address` are untouched — and `PgStore::catalog` keeps the name **CATALOG**, with a
hint saying it is the catalog *prefix* Strata queries by rather than anything the server has.
(`catalog` is the federation engines' word — Trino and Athena both use it — which is why it was
there; `pg_catalog` being a Postgres system schema is why the hint has to say which is meant.)

**The reason a catalog prefix is needed at all is the next task.** Alex's ask — "we want users to
be able to write `select * from orders`, not `select * from pg.public.orders` every time" — is
**DB-09**, filed rather than folded in here: it needs `providers::in_workspace` to *resolve* a
bare name against a session-scoped current database instead of assuming the workspace, and
without that a view over a bare `orders` records a workspace dependency while reading Postgres.

**Three corrections to the Build below, each because the code said otherwise.**

1. **SSL defaults to `prefer`, not `verify-full`.** DB-02 landed `PgSslMode::default() = Prefer`
   as libpq's own default, the spec's def table says so, and a test pins it. The form seeds from
   `PgStore::default()` rather than overriding it, so nothing here restates a default.
2. **`secret::forget_derived` does not exist**, as this file's own Current state already recorded:
   a forget is `SecretRef::derived(…).delete()`.
3. **The keystore work is Save's, not `def()`-assembly time.** `blocker` builds a def per
   keystroke, so a keystore call there is a blocking platform call — on macOS a Keychain prompt —
   per frame. `views::footer::password_ops` plans the operations purely (migrate → put/delete) and
   Save runs them on a `task::offload` worker **in front of** the store write, under a new
   `Status::Storing`, so a keystore that refuses writes nothing. `Storing` is its own status
   because `Connecting(url)` is what `use_watch_connection` reads: set before the store write it
   would find an edited connection's existing `Ready` row and close the window over a save that
   had not happened.

**The rename test lives in the container suite, not here.** `postgres_federation.rs`'s
`reconnect_and_disconnect` already re-connects the same URL under a new catalog name and asserts
the old name stops resolving — which is the assertion Build 2 asks for, in the only place that can
make it. An editor interaction test cannot run a registration pass, and the rule that no UI test
dials out stands.

**Left to DB-05, as the seam below says:** the `PG` badge and status display, Forget's
database-connection wording and its keystore-entry deletion, and the schema picker.

## Goal

The Postgres arm reaches the user through the connection editor: a fourth provider segment
whose rows are the arm's own (address, catalog name, user + password, SSL), the derived-ref
password capture, and Save's identity-move migration. Editor only — where connections are
*shown* (status, Edit/Forget, browsing) is DB-05's redesigned data-sources tree, which absorbs
what the first draft of this task put on the Connections pane.

## Current state (verified 2026-08-13)

**What DB-02 landed in this window, and what it deliberately left** (2026-08-13):

- `ProviderPicker` iterates **`ProviderId::OBJECT_STORES`**, not `ALL`. Flipping it to `ALL` is
  this task's first line — and nothing else will remind you: `ProviderId::ALL`'s guard test no
  longer claims anything about a picker (it cannot, since neither picker reads `ALL`), so the
  suite stays green with Postgres unselectable. `ConnectionDraft::of` clamps a non-object-store
  def to `S3` for the same reason; that clamp comes **out** when the rows land, or a stored
  Postgres connection opens as an S3 one.
  (DB-02 narrowed it because offering `PG` before the rows exist produces a def with no fields
  to fill in.)
- `ConnectionDraft` already carries **`pg: PgStore`**, whole and unedited, and `of`/`def`
  round-trip it — so a hand-written def survives the window today. This task edits that field in
  place; it does not introduce it.
- The CLIENT OPTIONS **validator** is gated on `is_object_store()` to match its row. When this
  task adds rows it must keep the two in step: a rule with no control behind it blocks Save with
  nothing on screen to clear.
- `blocker`'s `ProviderId::Postgres` arm already asks `PgStore::check_catalog` (shape, and not
  `strata`) and `PgStore::check_user`. What it does **not** ask is `check_catalog_name` against the
  project's other connections — that needs the def list, so it belongs in the **footer**, beside
  the URL clash, exactly as this task's Build says.
- `address_label` / `address_noun` answer `SERVER` / `server`, and `set_address` strips a pasted
  scheme like every non-HTTP provider. `note()` has the keystore sentence.
- The password funnel is `strata_core::secret`: `SecretRef::derived(strata_engine::db::PG_PASSWORD,
  &def.url())` for put/get/delete, and **`secret::migrate_derived(&old, &new)`** for an identity
  move (address or user), which is built and is the one place that ordering lives. There is no
  `forget_derived` — a Forget is `SecretRef::derived(…).delete()`, which already tolerates absence.

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
