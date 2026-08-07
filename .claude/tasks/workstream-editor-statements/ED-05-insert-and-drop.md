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
- The catalog surface's drop confirm (`views/sidebar/catalog/drop_confirm.rs`) keeps owning the
  pointer gesture; the store's `ViewInfo` deps name dependents.

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

## Acceptance

- INSERT into an internal table appends a file; the sidebar row count refreshes via the rescan;
  a second INSERT reads back the union. INSERT into an external table / a view / with OVERWRITE
  refuses with the exact messages, nothing written.
- Schema mismatch surfaces DataFusion's own check as the run error.
- DROP internal removes row, def, and directory (verified on disk); DROP external removes row
  and def, source files untouched; both name dependent views when any exist; `IF EXISTS` on a
  missing name reports a no-op.
- Restart after each: the store and disk agree (no orphan def, no orphan directory).

## Verification

`cargo test -p strata-core` (pinned integration test: create → insert twice → two files → exact
rows back → drop → directory gone); run the app for the sidebar/rescan half.
