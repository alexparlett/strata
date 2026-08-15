# DB-05 · The data-sources tree: the catalog pane redesigned

**Workstream:** Database connections · **Status:** ✅ (2026-08-14) · **Depends on:** DB-02, DB-04

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
- **Engine reads** (DB-02, as built): **`Engine::db_listing(&ConnectionDef)`** — the def, not the
  URL, because the tagging reads `PgStore::schemas` off it. It answers
  `Option<(String /* catalog name */, Vec<db::SchemaListingView>)>`, where a
  `SchemaListingView { name, relations: Vec<db::Relation { name, relkind }>, visibility }` is
  already **scoped and tagged** (`SchemaVisibility::{Live, EnabledButMissing, NotEnabled}`) — so
  the tree and the schema picker read one answer and neither re-derives visibility. `None` means
  "not a live database connection".
  It is **synchronous and free**: it reads the connect-time enumeration held beside the pool, not
  the network, so it needs no freya-query keying of its own. Columns per table are still a read
  through the cached provider's Arrow schema (DB-07). ↻ re-connects, and that *is* the refresh.
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
   is views whose **`ViewInfo::remote_deps`** carry the connection's catalog prefix — the
   list DB-03 built for exactly this, holding each remote scan qualified whole
   (`pg.public.orders`) while `deps` stays workspace-only. ✅ **the data is there**; the
   match itself is this task's (`left_invalid` is the
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

---

## As built (2026-08-14)

Everything above shipped, with four corrections worth recording.

**The fork's `Tree` wrapper is not used; its row vocabulary is.** The pane composes `TreeItem`,
`Disclosure` and `TreeConfig` — themed once, with the app's own chevron through `TreeItem::arrow`
— as **nested components** under the catalog's existing `ScrollView`, rather than through `Tree`.
`Tree` is `VirtualScrollView` over a flat list of *visible* rows, so it needs the row count
synchronously; this tree's rows fetch as they open (a status glyph subscribes, a Profile
dispatches, a remote relation would introspect), and answering the count would mean mirroring
those query results into a pane-local map — the "never a mirror on a store" rule, one surface
along. Nesting also keeps the per-`ProjChan` subscriptions the flat pane had, which a
root-built row list would have collapsed into one. `Tree` stays where its contract fits: the
results record view.

> **Overturned by the follow-up below (2026-08-14).** The reason did not survive inspection: a
> relation is a leaf, `db_listing` is synchronous, and a def's columns come off its `Reg` row, so
> nothing the count needs is fetched on open. The tree is virtualized now.

**Postscript (2026-08-14):** `SavedQueryRow`'s keying problem below is moot — no row in the tree is
keyed any more, because a virtualized window reconciles positionally. The fork differ bug it names
is still real and still unfixed; it is simply no longer in this pane's way.

**One fork change came with it** (AGENTS §6): `TreeItem` is now every row in this pane, so it
gained the `Link` role, tab stop and keyboard focus ring that `SideBarItem` already carried,
both following whether the row is pressable — plus the `focus_border_fill` theme field the ring
needs. (The fork is pushed and the gitlink moved; the follow-up below adds three more fork commits
on top of it.)

**A relation is a leaf.** Build §3 asked for columns under it; they are DB-07's, along with the
`ColRef` widening that would let one be selected. A column row the tree can draw but not address
is a row with nothing behind it, and the fetch that fills it is the same introspection the
inspector performs — noted in DB-07's file.

**`secret::forget_derived` resolved to `engine::db::password_ref`.** A bare alias over
`SecretRef::delete` would have been the redundant half; what was actually missing was one place
that answers *which* keystore slot a connection def owns. The editor's `password_ops` and the
Forget arm both go through it now, so the derivation is written once.

Two smaller notes. `SidebarPane` keeps its enum with one variant, because `sidebar_pane`'s
retired-name tolerance is what stops an old `session.json` costing the user every tab — its test
now asserts the known name, the retired name and `null` together, since with one variant a test
that only checked the known one would pass against a reader that had lost the distinction. And a
connection row's refusal now carries the engine's own words clipped to `TIP_CHARS`, replacing the
Connections pane's fixed "see Problems" pointer: the newer rule (a limit belongs to the surface
that has it) supersedes, and both kinds of row now say as much as they can hold.

