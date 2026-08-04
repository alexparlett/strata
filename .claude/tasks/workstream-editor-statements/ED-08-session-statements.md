# ED-08 · Session statements: SET/RESET overlay · PREPARE/EXECUTE/DEALLOCATE

**Workstream:** Editor statements · **Status:** ⬜ · **DEV_TASKS:** E5 · **Depends on:** ED-02

## Goal

Session-scoped statements, with the Settings store untouched as the durable config authority.
`docs/STATEMENTS_SPEC.md` §6.4 + §6.5.

## Current state

- Config: one app-global store; disk read once; `write_config` the sole write path; the engine's
  overrides are a launch value + `set_config` live-apply; owned keys fenced
  (`engine/config.rs:443`); `restart_owed` measures `runtime.*` against `built_runtime`.
- Verified (spec §2): native SET applies `runtime.*` live (bypassing the restart discipline) and
  native RESET restores DataFusion's default, not the Settings baseline — both are why SET/RESET
  never run natively. PREPARE/EXECUTE/DEALLOCATE are supported; plans in
  `SessionState.prepared_plans` (`pub(crate)` — no enumeration); `verify_plan` cannot see
  through EXECUTE, so DML/DDL fence at PREPARE.

## What to build

**SET/RESET (`engine/ddl.rs` + `Engine::{set_session, reset_session}`):**
- Refusals: owned keys (`is_owned_key`) → `Blocked::SetOwned`; `datafusion.runtime.*` →
  `Blocked::SetRuntime` ("'datafusion.runtime.memory_limit' requires a restart. Set it in
  Settings"); `datafusion.format.*` → `Blocked::SetFormat` (display keys drive the grid
  formatter and chart-read cache identity from the Settings store — a session value would
  split-brain them).
- Otherwise: apply to the live ctx (the `set_config` apply path's options-set call) and record
  in `session_overlay: Mutex<BTreeMap<String, String>>` on `Engine`. `RESET k` removes the
  overlay entry and re-applies the **Settings baseline** from `Engine::overrides` (or the DF
  default when unset). Reports state the scope: "Set 'k' to 'v' for this session."
- Interactions, documented in the module doc: `set_config` (a Settings Apply) re-applies
  baselines under the overlay's keys? No — settled: **the overlay wins for its keys until RESET
  or restart**, and a `set_config` restart drops the overlay silently. `restart_owed` unchanged
  (runtime keys can't enter the overlay).
- Update the SET/RESET invariant text + messages per spec §10.

**PREPARE/EXECUTE/DEALLOCATE:**
- PREPARE: verify the parsed statement's inner statement is a query (`Blocked::PrepareNonQuery`:
  "PREPARE supports queries"), then dispatch natively under statements-only options; keep an
  engine-side mirror `prepared: Mutex<BTreeMap<String, Vec<DataType>>>` feeding completion
  (DF's map is unreachable). Duplicate names keep DF's own error.
- EXECUTE classifies as `Verdict::Query` (ED-01) and rides the full snapshot pipeline; the only
  `materialize` change is a per-kind `SQLOptions` (statements=true for EXECUTE alone —
  ddl/dml stay false, safe because PREPARE gated the inner plan).
- DEALLOCATE: native + report + mirror removal.

## Acceptance

- `SET datafusion.execution.batch_size = 1024` applies live (visible in `df_settings`), reports
  session scope; `RESET` restores the Settings-store value when one exists, the DF default
  otherwise; owned/runtime/format keys refuse with their exact messages and change nothing.
- `PREPARE p AS SELECT $1 + 1` then `EXECUTE p(41)` produces an ordinary snapshot-backed result
  (pages, sorts, exports); `PREPARE bad AS INSERT …` refuses at PREPARE; `DEALLOCATE p` then
  `EXECUTE p(1)` fails with DF's own missing-name error.
- Completion offers prepared names after PREPARE and drops them after DEALLOCATE.
- Engine restart (ProjectRoot remount) clears overlay, prepared statements, and mirror — asserted.

## Verification

`cargo test -p strata-core`; run the app for the completion half and `df_settings` reads.
