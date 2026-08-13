# DB-05 · The data-sources tree: the catalog pane redesigned

**Workstream:** Database connections · **Status:** ⬜ · **Depends on:** DB-02, DB-04

## Goal

One tree answering "what data do I have" — the DataGrip model (Alex, 2026-08-13), replacing
both the flat-section catalog pane and the separate Connections pane. Top level is data
sources: the **project workspace** (the def-driven tables/views/saved queries, exactly the
rows and failure states the pane shows today, re-homed), each **database connection**
(enabled schemas → Tables and Views groups → columns, from the engine's discovery caches),
and each **object-store connection** (status node whose children are navigation links to the
workspace defs reading through it). Connection status, Edit, Forget, `+` and the schemas
picker all live on the tree. The invariant underneath does not move: defs stay
`ProjectState` rows, remote listings stay discovery reads — this task changes what the pane
paints, never where truth lives.

## Current state (verified 2026-08-13)

- **The catalog pane** (`apps/project/views/sidebar/catalog/`): `mod.rs` (TABLES · VIEWS ·
  QUERIES flat sections + filter), `section.rs` (collapsible section), `entry.rs` (the row:
  icon, name, meta label, status slot with `PROGRESS_HOLD`, expand-to-columns), `columns.rs`
  (nested column tree), `menu.rs` (`use_catalog_actions`: Refresh, Configure, Drop, rename),
  `interaction.rs` (1659 lines of tests — these define the workspace behavior that must
  survive re-homing). Expansion state is pane-local: **two** `use_state(HashSet<String>)`
  sets keyed `"{kind}::{name}"` (mod.rs:105-109) — already kind-qualified; a tree
  generalizes the key to a stable **node path**.
- **The TABLES `+` is unchanged by IT-01** (landed 2026-08-13): it is still one press setting
  `ConfigureTarget::New`. Creating a table Strata stores is a third LOCATION *inside* that
  window (Local · Remote · Internal), not a second gesture here — a menu on the `+` was built
  and rejected. So this task moves one press, as it always did.
- **The fork ships the tree** (corrected in review — do not hand-roll one):
  `freya::components::{Tree, TreeItem, Disclosure}` + `TreeConfig`
  (`crates/freya/crates/freya-components/src/tree.rs`) — virtualized over
  `VirtualScrollView` (staleness handled inside), depth + indent guides + disclosure +
  selection dress + `on_toggle`, its own `define_theme!`, a runnable example
  (`examples/component_tree.rs`), and Strata already consumes it in the results record view
  (`workbench/results/cell_view.rs:22`, `Tree::new_with_data`). AGENTS §3: standard
  components first — this task **themes and composes** the fork `Tree`; a gap found in it is
  fixed in the fork (§6), never worked around with a lookalike.
- **The Connections pane** (`sidebar/connections/mod.rs`, 570 lines): provider badge, address,
  status glyph + `PROGRESS_HOLD` hold, ⋮ Edit/Forget, `AddConnectionButton`, empty state,
  `ConnectionsHint`. All of it is absorbed or retired here. `SidebarPane::{Catalog,
  Connections}` arms are `sidebar/mod.rs:112-140`; the rail toggle and `Chan::Layout`
  plumbing follow the one-pane-per-edge rule (AGENTS §2).
- **Engine reads** (DB-02): `Engine::db_listing(url)` over the provider's caches — schemas
  (connect-time), relations per schema with relkind (lazy, `pg_class`), columns per table
  (cached provider's Arrow schema). No new network path; ↻ re-connects and thus refreshes.
- **Store reads**: `ProjectState` rows for defs and `ConnRow`s; `tables_over(url)` /
  `views_over(tables)` (project.rs:744, 761) give an object-store node its children and
  Forget its consequences.
- **Design**: the `.claude/design-handoff/` canvases predate this surface — there is no
  `.dc.html` for the tree. Build within the existing dress (catalog row vocabulary, metrics
  scale, theme roles); if a canvas arrives later, phase-5 polish reconciles against it.
- The remote-browse discovery reads follow the freya-query shape: keyed by
  `(url, catalog-epoch)`, `stale_time(MAX)` within an epoch, no store field ever holding a
  listing (INVARIANTS: "an expensive result is keyed by the request").

## Build

1. **Tree architecture** — the fork's `Tree`/`TreeItem`/`Disclosure` (Current state),
   themed for the sidebar and composed with `section.rs`/`entry.rs`/`columns.rs`' row
   dress: node identity is a stable path (`ws/tables/events`,
   `conn/pg/analytics/tables/sessions`), lazy children per node kind, expansion state
   pane-local keyed by path, one filter box matching names at any depth (auto-expanding
   hits, the current filter's behavior generalized).
2. **The workspace node** — first, expanded by default, **labeled with the project's name**:
   it is not a "files provider" but *the project's own database* — the catalog Strata's
   federating engine defines (`strata`), holding everything the project declares: file
   tables, internal tables, views, saved queries. This framing is load-bearing (settled with
   Alex, 2026-08-13): a **cross-source view** (workspace files ⋈ `pg.…`) nests here because
   the tree's node for a database groups by *what it defines*, not where the bytes live —
   the DataGrip/FDW precedent: a Postgres view over a foreign table lives under Postgres.
   Its definition persists in `project.json`, its join executes in our engine, its address
   is bare/`strata.public` — no other node could honestly claim it. Children are the current
   TABLES / VIEWS / QUERIES groups verbatim: same rows, same `Reg` status slots, same
   `use_catalog_actions` menus, same columns expansion. Re-home, don't rebuild — but budget
   the test migration honestly (corrected in review): beyond path-keyed expansion, the
   suite's fold tests assert against **hard-coded pane widths and pixel offsets**
   (`runner_sized(…, 240./150./130.)`, `CHEVRON_BACK_FROM_NAME`, the "rightmost 22×22
   square" ⋮ locator — interaction.rs:372-405, 987-1118), all of which an extra indent
   level shifts, and the harness fixtures carry no connection state at all
   (interaction.rs:243-292) while this task adds connection nodes and deletes the pane the
   other fixtures assumed. The workspace *behaviors* re-home; the harness geometry and
   fixtures are a real rewrite, and the fold thresholds are re-tuned, not preserved.
3. **Database connection nodes** — badge, address (or catalog name — show both: name
   primary, address as the dim meta label), status glyph with the `PROGRESS_HOLD` semantics
   carried over; refusal states point at Problems exactly as the Connections pane did.
   Children: the def's enabled `schemas` (∩ the live enumeration; a named-but-missing schema
   renders as its own failed node naming the fact) → **Tables** and **Views** groups (split
   by the listing's relkind) → columns from the Arrow schema. Fetch states per node in the
   pane's dress; a failed fetch names the engine's error and does not retry on its own.
   ⋮ menu: **Edit** (the editor window), **Forget**, **Schemas…** — a picker over
   `Engine::db_listing`'s scoped-and-tagged answer (DB-02: `Live | EnabledButMissing |
   NotEnabled` — the tree, the picker and completion all read that one function, never
   re-deriving visibility from `PgStore.schemas`). The picker's write **cannot go through
   `upsert_connection`** (corrected in review — that replaces the row with a fresh
   `Reg::Loading` that only a rescan answers, i.e. a permanent spinner): the store grows a
   def-in-place write (`update_connection_def`, preserving the row's `Reg`) + 
   `persisted_defs`, legitimate exactly because enablement is display-only. A connection
   sitting `Reg::Failed` has no live enumeration: the picker then lists the def's own
   `schemas` with the connection's failure named, rather than an unexplained empty list.
4. **Object-store connection nodes** — badge, address, status glyph, ⋮ Edit/Forget.
   Children: the workspace defs reading through the connection (`tables_over`), as
   **navigation links** — a press reveals the def's row under the workspace node (expand +
   scroll), never a second editable row; the link row carries a jump affordance, no menu.
5. **Forget on the tree** — same `DropTarget::Connection` confirm; the database-connection
   arm's wording lands here (moved from the first draft of DB-04): "Removes the database
   connection. Nothing in the database is deleted." Consequences come from the **existing
   forget path** (`tables_over`/`views_over` + `forget_consequence`,
   drop_confirm.rs:279-283) extended for databases: no `TableDef` names one, so the match
   is views whose recorded deps carry the connection's catalog prefix — which is what
   DB-03's qualified `plan_deps` recording exists to make findable (`left_invalid` is the
   drop arms' *sentence formatter*, `pub(super)` to `engine::ddl` — not a data source and
   not reachable from here). The derived keystore entry is deleted through
   `secret::forget_derived` (re-derivable from the def being forgotten — no stored ref).
6. **Retire the Connections pane** — `SidebarPane::Connections` removed, the rail loses the
   toggle, `+` moves to the tree header, empty states merge (a project with no connections
   shows the workspace node plus the add CTA), `ConnectionsHint`'s content folds into the
   header ⓘ or goes. No orphaned code: the pane module is deleted, not stranded. The enum
   lives in **`strata-model/src/session.rs:67-70`** with the retired-pane tolerance
   machinery and its test (`a_stored_sidebar_pane_reads_back`, session.rs:286-291) — a
   stored `"Connections"` becomes a **retired** pane read as the fallback, and the test must
   keep distinguishing known-pane from retired-pane after the removal (with one live
   variant, rewrite it so both halves don't collapse into one assertion), because that
   tolerance is what keeps an old `session.json` from being moved aside and costing the
   user every open tab.

## Acceptance

- Every existing catalog-pane interaction test passes re-homed (workspace behavior
  unchanged); the connections pane's test coverage is re-expressed against the tree nodes.
- Against a container-backed connection in the running app: the database node lists real
  schemas, tables **and views**; the schemas picker persists and re-scopes the tree without
  a reconnect; collapse/expand does not re-fetch within an epoch; ↻ refreshes.
- An object-store node's child press lands on (reveals) the workspace def row.
- Forget from the tree removes node + catalog + keystore entry; a dependent view is named in
  the confirm and settles `Failed` afterwards.
- `grep`-proof: no `ProjectState` field holds a remote listing; `SidebarPane::Connections`
  is gone from the codebase.
- Docs in the same change: `CONNECTIONS_SPEC.md`'s pane section rewritten for the tree,
  `docs/reference/MODULE_MAP.md` + `FREYA_UI.md` notes, AGENTS §2's "each edge offers one
  pane at a time" untouched (the left edge now has one fewer pane to offer) — and
  INVARIANTS.md's pane enumeration ("catalog · agents · connections",
  INVARIANTS.md:1674-1675) updated, since it names the pane this task deletes.

## Files

`crates/strata-freya/src/apps/project/views/sidebar/catalog/` (the tree; heavy) ·
`crates/strata-freya/src/apps/project/views/sidebar/{mod.rs, connections/ (deleted)}` ·
`crates/strata-model/src/session.rs` (`SidebarPane` + the retired-pane test) ·
`crates/strata-freya/src/apps/project/views/dialogs/drop_confirm.rs` ·
`crates/strata-freya/src/apps/project/state/project.rs` (`update_connection_def`,
jump-to-def support) · `docs/CONNECTIONS_SPEC.md` ·
`docs/reference/{MODULE_MAP, FREYA_UI, INVARIANTS}.md`.
