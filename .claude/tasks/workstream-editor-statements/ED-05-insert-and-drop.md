# ED-05 · INSERT (native, target-gated) + DROP TABLE (both origins)

**Workstream:** Editor statements · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** ED-04

## As built (2026-08-08) — where it differs from the draft below, and why

The shape is the draft's: one `drop_table` both surfaces reach, an origin-gated `INSERT`, the
dialog as the confirm in front of the funnel. Nine things were settled while building it.

1. **`ddl::execute` takes the internal-name set** (`InternalTables`, an `Arc`-shared handle on
   `Engine`). Both arms ask `is_internal` about a name only the *parse* produces, so neither can
   be gated from `Engine::run`; and the task `bookkeep` spawns must **not** hold an `Arc<Engine>`,
   because `Engine::drop` is what aborts that task — a task keeping the engine alive would keep
   the abort from ever arriving. The set holds names only, so it outlives an engine harmlessly.
   ED-06+ inherit the parameter.
2. **The engine's own bookkeeping is folded off the returned effect**, in one place —
   `Engine::settle_effect`, which `run` and `drop_table` both call. `TableRemoved` cancels the
   table's profile scan and clears its internal flag; `TableUpserted` records the origin as
   before. `cancel_profile` therefore lands *after* the deregister rather than before it: it
   needs the lifecycle lock, which the spawned task cannot reach, and the end state is identical
   (the scan is aborted either way and no new plan can resolve the name).
3. **The app-facing name is `Engine::drop_table`**, delegating to `ddl::tables::drop_table`. The
   pane has an `&Engine`, not a `SessionContext`, so it cannot call the free function directly —
   same funnel, one hop.
4. **Dependent views are read from the providers** (`catalog::dependent_views`), not from the
   store's `ViewInfo`. The report is the *engine's* sentence and ED-02's fold already said so
   ("named in the report's own sentence"); a view is anything in the schema still carrying a
   plan, and the plan was inlined at creation, so the nested reader is found with no recursion.
   The pane's before-the-fact warning still reads the store's recorded copy — the same fact from
   before the drop.
5. **Refusal wording stays in `Blocked`**, so `INSERT` into an external table answers
   `Blocked::InsertExternal` verbatim ("INSERT targets internal tables. Load external table data
   through Table Config") rather than the draft's name-carrying sentence. `Blocked` is `Copy` and
   is the one place a refusal is worded (validate.rs says so itself); a variant carrying a name
   would be a second vocabulary for one refusal.
6. **`Blocked::InsertOverwrite` was reworded** to "An INSERT that replaces rows is not supported…"
   because `REPLACE INTO` reaches the arm — only the *plan* names it, so the router cannot — and
   DataFusion folds it onto the same `InsertOp` the Arrow sink refuses. Not an agent-path message,
   so nothing pinned moved.
7. **INSERT drives the plan it gated** rather than re-dispatching the text through
   `ctx.sql_with_options`. `execute_logical_plan` special-cases only `Ddl` and `Statement` and
   hands a DML node to exactly this, so it *is* the native dispatch minus a second parse — and a
   second parse would gate one value and execute another. The dml-only `SQLOptions` triple is
   applied to the same plan as defense in depth.
8. **`DropTarget::Table` carries the `TableOrigin`**, set from the row the gesture started on.
   Resolving it inside the dialog would need either a lookup that cannot fail or a default, and a
   default reads "the source files on disk are not deleted" at the one moment that is false.
9. **The dialog's *title* keeps "Drop table" for both origins**; only the body copy reads the
   origin. The action is the same one, the button label and the log entry share that verb with the
   other three targets, and what the user needs to know — that the data goes — is what the body
   sentence now says in the engine's own words.

Then two more, settled in review:

10. **The data is discarded by rename** (`ddl::tables::discard`), the mirror of the spool's
    publish-by-rename. Deleting in place is interruptible at every step, and what an interruption
    leaves is a half-emptied directory under a live table name that nothing collects — the def is
    already gone and `tidy_strata_dir` sweeps only `.tmp-…`. The rename is the operation; the
    removal that follows is housekeeping, logged rather than returned.
11. **A drop is background work.** One `INSERT` is one file with no compaction, so a heavily
    written table is thousands of files and the delete is not instant. `Engine::drop_table` holds
    a `BackgroundGuard` — `Lifecycle::exports` generalized to `background`, since the
    close-while-running flag is its only consumer and it asks one question — so a window closing
    over a running delete asks first. The confirm's copy grew a third arm with it (`whose_work` →
    `Mine`/`Agent`/`Background`): "Queries are running" was already inaccurate for a profile scan
    and an export, and a table delete made it plainly wrong.

