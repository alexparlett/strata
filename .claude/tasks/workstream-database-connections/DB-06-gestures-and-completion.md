# DB-06 · Gestures + completion over the tree

**Workstream:** Database connections · **Status:** ✅ (built 2026-08-14) · **Depends on:** DB-05

## Goal

The tree becomes a place work starts, and the editor knows the remote names: a remote table
node opens a query, pins as a view, and completion offers catalog-qualified names scoped to
the enabled schemas.

## What was built

1. **The quoting pair has one home.** `strata_core::engine::sql::ident` — `needs_quoting`,
   `quote_verbatim` and `qualified` (a dotted name rendered **segment by segment**, since
   quoting `pg.public.orders` whole makes it a bare relation with dots in it). It is the
   completion insert's own rule, lifted out of `complete/mod.rs`, which now calls it. The
   module docs state the difference from `engine::quote_ident` (fold-preserving, for a name
   whose identity is a workspace def's) and from `export::quote_col` (unconditional), so the
   choice is made by whose identity the name is rather than by which helper was nearest.
   `quote_ident` is now `pub` for the one surface that needs the *other* answer: Pin as view
   names the def the store will key.
2. **Query gesture** — a relation row's double-press or ⋮ *Query table* / *Query view*:
   `SELECT * FROM <catalog>.<schema>.<relation>` at the row-limit setting, in a new unrun tab.
   It is `view_row`'s own funnel widened, not a second copy: `menu::select_sql` composes for
   both, and the workspace gesture now renders its own `FROM` through `quote_ident` (it
   interpolated the raw def name before, so a def called `Sales 2024` composed a broken
   statement). A press that is **not** a mouse press runs the gesture rather than failing the
   double-test: wiring `on_press` is what makes the fork's `TreeItem` a tab stop with the `Link`
   role, and a tab stop that answers no key is what that component's own comment warns against.
3. **Pin as view** — ⋮ *Pin as view…*: `CREATE VIEW <relation> AS SELECT * FROM <address>`,
   composed into an unrun tab. Running it lands the def through the view funnel that already
   exists. It titles its tab with the **view being made** where Query titles one with the
   relation being read, because `open_or_focus` finds a scratch tab by name *and* text: two
   gestures asking for one name means the second never owns it, and its repeat press stacks
   `… 2`, `… 3` — the very thing that funnel exists to prevent. `Remote` therefore carries the
   three-part name **twice**, as an `address` (rendered, for the statement) and a `label`
   (plain segments, for the title) — a tab strip should not show SQL quoting.
4. **Completion** — `Catalog::databases` (a `DatabaseSym` per connection, built by
   `Engine::database_syms`) carries the catalog name from the **def** and the schemas and
   relations from **`Engine::db_listing`**. The offer: catalog names at relation-target
   positions (secondary tier — a qualifier is not something that can stand alone), enabled
   schemas after `catalog.`, relations after `catalog.schema.`, nothing after a third segment.
5. **Docs** — `COMPLETION_SPEC` §2/§4/§6/§7/§10, `CONNECTIONS_SPEC`'s tree section (the two
   gestures and a completion section).

## Corrections recorded while building

- **There is no warming step, so there is no interior-swappable handle.** This file's plan
  required one, on the premise that a listing is fetched lazily as the tree opens a node and
  so would arrive between catalog epochs. It is not: DB-02 enumerates a whole database — every
  schema, every relation — in **one round trip at connect**, and `db_listing` reads that. A
  listing therefore changes only at connect and disconnect, both of which are catalog-epoch
  events, so the ordinary snapshot rebuild already sees it and the offer is plain data on the
  `Catalog`. (What *is* lazy is the per-relation `TableProvider`, which is the column list —
  and that is exactly the thing this task does not offer.) The timing constraint the plan
  guarded against was real for the design it was written for; it does not exist in the one
  that was built.
- **A Forget did not bump the catalog epoch, and now does.** `engine.disconnect` takes a
  catalog off the session, which is the discrete catalog mutation `catalog_settled` exists for
  — without it, completion went on offering a forgotten database's names, and every open tab
  kept whatever verdict it had against a catalog that no longer resolves. One line in the drop
  confirm; it fixes the diagnostics half as much as this one.
- **The chain's head picks the namespace, not its tail.** `Context::Dot` carries every segment
  now (it carried the last one, alias-resolved). Reading the last segment alone made
  `pg.public.` indistinguishable from a relation called `public`, and would have let
  `pg.public.orders.` answer with the columns of a *workspace* table called `orders`. Alias
  resolution stays scoped to a single segment: an alias binds one name, and a qualified address
  has none.
- **A listing no longer clones the schemas nothing shows.** `db::listing` tagged every schema
  and cloned every schema's relations, and all three consumers then dropped the `NotEnabled`
  ones — the tree before it draws, the picker reading only the name and the tag, completion
  offering what the connection shows. A relation list is the *server's* to size, so
  `SchemaListingView::relations` is now filled for a `Live` schema only. The tree pays this on
  every walk and completion on every catalog epoch, so it was the one avoidable cost in the read.
- **The workspace catalog is deliberately not offered as a qualifier** (`strata.` completes
  nothing). Every workspace surface addresses its tables bare — the deepest naming assumption
  in the app — and nothing in the UI spells the catalog name, so offering it would invent a
  second way to say what already has one. Recorded in COMPLETION_SPEC §10 beside the other
  deliberate silences.

## Coverage

- `complete/tests.rs::qualified` — the offer at each segment, the catalog name's rank and
  detail, a connection that has never answered offering its name and nothing under it, the
  case-preserving insert, and the two negatives (a remote qualifier is never answered by the
  workspace; a project with no database offers exactly what it did before).
- `sql::ident` unit tests — the pair and the per-segment rendering.
- `catalog::interaction::gestures` — the two statements the gestures compose, including the
  two renderers in one `CREATE VIEW`. Unit tests rather than driven ones, and that is the
  fixture's limit rather than a choice: a relation row exists only while `db_listing` answers,
  which needs a live pool.
- `tests/postgres_federation.rs::qualified_offer` — the container-backed half: the offered
  names are the ones the catalog resolves, the composed address runs, and a non-enabled schema
  is absent from the offer while still resolving when typed.

## Left for DB-07

The relation row is still a leaf and its menu has two items. Columns, selection and Profile
(with the remote expression set and a confirm that says the scan runs on the server) are that
task's, and `sql::qualified` is what its remote `profile_sql` should render through.