The fold thresholds were re-tuned rather than preserved, as the task allowed: the measured run is
the row's **content** alone, since a tree row's indent, chevron and ⋮ lay out outside it, so there
is no pinned tail in the arithmetic. The interaction suite's positional `Handles` tuple became a
struct in the same change — every test destructured it with a `..`, so adding a handle silently
re-pointed a dozen bindings.

## What a max-effort review changed (2026-08-14)

Ten adversarial finder angles over the finished diff; fifteen findings, thirteen applied.

**Three hook-order panics.** `StoreNode` called `use_state` and `name_width` (a hook, through
`scale()` → `use_theme()`) *after* its filter's early return, and `DatabaseNode` did the same with
`name_width` alone — so a connection node whose first render was narrowed away allocated fewer
hooks than its next one asked for, and Freya hard-fails on that. The schemas picker had the same
shape from the other end: `tones()` is a hook and was called twice **inside the per-row closure**,
making the picker's hook count a function of how many schemas the server had dropped. All three are
hoisted; the rule is now stated on `StoreNode`'s own doc.

**The filter never opened what it kept.** A node survived on a descendant match and then hid the
match, which is worse than not keeping it — and connections start collapsed, so a filter reached
them not at all. Build §1's "auto-expanding hits" is implemented now, at all four container kinds,
and `TreeCtx::is_open`'s doc says which half the caller owns.

**The jump was a layout handler.** Answering a reveal from the target row's `on_sized` meant a row
already on screen never fired one, and the request then went off at the next unrelated relayout.
It is an effect over the reveal slot and the row's last measured area, and it scrolls to the
**row's** area rather than the wrapper's, so an expanded entry lands on its name and not on the
bottom of its column block.

**Two smaller mistakes about what a row knows.** A connected database drew no chevron while
collapsed, because `live` was derived from a listing the node only fetched once it was already
open — it reads the `Reg` now, and takes its catalog label from the def. And `fold_plan`'s third
slot was budgeted at an icon's width while a database row spends it on a variable-width catalog
name; the slot is `mark` now and its width is the caller's, `0.` on a row that draws none.

**The schemas picker's Cancel did not discard**, since the dialog is mounted for the window's life
and the draft was only re-seeded on a *different* connection. Closing drops the seed.

**One finding could not be applied.** `SavedQueryRow` is the only row in the tree still unkeyed:
keying it crashes the fork's reconciler on the one gesture the key exists for — a rename re-sorts
the list, the keyed row moves, and `Tree::apply_mutations` unwraps a `moved` node its parent no
longer holds (`freya-core/src/tree.rs:332`). `EntryRow` and `ColumnRow` are keyed, which is what
stops a filtered-out row handing its status glyph and hover state to the row that shuffles up.
**That differ bug is owed a fork fix**, and until it lands this one list reconciles positionally.

**Left as known cost:** `Engine::db_listing` deep-clones a connection's whole relation enumeration
per call, and an open schema clones its relations again per group. The call is now gated on
`connected && (open || filtering)`, so a collapsed or unconnected node pays nothing, but a large
database still pays per render while open or while a filter is being typed. Sharing the listing by
`Rc` down the subtree is the fix and it is not in this change.

---

## Follow-up: the tree is virtualized (2026-08-14)

The pane walks its own tree into a **flat list of visible rows** (`catalog/node.rs`) and hands that
to the fork's `Tree`. Only the rows on screen are mounted.