12. **`RescanTable` re-reads the table's facts rather than re-registering it.** The draft said
    "`StoreEffect::RescanTable` so `TableMeta.rows` refreshes through the scan driver", and that
    is what shipped first — but the scan driver re-*registers*, which replaces the provider and
    strands the `Arc` every view above it captured (D10/D11), which is the only reason a table
    Refresh re-creates them. An append cannot do that: the sink schema-checks first, so the shape
    a view captured is still there, and the provider re-LISTs per scan so it finds the new file
    unaided. The fold is now `refresh_table_rows` → `Engine::table_meta` → `table_registered`:
    no re-inference, no view churn, no epoch bump, no `Loading` flash, and the count still read
    from the footers. A table **Refresh** keeps the full pass — it may genuinely move the schema.
13. **The runtime's per-file statistics cache is handed to the table** (`ListingTable::with_cache`
    in `register_external`). Not ED-05's by rights, but ED-05 is what made it bite: statistics are
    collected per scan *and* per registration, and an `INSERT` asked for a re-scan, so the *k*th
    write re-read *k* footers. `SessionContext::register_listing_table` does this for itself, so
    snapshots always had it and only our hand-built config did not — the second default the
    convenience constructor applies that we had to re-apply by hand, after `collect_stat`.

Landed with it: the invariant amendments (AGENTS.md §2 + `reference/INVARIANTS.md` +
`reference/ENGINE.md`), and both statements documented as built in `STATEMENTS_SPEC.md` — out of
its *Not yet implemented* list and into a section of their own, per this workstream's own rule.
(The spec's old amendment table went with the docs rebuild that made the file a record of the code
as built rather than a plan for it.)

---

## Goal

The two write statements over ED-04's tables. INSERT executes on stock DataFusion machinery
behind an origin gate; DROP TABLE works on both origins from the editor — internal deletes its
data, external removes only the def — with dependents named in the report. The built substrate
these ride on is documented in `docs/STATEMENTS_SPEC.md` §6.1.

## Current state

- ED-04 established `.strata/tables/<slug>/`, the engine internal-name set, and the
  `TableUpserted`/`TableRemoved` folds. Concretely, and **already built** — do not re-derive:
  - `Engine::is_internal(name)` is the gate INSERT and DROP ask. It is maintained by
    `Engine::note_origin`, called from `Engine::register` (from `TableSpec.internal`),
    `Engine::deregister`, and `Engine::run`'s intercept arm (from the effect's `def.origin`).
  - `ddl::tables::slug` maps a folded table name to its directory name, and
    `project::tables_dir(root)` is the absolute root. **DROP's data deletion needs both**, so
    give `tables` a `pub(super) fn table_dir(root, name)` composing them rather than a second
    copy of the layout — ED-04 deliberately did not add one, having no reader for it.
  - `ddl::DataRoot` is already threaded into `ddl::execute`, so DROP has the project folder
    without a new parameter.
  - `TableDef.origin` is what the sidebar's drop must branch on for its wording and its data
    deletion; the row already renders the distinction (`INTERNAL` badge, `entry.rs`).
- Verified (workstream README, DataFusion 54 facts): `ListingTable::insert_into` for Arrow requires a directory-collection URL
  (`listing_url` already emits the trailing slash), schema-checks via
  `logically_equivalent_names_and_types`, appends one LZ4 IPC file, `Append` only.
- The catalog surface's drop confirm (`views/project/dialogs/drop_confirm.rs` — the path in the
  first draft was wrong) keeps owning the pointer gesture; the store's `ViewInfo` deps name
  dependents. **It does not know about origin, and as drafted this task left it that way** — see
  the gap note below, found while building ED-03.

## What to build

