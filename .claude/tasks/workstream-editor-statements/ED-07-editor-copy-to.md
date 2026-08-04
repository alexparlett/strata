# ED-07 · Editor COPY TO: pre-flight NULL gate + native dispatch

**Workstream:** Editor statements · **Status:** ⬜ · **DEV_TASKS:** E5 · **Depends on:** ED-02

## Goal

Typed `COPY … TO` runs from the editor — natively, behind the two checks only the managed
surface used to provide: bare-word partition identifiers and the NULL-partition corruption gate.
The Export window is unchanged and remains the snapshot-backed, race-free path.
`docs/STATEMENTS_SPEC.md` §6.3.

## Current state

- `Engine::export` (`engine/mod.rs:938`) + `export::run_export` (`engine/export.rs:218`) render
  COPY against a pinned snapshot; the NULL gate `partition_columns_have_no_nulls`
  (`export.rs:411`) reads exact counts from the spool's `SnapshotStats` — proceed only on exact
  zero (DF 54 misfiles NULL partition values; schema nullability is no signal, DF reports
  everything nullable). Bare-word check: `is_bare_word` (`export.rs:438`) — DF 54's COPY parser
  re-renders quoted idents broken.
- Known wart to fix here: `run_export` sets `datafusion.execution.keep_partition_by_columns` per
  export (`export.rs:255`) and never restores it — invisible today, observable once SET and
  `df_settings` are real (ED-08).

## What to build

`engine/ddl.rs::copy_to`:

1. From the parsed `CopyToStatement`: partition idents through the export module's bare-word
   check and wording (shared, not copied); a `__snap_`-prefixed source reference →
   `Blocked::ReservedName` (a typed `COPY (SELECT * FROM __snap_3)` must never write
   `__strata_ord`).
2. **Pre-flight NULL gate** when `PARTITIONED BY` is non-empty: run
   `SELECT count(*) FILTER (WHERE "p" IS NULL) AS n_p, …` over the statement's source; refuse on
   any non-zero with `partition_columns_have_no_nulls`' wording. One extra scan per partitioned
   typed COPY — the honest price of generality; the Export window keeps its free counts.
3. Dispatch the user's statement text via `ctx.sql` (dml-only options); report
   "Exported N rows to '<path>'" from the sink's count column. `StoreEffect::None` — a COPY
   changes no catalog state; history and the event log still record it (ED-02).
4. Fix the `keep_partition_by_columns` wart in `run_export`: save/restore around the COPY (or
   route through ED-08's overlay if it lands first — coordinate, don't duplicate).
5. Update the COPY invariant text + `Blocked::CopyTo` message per spec §10.

## Acceptance

- Unpartitioned COPY to CSV/parquet/JSON/Arrow writes the file(s) and reports the row count;
  the written file has no `__strata_ord` column even when the source query selects from a
  snapshot-backed table indirectly (reserved-name refusal test).
- Partitioned COPY with a NULL in a partition column refuses, names the column, writes nothing;
  with exact-zero NULLs it writes the partition directories.
- A quoted partition identifier refuses with the bare-word message.
- After any export (window or typed), `SHOW VARIABLES`-visible
  `keep_partition_by_columns` equals what it was before the export.
- The Export window's tests are untouched and green.

## Verification

`cargo test -p strata-core`; run the app: type a partitioned COPY over a fixture with a NULL
partition value (refused), fix the data, re-run (files on disk inspected).
