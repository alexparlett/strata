# ED-05 · INSERT (native, target-gated) + DROP TABLE (both origins)

**Workstream:** Editor statements · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** ED-04

## Goal

The two write statements over ED-04's tables. INSERT executes on stock DataFusion machinery
behind an origin gate; DROP TABLE works on both origins from the editor — internal deletes its
data, external removes only the def — with dependents named in the report.
`docs/STATEMENTS_SPEC.md` §6.1.

## Current state

- ED-04 established `.strata/tables/<slug>/`, the engine internal-name set, and the
  `TableUpserted`/`TableRemoved` folds.
- Verified (spec §2): `ListingTable::insert_into` for Arrow requires a directory-collection URL
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
  change, per spec §10.

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