**INSERT (`engine/ddl.rs::insert`):**
- Gate the parsed target: not in the internal set → `Blocked::InsertExternal` ("'events' is an
  external table. INSERT targets internal tables"); a view → same class; `INSERT OVERWRITE` →
  `Blocked::InsertOverwrite` (refused before the Arrow sink's `not_impl` would fire).
- Then dispatch the user's own statement text via `ctx.sql` under dml-only `SQLOptions`
  (ddl=false, statements=false). Report the sink count ("Inserted 42 rows into 't'");
  `StoreEffect::RescanTable` so `TableMeta.rows` refreshes through the scan driver.
- One IPC file per INSERT, no compaction — say so in the module doc; DROP + CTAS is the story
  until a compaction task exists.

**DROP TABLE (`engine/ddl.rs::drop_table`):**
- Both origins, no dialog (settled 2026-08-04). The target resolves against the store first —
  an unknown name errors, `IF EXISTS` no-ops, and nothing ever calls `ctx.deregister_table` for
  a name with no def (a `__snap_` target was already refused at the router — spec §4 reserved
  names). Then: `cancel_profile` → `ctx.deregister_table` → internal only: delete
  `.strata/tables/<slug>/` → `StoreEffect::TableRemoved { name, dependents }`.
- Wording distinguishes origins: internal "'t' and its data were deleted"; external "'x' removed
  from the catalog. Source files were not deleted". Dependent views named in the report (from the
  store fold, which owns `ViewInfo`); no cascade — they go `Reg::Failed` on the epoch bump's
  revalidation, honestly (a `ViewTable`'s inlined plan keeps executing until reload — D11).
- `IF EXISTS` honored; deregister-first so a racing scan fails as cleanly as a retired snapshot;
  snapshots themselves unaffected (materialized copies).
- Update the drop-routing invariant text (AGENTS.md §2 + `docs/reference/INVARIANTS.md`) in this
  change — DROP TABLE works on both origins from the editor; the catalog confirm remains for the
  pointer gesture — and move INSERT/DROP out of `docs/STATEMENTS_SPEC.md`'s *Not yet implemented* list, documenting the
  built behaviour there.

**The sidebar drop is the same destructive action, so it goes through the same funnel** (gap found
while building ED-03; settled with Alex 2026-08-08). As this task was first drafted, the editor's
`DROP TABLE` deleted `.strata/tables/<slug>/` and the sidebar's drop did not — two implementations
of one gesture, and the divergence is silent data left on disk:

- `dialogs/drop_confirm.rs`'s `DropTarget::Table` arm does `remove_table` → `persisted_defs` →
  `engine.deregister(name)` and nothing else. On an internal table that **orphans the data
  directory forever**: no def points at it, and `tidy_strata_dir` only sweeps `.tmp-*`.
- Its body copy pins "files on disk are not deleted"
  (`drop_confirm.rs`'s `a_drop_with_no_dependents_shows_no_consequence_line`). True for every
  table today, **false** for an internal one — the dialog would be reassuring the user at exactly
  the moment the action is destructive.

So: the apply path calls `engine::ddl::drop_table`, with the existing dialog as the confirm in
front of it — AGENTS.md §2's "one entry point per expensive action, with the confirm in front of
it", which the editor's no-dialog path then rides behind. The dialog's copy and title read the
def's origin: internal names the data ("'t' and its data will be deleted"), external keeps today's
sentence. The two wordings are the report's, so state them once and let both surfaces render it
rather than writing a second vocabulary. **Do not** add directory deletion to the store-first path
as a second implementation.

The store-first ordering the dialog has today (write the def, roll back if the persist failed,
*then* touch the engine) is deliberate and must survive the refactor — a drop the project file
never heard about comes back on the next open.

## Acceptance

- INSERT into an internal table appends a file; the sidebar row count refreshes via the rescan;
  a second INSERT reads back the union. INSERT into an external table / a view / with OVERWRITE
  refuses with the exact messages, nothing written.
- Schema mismatch surfaces DataFusion's own check as the run error.
- DROP internal removes row, def, and directory (verified on disk); DROP external removes row
  and def, source files untouched; both name dependent views when any exist; `IF EXISTS` on a
  missing name reports a no-op.
- **The sidebar drop and the editor `DROP TABLE` leave the same state**, asserted on disk for an
  internal table — the point of the shared funnel, and the one thing a per-surface implementation
  would pass tests without delivering.
- The confirm dialog names the data for an internal table and keeps "files on disk are not
  deleted" for an external one; the existing no-dependents test moves with the wording rather
  than being deleted.
- A failed persist still rolls the def back and leaves the directory alone.
- Restart after each: the store and disk agree (no orphan def, no orphan directory).

## Verification

`cargo test -p strata-core` (pinned integration test: create → insert twice → two files → exact
rows back → drop → directory gone); run the app for the sidebar/rescan half.