**Why the original reason did not hold.** "Rows fetch as they open" was true of a design, not of
what shipped: relations are leaves (columns are DB-07's), `Engine::db_listing` reads the
connect-time enumeration rather than the network, and a def's columns come off `Reg::Ready`.
Everything the row count needs was already synchronous. Meanwhile `RELATIONS_QUERY`
(`strata-engine/src/db.rs`) carries no `LIMIT`, so one Postgres schema is the only row count
in the app the *server* decides — which is the case that made mounting every row untenable rather
than merely wasteful.

**What went, rather than moving across.** `entry.rs`'s `use_reveal` + `row_area`: a virtualized
reveal is scroll-to-index off the flat list, so the "already on screen" and stale-slot cases it
handled stopped existing. The per-level relation clones (`db_listing` → `SchemaNode` →
`RelGroupNode`, each deriving `PartialEq` over a full `Vec<Relation>`): one walk, one clone per
*visible* row. The whole-tree re-render on any chevron press, and `Row`'s always-unequal derived
`PartialEq` over four `EventHandler` fields — both now bounded to what is on screen. And
`database.rs` + `store.rs` became one `connection.rs`: their fifty near-identical render lines were
one `connection_row`, because what actually differs between a bucket and a database is what opens
*underneath* them, which is the walk's business.

**One component type per row, which is the load-bearing part.** `view.rs`'s `TreeRow` is every row,
running every hook and dispatching on the node. Mixed component types in a scrolling window get
paired **by type** by Freya's differ, which hands one row's scope to a different row a level up the
list; a scope reused across two components allocates the wrong number of hooks and hard fails, and
it did, three times, before this landed. Keying the rows is not the escape — the window shifts by
one on every scroll step, so identity keys turn each step into a list of moves, which is the
`Tree::apply_mutations` path DB-05 already found a crash in. The corollary is that **every helper a
row reaches has to be checked for being a hook**: `name_width` read the type scale and
`type_palette` reads the theme, and both were reached from two arms out of twelve. `name_width` is
a pure function over `mono_advance()` now, and both are resolved once in `TreeRow`.

**What a slot may remember.** A virtualized row's scope is a *slot*, so anything it keeps is kept
about whatever scrolls into it. `RowCtx::measured` stays per slot because a run width is the slot's
own fact. `use_status`'s held verdict is now **tagged with whose it is** (the row's node path), or a
waiting row would show the previous occupant's triangle indefinitely. The saved-query rename flag
moved to `TreeCtx::renaming` as a `Option<Uuid>`, which is where it belonged anyway: one rename at a
time is a pane fact. The hold-back *timer* is deliberately not tagged — it says the pane has been
waiting a while, which stays true of the row behind one that was.

**What it costs, stated in `catalog/mod.rs`.** The walk reads Tables + Views + Queries +
Connections at the pane root, so a table registration now re-walks the tree where it used to wake
the TABLES group alone. That is the trade, and the module's Subscriptions section says so.

**Three fork changes** (`crates/freya`, AGENTS §6):

- `VirtualScrollView` publishes its viewport to the `ScrollController`, the line `ScrollView`
  already had. Without it `scroll_to_item`, `is_scrollable` and `is_at_end` were all silently inert
  on a virtual view.
- `ScrollController::scroll_to_offset(offset, size, direction)` — `scroll_to_item`'s virtualized
  twin, for a target with no measured rectangle because it has not been built.
- `Tree::scroll_controller(…)`, so a tree can be driven externally at all.

**What a max-effort review changed (2026-08-14, second pass)**

Ten finder angles over the finished diff; fifteen findings, thirteen applied.

*Four ways a slot was mistaken for a row.* **`Place::open()` derived the press's answer from the
chevron**, and the chevron is forced to `Leaf` whenever a node has nothing to open — so an
unregistered table, a reconnecting database and an emptied object store were all stored open,
reported closed, and could not be collapsed: the press inserted a path that was already there and
the node sprang back open when its children returned. `Branch` carries `open` and `can_open`
separately now, and the disclosure is derived from them rather than the other way round.
**`use_status` took `Option<&str>`'s job as `""`**, so the ten statusless row kinds wrote an
untagged "nothing is wrong" over whichever verdict the slot was holding; the tag is an `Option` now
and a row with no status says nothing. **The rename's draft stayed in the row** while its flag moved
to the pane, so scrolling the row out destroyed the typed text and re-seeded it from the stored
name — a commit then wrote the name the user had just replaced. Draft and flag are both on
`TreeCtx`, seeded where the rename is asked for, and Escape is the **pane's** listener because a
row's would go with the row. **And the profile subscription was still a row's**, which meant
scrolling away from a running scan dropped the read the user had accepted a cost confirm for; the
watchers are mounted at the pane root now, one per outstanding scan (`node::scans`), and the row
only draws.

*Two more.* The reveal cleared its request even when `scroll_to_offset` could do nothing because the
viewport was unmeasured — a jump on the pane's first frame was simply swallowed. And the body's
`PANE_BODY_MIN_W` floor had moved onto the pane's own frame, where it stopped the *panel* shrinking
instead of the content: measured at 100px, five rows laid out past the panel's edge. A tree row
ellipsizes rather than wraps, so the floor buys it nothing and is gone; the note that did wrap
ellipsizes like every other label, and `the_tree_lays_out_within_its_panel_at_stub_width` pins it.

*One walk input nothing subscribed to.* The workspace row renders `project.name`, which is
`ProjChan::Meta` — the pane read it and never woke for it, while the module doc claimed the
subscription list was complete. It is complete now.

