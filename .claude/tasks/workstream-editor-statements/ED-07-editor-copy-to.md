# ED-07 · Editor COPY TO: pre-flight NULL gate + native dispatch

**Workstream:** Editor statements · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** ED-02

## Goal

Typed `COPY … TO` runs from the editor — natively, behind the two checks only the managed
surface used to provide: bare-word partition identifiers and the NULL-partition corruption gate.
The Export window is unchanged and remains the snapshot-backed, race-free path. The dispatch and
report it rides: `docs/STATEMENTS_SPEC.md` §2.

## What was built

`engine/ddl/copy.rs::copy_to`, reached from `ddl::execute`'s `StmtKind::Copy` arm. Documented as
built in `docs/STATEMENTS_SPEC.md` §6.4, with the invariant in AGENTS.md §2 +
`docs/reference/INVARIANTS.md`.

1. **Bare-word partition idents**, before planning, through `export::partition_columns_are_bare_words`
   — made `pub(super)` and shared, not copied. Asked of the strings `CopyToStatement::partitioned_by`
   holds, which are `Ident::to_string()`'s output, so a quoted `PARTITIONED BY ("region")` arrives
   *with its quotes* and is refused in the Export window's own words. That message's bad-name
   rendering moved from `{bad:?}` to `'{bad}'` in the same change: `Debug` on an
   already-quoted string prints escaped Rust at the user, and single quotes are the house
   convention for identifiers (AGENTS.md §3). Only `single plain word` was pinned by a test.
2. **Pre-flight NULL gate** (`no_null_partition_values`) when `partition_by` is non-empty:
   `count_all()` plus one `count(col)` per partition column over the **planned input**, decoded
   positionally, nulls derived as `rows - non_null` — the shape `profile::aggregates` already uses,
   which sidesteps the fallible `ExprFunctionExt` FILTER builder. Refuses on anything but an exact
   zero, in `export::partition_null_refusal`'s wording (extracted so both surfaces state one fact
   once). One extra scan per partitioned typed COPY.
3. **Dispatch drives the planned `LogicalPlan::Copy`**, not a re-parse of the text — see the
   deviation below. Report is `Exported N rows to '<path>'` off the sink's `count` column;
   `effect: None`.
4. **The `keep_partition_by_columns` wart is gone**, and not by save/restore: `run_export` now
   sends `'execution.keep_partition_by_columns' '<bool>'` in the COPY's own `OPTIONS`. DataFusion's
   physical planner reads that key out of the statement's options and only falls back to the
   session config when it is absent (`physical_planner.rs`, the `Copy` arm), and
   `TableOptions::set` skips the whole `execution.` namespace, so the key reaches the planner
   without a format refusing it as unknown. Nothing needs coordinating with ED-08: the session is
   never written in the first place. `format_options` split into `format_pairs` + `options_clause`
   so the partition option can join the format's.
5. A `__snap_` source needed no code — the router already refuses it (`Blocked::ReservedName`,
   `names_reserved`'s `CopyTo` arm). `Blocked::CopyTo` and its message stay verbatim as the agent
   path's refusal.

## Deviations from the original plan

- **The plan is driven, not the text.** The task said "dispatch the user's statement text via
  `ctx.sql`". The statement is planned anyway for the NULL gate, so re-parsing would gate one
  value and execute another — the exact failure the `INSERT` arm's "the plan that was gated is the
  plan that runs" exists to prevent. Driving the plan *is* `ctx.sql` minus the re-parse:
  `execute_logical_plan` special-cases `Ddl` and `Statement` and hands `LogicalPlan::Copy` to
  `DataFrame::new`. The dml-only `SQLOptions` floor is still applied, as defense in depth.
- **The count is built with the DataFrame API, not a rendered `SELECT count(*) FILTER (…)`.**
  Internal logic does not write SQL (the `profile` rule), and counting over the planned input means
  the thing measured is the thing that will be written.
- **The keep-columns fix is an option, not a save/restore** (point 4 above).

## Known and stated

- The gate is a **pre-flight, not a lock**. The Export window writes an immutable snapshot; a typed
  COPY reads live tables, and a partitioned one reads them twice. Said plainly in the module doc
  rather than papered over.
- No destination gate: a typed COPY may write anywhere the user can, exactly as the Export window
  may.

## Verification

`cargo test -p strata-core` — `engine::ddl::copy`'s seven tests (every format flat; reserved-name
refusal writing nothing; NULL refusal then the same statement over a filtered source; the gate
ignoring non-partition columns; the quoted-ident message asserted whole; a column named twice in
`PARTITIONED BY`; the engine's options unmoved by a partitioned COPY) plus
`tests/engine_export.rs`, whose keep-columns test now also reads
`SHOW datafusion.execution.keep_partition_by_columns` back and asserts the export left it alone.

**One defect found and fixed in review.** The gate built one `count` per *entry* of `partition_by`,
so `PARTITIONED BY (region, region)` — which DataFusion plans without complaint — died in the
pre-flight aggregate's own schema construction ("duplicate unqualified field name
`count(t.region)`"), refusing a statement that would have run and naming a query the user never
wrote. It now counts once per distinct name, with the case pinned by
`a_column_partitioned_by_twice_is_counted_once`.
