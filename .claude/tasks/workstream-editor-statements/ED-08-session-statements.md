# ED-08 · Session statements: SET/RESET overlay · PREPARE/EXECUTE/DEALLOCATE

**Workstream:** Editor statements · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** ED-02

## As built

`engine/ddl/session.rs`, documented in `docs/STATEMENTS_SPEC.md` §6.5 and pinned by
`AGENTS.md` §2 / `docs/reference/INVARIANTS.md`. Three corrections to the plan below, all settled
while building:

- **`Blocked::SetRuntime` is reworded, not interpolated.** The draft's message named the key
  (`"'datafusion.runtime.memory_limit' requires a restart…"`), which `Blocked::editor_message`
  cannot do — it takes no argument, and the rule is that a refusal's wording has one home. The
  variant now reads "Engine runtime options require a restart. Set them in Settings", which carries
  the same information without a second place to spell it.
- **`PREPARE`/`DEALLOCATE` carry `StoreEffect::PreparedChanged`, not `None`.** Nothing persists,
  but `EXECUTE p` resolves now and did not a moment ago — the exact argument `FunctionsChanged`
  already makes — so the catalog epoch has to move, or the language-service snapshot and every
  tab's diagnostics keep an answer the engine has stopped giving. The editor tab's completion
  catalog now reads that epoch, which is also what rebuilds it.
- **There is a fourth refused key class: `datafusion.sql_parser.dialect`
  (`Blocked::SetDialect`).** Found in review. The draft's three classes missed it, and the miss
  was silent rather than loud: the language service carries the dialect on its own `Catalog`
  snapshot, built from the **Settings** store, while the validator and the planner read it
  **live** — so `SET datafusion.sql_parser.dialect = 'mysql'` left completion lexing the buffer by
  rules the planner had already stopped using, which is WJ-04 exactly, and nothing would have
  reported it. It is the same rule as `format.*` one surface over, so it is stated as one rule
  with two surfaces rather than as a second mechanism.
- **Writing a `ConfigOptions` key is only half of applying it.** Found by adversarial review, and
  it was a call made deliberately while building and made wrong: the plan said "the `set_config`
  apply path's options-set call", so that is all this did — but DataFusion's own `set_variable`
  makes the *same* call and then re-registers every UDF whose `with_updated_config` answers, and
  `NowFunc` captures `execution.time_zone` at registration and bakes it into the literal its
  `simplify` returns. So `SET datafusion.execution.time_zone` reported success, moved `SHOW`, and
  left `now()` in the zone the engine was built with. `Engine::set_config` had the identical gap,
  which is why the fix is a shared `refresh_config_dependent_udfs` called by all three writers
  rather than a patch on the new arm: "the two ways an option moves cannot land differently" was
  true and worth nothing, because both landed wrong.

Completion offers prepared names at an `EXECUTE` / `DEALLOCATE` operand (`Clause::Execute`) and
nowhere else. The rest of the session statements' completion — statement leads, `SET`/`RESET`
config keys and their values — is **ED-11**, split out rather than folded in here: a dotted config
key is one name the completion layer's `.`-rule reads as a qualified column, and fixing that is a
change to the caret model, not a table entry.

## Goal

Session-scoped statements, with the Settings store untouched as the durable config authority.
The dispatch and report they ride: `docs/STATEMENTS_SPEC.md` §2; the EXECUTE caveat this task
lifts: §1.

## Current state

- Config: one app-global store; disk read once; `write_config` the sole write path; the engine's
  overrides are a launch value + `set_config` live-apply; owned keys fenced
  (`engine/config.rs:443`); `restart_owed` measures `runtime.*` against `built_runtime`.
- Verified (workstream README, DataFusion 54 facts): native SET applies `runtime.*` live (bypassing the restart discipline) and
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
  default when unset). The overlay is engine-wide — all tabs, agent reads included — and gone on
  restart. Reports state the scope: "Set 'k' to 'v' for this session."
- Interactions, documented in the module doc: `set_config` (a Settings Apply) re-applies
  baselines under the overlay's keys? No — settled: **the overlay wins for its keys until RESET
  or restart**, and a `set_config` restart drops the overlay silently. `restart_owed` unchanged
  (runtime keys can't enter the overlay).
- Update the SET/RESET invariant text (AGENTS.md §2 + `docs/reference/INVARIANTS.md`) — a session
  overlay for non-owned, non-runtime, non-format keys; Settings stays the durable authority — and
  move SET/RESET/PREPARE/DEALLOCATE out of `docs/STATEMENTS_SPEC.md`'s *Not yet implemented*
  list, documenting the built
  behaviour (and the session lifetimes in its §8) there. `Blocked::Set`/`Reset` keep their variants
  and words for the agent surface; the `SetOwned`/`SetRuntime`/`SetFormat` messages join them.

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