*Two efficiency findings applied, two left.* The connection walk was re-finding each connection in
the list it was already iterating, with a `format!` per comparison (it reads the row's `Reg`); and a
collapsed object store materialised every link name only to drop it. **Left:** `db_listing`
still deep-clones a connection's whole enumeration, and now on every `Tables`/`Views`/`Queries`
write rather than only on `Connections` — the four-channel subscription made the known cost worse,
and sharing the listing by `Rc` is still the fix and still not in this change. `TreeData`'s derived
`PartialEq` is likewise an O(visible rows) compare run twice per render, which a stable `Rc` would
fix and a fresh one cannot.

*Two fork fixes, on top of the three above.* `VirtualScrollView` resolved its own controller inside
`unwrap_or_else`, so its hook count depended on a prop — harmless while the field was construction-
only, and a trap the moment `Tree::scroll_controller` handed callers an `Option`. And
`scroll_to_offset` now reports whether it could answer, with `is_measured` as the subscription that
wakes a caller when it finally can.

*One claim corrected.* `view.rs` said one component type per row meant the differ "never reports a
move". It means the **row list** stops reordering; a row's own children still change shape as the
fold plan gives marks up, and that reorders same-typed siblings one level down. The `unwrap` that
path ends in is owed a fork fix either way.

**Two tests** (`interaction::virtualization`): a 600-table project in a 400px viewport builds a
small fraction of its rows while the group count still reads 600, and a jump from an object-store
link reaches a row six hundred rows below the fold. The second one was checked against a stubbed-out
scroll and fails there, so it is not passing on the list happening to be short.

**Left as known cost, unchanged:** `Engine::db_listing` still deep-clones a connection's whole
relation enumeration per walk. The walk is now once per pane render rather than once per node per
level, and it is still gated on `connected && (open || filtering)`, but sharing the listing by `Rc`
is still the fix and still not in this change.

## What a max-effort review changed (2026-08-14, third pass)

Ten angles again, over the code *including* the second pass's fixes. Fourteen of fifteen applied.
The headline is that two of the second pass's own fixes were wrong.

**A fix built on a false premise, reverted.** The profile-scan watcher was moved to the pane root
because scrolling a row away looked like it dropped a running scan. It does not:
`freya-query`'s `use_query` says in as many words that "the running execution is deliberately not
cancelled on unmount", so the row-owned subscription never lost work. Worse, the replacement was not
durable either — `scans()` read the *walk's* output, so collapsing a group or typing in the filter
unmounted the watcher anyway, while its doc claimed "the pane outlives both". The row-owned
`watched_scan` design is back, with the virtualization case stated on it rather than assumed away.

**A fix that cost more than the bug, redesigned.** `is_measured` let the reveal retry once the
viewport was measured — by *subscribing* the pane to the viewport. The viewport moves whenever the
auto-hiding scrollbar appears or the sidebar is dragged, so one reveal bought a full re-walk of the
tree on every scroll gesture, for the life of the pane: the exact cost this whole change exists to
remove. The retry belongs to the controller, which knows when it is laid out — `scroll_to_offset`
keeps a request it cannot honour and `use_apply` drains it at the first layout. `is_measured` is
gone and the app subscribes to nothing.

**Two more fork faults the new API exposed.** `scroll_to_offset` revealed against the raw stored
scroll position, which the views never write back after clamping, so a stale position could report
an off-screen row as already visible; it clamps first now. And `ScrollController::managed` never
mirrored its position at all, so the new method read zero on the code editor's controller — the
mirror is refreshed in `use_apply`. `ScrollView` also still had the conditional-hook shape the
second pass fixed only in `VirtualScrollView`.

**Three smaller ones.** A blank Postgres catalog was `Some("")`, which budgeted a fold slot for a
mark the row drew as an empty label and cost the provider badge its room. The pane's Escape handler
wrote `renaming` on *every* `Command::Cancel` in the window (`State::take` writes before it tests),
waking every saved-query row; it is registered only while a rename is live now, which is the tab
strip's idiom and also keeps it out of the datagrid's way. And `has_links` was evaluated twice per
object-store connection per walk.

**Left as known cost.** The `PANE_BODY_MIN_W` floor is still absent. Its original placement was on
the scrolled *content*, where it let the content hold its width and the view pan; the pane no longer
owns that container, and putting it on the frame (the second pass's mistake) stops the panel
shrinking instead. Below roughly 50px a deep row's indent exceeds the inner width and its content
clamps to nothing with no way to pan to it. Restoring it properly needs a content-min-width on the
fork's `VirtualScrollView`, which is a new API this change did not take on.

**Documentation.** `Row`'s key machinery is gone (no caller sets one, and the differ now panics on a
duplicate sibling key, so leaving the setter was an invitation). `docs/reference/FREYA_UI.md` still
carried the "reports no moves at all" overclaim the code had already retracted; `MODULE_MAP.md` said
four subscriptions and listed a scroller `TreeCtx` no longer holds; DB-07's plan still directed work
at the deleted `catalog/database.rs` and at a row-local `Disclosure` the walk now owns. All three are
true again. And the doc comments that narrated this change's own history — in `node.rs`,
`connection.rs`, `view.rs` and, orphaned onto `update_connection_def` by the second pass's deletion,
in `state/project.rs` — say what the code is instead.

## What a max-effort review changed (2026-08-14, fourth pass)

Ten angles over the code including both earlier fix passes. Fifteen findings, fourteen applied. The
subject was mostly the third pass's own work, and the verdict on its central piece was unanimous.

**The reveal latch is gone.** The third pass moved the reveal's retry into `ScrollController` as a
`pending` slot drained by `use_apply`. Six of the ten angles found it independently, and between them
they found six distinct faults: `pending.write()` sat inside its own `pending.peek()` guard, so the
first deferred reveal would have hard-panicked on a borrow; the drain read the viewport before the
view published it and the content extent before `use_apply` refreshed it; nothing subscribed to
either, so no render was scheduled to retry on; the drain ran before the `requests` queue, so a
queued `scroll_to` in the same frame silently undid it; a later direct reveal left an older latch
armed to fire afterwards; and `reveal_offset` reported success against content of zero height. None
of it was reachable by the test suite, because the one caller always finds a laid-out tree.

So the mechanism is removed rather than repaired. `scroll_to_offset` is one imperative method again,
and its doc says what it cannot do: before the first layout there is nothing to reveal against, and a
caller that must move the view on the frame it mounts wants `scroll_to`, whose request the existing
queue already defers. The clamp against the content extent stays, and it now refuses a zero extent
instead of computing against it.

**Two fixes the third pass reported and did not make.** `has_links` was still evaluated twice per
object-store connection, and `docs/reference/FREYA_UI.md` still carried the "reports no moves at all"
overclaim. Both edits had been written as string replacements that silently failed to match, and both
were reported `fixed`. They are applied now, and the lesson is recorded here rather than in a doc
comment: an edit that cannot fail loudly has to be verified by reading the file back.

**The Escape handler stopped costing a walk.** Gating its registration on `renaming.read().is_some()`
subscribed the *pane* to the rename slot, so starting or cancelling a rename re-walked the whole
tree — twice per gesture, including `db_listing`'s clone per open database. The handler is
unconditional now and peeks, which also stops it consuming a `Command::Cancel` it has no use for.

**A rename now ends when its row leaves the tree.** The flag, the draft and Escape were the pane's,
but the commit-on-outside-press listener was still the row's, and a virtualized row is unmounted by a
scroll or a filter keystroke — so a rename could reach a state with nothing on screen to commit it.
The pane watches for its row leaving the walk and commits, which is what the outside press meant.

**`Row` has one root shape.** It wrapped its `TreeItem` in a rect only on the six kinds with a
context menu, so a slot crossing that boundary swapped an element root for a component root, which
the differ cannot pair: the whole `TreeItem` was rebuilt and its hover and keyboard focus went with
it. That is the per-scroll rebuild the one-type rule exists to stop, one level below where it was
applied.

**Smaller.** `Settled` is gone — `Reg::ready`/`Reg::error` already answer it, and the connection walk
now says "is it loading" the same way the workspace walk does. The mirror refresh in `use_apply` is
gated on a `mirrored` flag that only `ScrollController::managed` sets, since for a controller built
by `new` the getter reads the state being written and the refresh was provably a no-op on every
layout of every scrollable in the app. The viewport is published before `use_apply` rather than
after. And the fork's body comments became doc comments, as its own `AGENTS.md` asks.

**Left as known cost.** The `PANE_BODY_MIN_W` floor, unchanged from the third pass and for the same
reason. Two efficiency findings from the second pass also stand: `db_listing`'s per-walk deep clone,
and `TreeData`'s O(rows) `PartialEq`.
