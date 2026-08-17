//! **Session statements** (ED-08) — `SET` / `RESET` over a session overlay, and
//! `PREPARE` / `DEALLOCATE` over DataFusion's own prepared-plan store.
//! `docs/STATEMENTS_SPEC.md` §6.5.
//!
//! Everything here dies with the engine, and every report says so, because the report is where the
//! user learns a statement's scope.
//!
//! **`SET` and `RESET` never run natively**, so Settings stays the durable config authority: native
//! `SET` applies `datafusion.runtime.*` **live**, bypassing the `restart_owed` discipline, and
//! native `RESET` restores *DataFusion's* default rather than the Settings one — so a user who set
//! `batch_size` in Settings, typed `SET`, then typed `RESET` would land on 8192 with their own
//! setting silently gone.
//!
//! A `SET` therefore goes through the same `ConfigOptions::set` call `Engine::set_config` uses and
//! is recorded in [`SessionScope`]'s overlay; a `RESET` drops the entry and re-applies the Settings
//! baseline. The overlay is engine-wide and wins for its keys until a `RESET` or a restart — a
//! Settings Apply over an overlaid key records the baseline and leaves the live value alone, which
//! makes "the last thing you typed is in force" true with no precedence table.
//!
//! Four key classes are refused rather than overlaid, each toward the surface that owns it:
//! `is_owned_key`, `datafusion.runtime.*` (a restart), and the two the app reads from the *Settings
//! store* rather than the session — `datafusion.format.*` and `datafusion.sql_parser.dialect`,
//! where a session value would leave two layers answering differently about one buffer.
//!
//! **`PREPARE` and `DEALLOCATE` do run natively**, because DataFusion owns the prepared plan. Ours
//! are the fence and the mirror. The fence is `PREPARE`'s: `verify_plan` descends into a `Prepare`
//! node's input and an `Execute` has none, so a DML/DDL body is refused at `PREPARE` or never. The
//! mirror exists because `prepared_plans` is `pub(crate)` and completion has to offer the names.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use datafusion::arrow::datatypes::DataType;
use datafusion::config::ConfigField;
use datafusion::logical_expr::{LogicalPlan, Statement};
use datafusion::prelude::{SQLOptions, SessionContext};
use datafusion::sql::parser::Statement as DFStatement;

use crate::catalog::short_type;
use crate::refresh_config_dependent_udfs;
use crate::sql::{Blocked, PreparedSym, StmtKind};
use strata_arrow::config::{effective, is_display_key, is_owned_key, is_restart_key, DIALECT_KEY};

use super::{StatementOutcome, StoreEffect};

/// The engine state a **session statement** moves: the `SET` overlay and the prepared-statement
/// mirror.
///
/// **Shared by handle** for the reason `InternalTables` is: these arms run inside the task
/// `Engine::bookkeep` spawned, and that task must not hold the engine — the engine's `Drop` is
/// what aborts it. Both maps hold values only, so they outlive an engine harmlessly, and a fresh
/// engine gets a fresh [`Default`], which is what makes "a restart clears the session" true by
/// construction rather than by a teardown step somebody has to remember.
#[derive(Clone, Debug, Default)]
pub struct SessionScope {
    /// `SET` keys and their session values — the keys the overlay owns until `RESET` or restart.
    /// Engine-wide, never per tab: they are applied to the one `SessionState` everything plans
    /// against, so a per-tab overlay would be a claim the engine cannot keep.
    overlay: Arc<Mutex<BTreeMap<String, String>>>,
    /// Prepared statement names and the parameter types DataFusion resolved for each — a
    /// **mirror** of `SessionState::prepared_plans`, which is `pub(crate)`. Written only after
    /// DataFusion accepted the statement, so a duplicate name keeps DataFusion's own error and
    /// the mirror cannot claim a plan the session does not hold.
    prepared: Arc<Mutex<BTreeMap<String, Vec<DataType>>>>,
}

impl SessionScope {
    /// Whether the overlay holds `key` — asked by `Engine::set_config`, which must not re-apply a
    /// baseline underneath a session value.
    pub fn overlaid(&self, key: &str) -> bool {
        self.overlay.lock().unwrap().contains_key(key.trim())
    }

    /// The prepared statements this session holds, as the language service's symbols: name plus
    /// each parameter's type in the same vocabulary a column's dtype uses (`short_type`), so the
    /// UI never depends on DataFusion's types.
    pub fn prepared(&self) -> Vec<PreparedSym> {
        self.prepared
            .lock()
            .unwrap()
            .iter()
            .map(|(name, params)| PreparedSym {
                name: name.clone(),
                params: params.iter().map(short_type).collect(),
            })
            .collect()
    }
}

/// `SET key = value` — apply it to the live session and record it in the overlay.
pub async fn set(
    ctx: &SessionContext,
    stmt: DFStatement,
    scope: &SessionScope,
) -> Result<StatementOutcome, String> {
    let plan = ctx
        .state()
        .statement_to_plan(stmt)
        .await
        .map_err(|e| e.to_string())?;
    let LogicalPlan::Statement(Statement::SetVariable(set)) = plan else {
        return Err(format!("{} did not plan as a set", StmtKind::Set.label()));
    };
    refuse_reserved_key(&set.variable)?;

    {
        let state = ctx.state_ref();
        let mut state = state.write();
        state
            .config_mut()
            .options_mut()
            .set(&set.variable, &set.value)
            .map_err(|e| e.to_string())?;
        refresh_config_dependent_udfs(&mut state);
    }
    scope
        .overlay
        .lock()
        .unwrap()
        .insert(set.variable.clone(), set.value.clone());

    Ok(StatementOutcome {
        message: format!("Set '{}' to '{}' for this session", set.variable, set.value),
        count: None,
        effect: None,
    })
}

/// `RESET key` — drop the overlay entry and put the key back to the Settings baseline.
///
/// `baseline` is the engine's `datafusion.*` overrides, cloned at dispatch: the `RESET` runs
/// inside that dispatch, so the snapshot *is* the current baseline, and a clone is what keeps this
/// arm reachable from a task that may not hold the engine. `config::effective` is the same
/// resolution Settings itself displays — the override when the user named one, otherwise the
/// `ENGINE_KEYS` default the engine was built with. A key neither names is DataFusion's own
/// `ConfigOptions::reset`, which is the correct answer for exactly the keys where DataFusion's
/// default *is* the baseline.
pub async fn reset(
    ctx: &SessionContext,
    stmt: DFStatement,
    scope: &SessionScope,
    baseline: &BTreeMap<String, String>,
) -> Result<StatementOutcome, String> {
    let plan = ctx
        .state()
        .statement_to_plan(stmt)
        .await
        .map_err(|e| e.to_string())?;
    let LogicalPlan::Statement(Statement::ResetVariable(reset)) = plan else {
        return Err(format!(
            "{} did not plan as a reset",
            StmtKind::Reset.label()
        ));
    };
    refuse_reserved_key(&reset.variable)?;

    let restored = effective(baseline, &reset.variable);
    {
        let state = ctx.state_ref();
        let mut state = state.write();
        let options = state.config_mut().options_mut();
        match &restored {
            Some(value) => options.set(&reset.variable, value),
            None => options.reset(&reset.variable),
        }
        .map_err(|e| e.to_string())?;
        refresh_config_dependent_udfs(&mut state);
    }
    scope.overlay.lock().unwrap().remove(&reset.variable);

    Ok(StatementOutcome {
        message: match &restored {
            Some(value) => format!("Reset '{}' to '{value}'", reset.variable),
            None => format!("Reset '{}' to its default", reset.variable),
        },
        count: None,
        effect: None,
    })
}

/// The key classes an overlay may not hold, each refused toward the surface that owns it.
/// Shared by `SET` and `RESET`: a native `RESET` of a runtime key rebuilds the `RuntimeEnv` just
/// as a native `SET` does, and a key Strata owns is not the user's to put back either.
///
/// The last two are **one rule with two surfaces**: a key some part of the app reads from the
/// *Settings store* rather than from the session cannot have a session value, or the two answer
/// differently about the same buffer. `datafusion.format.*` is the grid formatter and the chart
/// read's cache identity; `datafusion.sql_parser.dialect` is the language service, which carries
/// it on its own `Catalog` snapshot because a completion pass reached from a keystroke has no
/// engine to ask — while the planner and the validator read it live. A session value there leaves
/// the editor lexing the buffer by rules the planner has stopped using, which is WJ-04 exactly.
///
/// `pub(crate)` because the `SET` key **pool** calls it to filter `config::ENGINE_KEYS`
/// (ED-11) — zero drift by construction, and the fourth class is the reason a filter written
/// from the three predicates alone would not do: `DIALECT_KEY` is a plain
/// `datafusion.sql_parser.*` key with no predicate of its own.
pub(crate) fn refuse_reserved_key(key: &str) -> Result<(), String> {
    if is_owned_key(key) {
        return Err(Blocked::SetOwned.editor_message());
    }
    if is_restart_key(key) {
        return Err(Blocked::SetRuntime.editor_message());
    }
    if is_display_key(key) {
        return Err(Blocked::SetFormat.editor_message());
    }
    if key.trim() == DIALECT_KEY {
        return Err(Blocked::SetDialect.editor_message());
    }
    Ok(())
}

/// `PREPARE name [(types)] AS <query>` — dispatched natively, mirrored for completion.
pub async fn prepare(
    ctx: &SessionContext,
    stmt: DFStatement,
    scope: &SessionScope,
) -> Result<StatementOutcome, String> {
    let plan = ctx
        .state()
        .statement_to_plan(stmt)
        .await
        .map_err(|e| e.to_string())?;
    let LogicalPlan::Statement(Statement::Prepare(prepare)) = &plan else {
        return Err(format!(
            "{} did not plan as a prepare",
            StmtKind::Prepare.label()
        ));
    };
    let name = prepare.name.clone();
    let params: Vec<DataType> = prepare
        .fields
        .iter()
        .map(|f| f.data_type().clone())
        .collect();

    statements_only()
        .verify_plan(&plan)
        .map_err(|e| e.to_string())?;

    ctx.execute_logical_plan(plan)
        .await
        .map_err(|e| e.to_string())?;
    scope.prepared.lock().unwrap().insert(name.clone(), params);

    Ok(StatementOutcome {
        message: format!("Prepared '{name}' for this session"),
        count: None,
        effect: Some(StoreEffect::PreparedChanged),
    })
}

/// `DEALLOCATE name` — dispatched natively, mirrored.
pub async fn deallocate(
    ctx: &SessionContext,
    stmt: DFStatement,
    scope: &SessionScope,
) -> Result<StatementOutcome, String> {
    let plan = ctx
        .state()
        .statement_to_plan(stmt)
        .await
        .map_err(|e| e.to_string())?;
    let LogicalPlan::Statement(Statement::Deallocate(deallocate)) = &plan else {
        return Err(format!(
            "{} did not plan as a deallocate",
            StmtKind::Deallocate.label()
        ));
    };
    let name = deallocate.name.clone();
    statements_only()
        .verify_plan(&plan)
        .map_err(|e| e.to_string())?;

    ctx.execute_logical_plan(plan)
        .await
        .map_err(|e| e.to_string())?;
    scope.prepared.lock().unwrap().remove(&name);

    Ok(StatementOutcome {
        message: format!("Deallocated '{name}'"),
        count: None,
        effect: Some(StoreEffect::PreparedChanged),
    })
}

/// The options a prepared-statement dispatch plans under: the read path's triple with statements
/// allowed, because the node being driven *is* a `LogicalPlan::Statement`. DDL and DML stay
/// refused, which for `PREPARE` is the fence over the prepared query itself.
fn statements_only() -> SQLOptions {
    SQLOptions::new()
        .with_allow_dml(false)
        .with_allow_ddl(false)
        .with_allow_statements(true)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use crate::sql::{complete, Blocked, Catalog, CompletionKind};
    use crate::{Engine, RunOutcome, RunTag, StatementReport, StoreEffect, WsId, CATALOG, SCHEMA};

    /// An engine whose Settings baseline is `overrides` — a `(key, value)` list, because that is
    /// how a Settings row reads.
    fn engine(overrides: &[(&str, &str)]) -> Arc<Engine> {
        Engine::builder()
            .with_config(
                overrides
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect::<BTreeMap<_, _>>(),
            )
            .build()
    }

    /// Run one statement and take its report — anything else is a test asking the wrong question.
    async fn statement(eng: &Engine, sql: &str) -> Result<StatementReport, String> {
        match eng.run(WsId(1), RunTag(1), sql.into(), 10).await? {
            RunOutcome::Statement(report) => Ok(report),
            RunOutcome::Rows(..) => panic!("{sql} ran as a query"),
        }
    }

    /// The error a Run fails with. `RunOutcome` carries a `RecordBatch` and derives no `Debug`,
    /// so the success arm is named rather than unwrapped.
    async fn run_err(eng: &Engine, sql: &str) -> String {
        match eng.run(WsId(1), RunTag(9), sql.into(), 10).await {
            Err(e) => e,
            Ok(_) => panic!("{sql} succeeded"),
        }
    }

    /// What the **session** says a key is now — `SHOW`, which reads the live `ConfigOptions` the
    /// planner reads, not anything this module remembers.
    async fn live(eng: &Engine, key: &str) -> String {
        let RunOutcome::Rows(output, _) = eng
            .run(WsId(2), RunTag(2), format!("SHOW {key}"), 10)
            .await
            .expect("show")
        else {
            panic!("SHOW did not return rows");
        };
        output
            .rows
            .first()
            .and_then(|row| row.last())
            .map(|cell| cell.text.clone())
            .unwrap_or_else(|| panic!("{key} is not set"))
    }

    /// **A `SET` applies to the live session and says what it did — for this session.** The scope
    /// is in the sentence because the report is the only place the user learns it (spec §8).
    #[tokio::test]
    async fn a_set_applies_live_and_reports_its_scope() {
        let eng = engine(&[]);
        let report = statement(&eng, "SET datafusion.execution.batch_size = 1024")
            .await
            .expect("set");
        assert_eq!(
            report.message,
            "Set 'datafusion.execution.batch_size' to '1024' for this session"
        );
        assert_eq!(report.count, None, "a SET moves no rows");
        assert_eq!(report.effect, None, "and changes nothing the catalog holds");
        assert_eq!(live(&eng, "datafusion.execution.batch_size").await, "1024");
    }

    /// **Applying an option is not just writing it.** `NowFunc` captures `execution.time_zone`
    /// when it is *registered* and bakes it into the literal its `simplify` returns, so a write
    /// without DataFusion's own UDF refresh moves `SHOW` and leaves `now()` answering in the zone
    /// the engine was built with — success reported, nothing changed, until a restart.
    ///
    /// All three writers are checked, because the failure is a writer forgetting the second half:
    /// a typed `SET`, a typed `RESET`, and a Settings Apply. (`Engine::set_config` had the same
    /// gap before this task and it is fixed with them — otherwise "the two ways an option moves
    /// cannot land differently" would be true only because both were wrong.)
    #[tokio::test]
    async fn moving_an_option_re_registers_the_functions_that_captured_it() {
        /// The Arrow type `now()` reports — which carries the zone the UDF captured when it was
        /// registered, and so is the only thing that can tell a written option from an applied one.
        async fn zone_of(eng: &Engine) -> String {
            let RunOutcome::Rows(output, _) = eng
                .run(WsId(3), RunTag(3), "SELECT arrow_typeof(now())".into(), 1)
                .await
                .expect("now()")
            else {
                panic!("not rows");
            };
            output.rows[0][0].text.clone()
        }

        let eng = engine(&[]);
        assert_eq!(zone_of(&eng).await, "Timestamp(ns)", "the built-with zone");

        statement(&eng, "SET datafusion.execution.time_zone = '+05:00'")
            .await
            .expect("set");
        assert_eq!(zone_of(&eng).await, "Timestamp(ns, \"+05:00\")");

        statement(&eng, "RESET datafusion.execution.time_zone")
            .await
            .expect("reset");
        assert_eq!(zone_of(&eng).await, "Timestamp(ns, \"+00:00\")");

        eng.set_config(
            [(
                "datafusion.execution.time_zone".to_string(),
                "+09:00".to_string(),
            )]
            .into_iter()
            .collect(),
        );
        assert_eq!(zone_of(&eng).await, "Timestamp(ns, \"+09:00\")");
    }

    /// **A typed `EXPLAIN EXECUTE` needs the same widening the run of one does.** `verify_plan`
    /// visits the whole tree, so `Explain { Statement(Execute) }` is refused at its child — and
    /// the router draws no squiggle on the form, so without [`read_policy`] unwrapping the
    /// `EXPLAIN` the user gets DataFusion's internal "Statement not supported: Execute" from a
    /// statement the editor accepted.
    ///
    /// The Explain **gesture** is a different path and is asserted to still refuse, because that
    /// is the honest end-state rather than an oversight: it unwraps to the explained plan and asks
    /// for a *physical* one, and a `Statement(Execute)` has none — the bound plan exists only
    /// inside DataFusion's `execute_prepared`. Widening its options would move the failure one
    /// step, not remove it, so the widening is not there.
    #[tokio::test]
    async fn a_typed_explain_of_a_prepared_statement_runs() {
        let eng = engine(&[]);
        statement(&eng, "PREPARE p(INT) AS SELECT $1 + 1 AS n")
            .await
            .expect("prepared");

        let RunOutcome::Rows(output, _) = eng
            .run(WsId(1), RunTag(2), "EXPLAIN EXECUTE p(1)".into(), 10)
            .await
            .expect("explained")
        else {
            panic!("EXPLAIN did not return rows");
        };
        assert!(!output.rows.is_empty(), "and it is a real plan");

        assert!(eng
            .explain(WsId(1), RunTag(3), "EXPLAIN EXECUTE p(1)".into())
            .await
            .expect_err("the gesture cannot build a physical plan for a Statement node")
            .contains("Execute"));
    }

    /// **A `RESET` lands on the Settings baseline, not on DataFusion's default.** That is the
    /// whole reason it is intercepted: native `RESET` would put the key back to 8192 and take the
    /// user's own setting with it. A key Settings does not name falls back to DataFusion's own
    /// default, which for that key *is* the baseline.
    #[tokio::test]
    async fn a_reset_lands_on_the_settings_baseline() {
        let eng = engine(&[("datafusion.execution.batch_size", "4096")]);
        statement(&eng, "SET datafusion.execution.batch_size = 1024")
            .await
            .expect("set");
        let report = statement(&eng, "RESET datafusion.execution.batch_size")
            .await
            .expect("reset");
        assert_eq!(
            report.message,
            "Reset 'datafusion.execution.batch_size' to '4096'"
        );
        assert_eq!(live(&eng, "datafusion.execution.batch_size").await, "4096");

        let custom = "datafusion.optimizer.filter_null_join_keys";
        statement(&eng, &format!("SET {custom} = true"))
            .await
            .expect("set");
        assert_eq!(live(&eng, custom).await, "true");
        let report = statement(&eng, &format!("RESET {custom}"))
            .await
            .expect("reset");
        assert_eq!(report.message, format!("Reset '{custom}' to its default"));
        assert_eq!(live(&eng, custom).await, "false");
    }

    /// **The key classes an overlay may not hold refuse, and change nothing** — on `RESET` as much
    /// as on `SET`, because a native `RESET` of a runtime key rebuilds the `RuntimeEnv` exactly as
    /// a native `SET` does.
    ///
    /// The dialect is here for the same reason `format.*` is, and it is the one that bites
    /// silently: the language service carries the dialect on its own `Catalog` snapshot, built
    /// from the **Settings** store, while the validator and the planner read it **live** — so a
    /// session value leaves completion lexing the buffer by rules the planner has already stopped
    /// using (WJ-04). Nothing fails; the two layers just quietly disagree.
    #[tokio::test]
    async fn keys_the_app_reads_from_settings_refuse_toward_settings() {
        let eng = engine(&[]);
        let cases = [
            ("datafusion.catalog.default_catalog", Blocked::SetOwned),
            ("datafusion.catalog.default_schema", Blocked::SetOwned),
            ("datafusion.sql_parser.collect_spans", Blocked::SetOwned),
            ("datafusion.runtime.memory_limit", Blocked::SetRuntime),
            ("datafusion.format.null", Blocked::SetFormat),
            ("datafusion.sql_parser.dialect", Blocked::SetDialect),
        ];
        for (key, blocked) in cases {
            for sql in [format!("SET {key} = 'x'"), format!("RESET {key}")] {
                assert_eq!(
                    statement(&eng, &sql).await.expect_err("refused"),
                    blocked.editor_message(),
                    "{sql}"
                );
            }
        }
        assert_eq!(
            live(&eng, "datafusion.catalog.default_catalog").await,
            CATALOG
        );
        assert_eq!(
            live(&eng, "datafusion.catalog.default_schema").await,
            SCHEMA
        );
        assert_eq!(
            live(&eng, "datafusion.sql_parser.collect_spans").await,
            "true"
        );
        assert_eq!(live(&eng, "datafusion.sql_parser.dialect").await, "generic");
        assert!(!eng.restart_owed(), "and nothing owes a restart");
    }

    /// **The overlay wins for its keys until `RESET` or restart.** A Settings Apply over an
    /// overlaid key records the new baseline and leaves the live value alone — otherwise the last
    /// thing the user typed would be silently overwritten by a pane they were not looking at.
    #[tokio::test]
    async fn a_settings_apply_records_the_baseline_and_leaves_a_session_value_alone() {
        let eng = engine(&[("datafusion.execution.batch_size", "4096")]);
        statement(&eng, "SET datafusion.execution.batch_size = 1024")
            .await
            .expect("set");

        eng.set_config(
            [(
                "datafusion.execution.batch_size".to_string(),
                "2048".to_string(),
            )]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            live(&eng, "datafusion.execution.batch_size").await,
            "1024",
            "the session value survives a Settings Apply"
        );
        statement(&eng, "RESET datafusion.execution.batch_size")
            .await
            .expect("reset");
        assert_eq!(live(&eng, "datafusion.execution.batch_size").await, "2048");
        eng.set_config(
            [(
                "datafusion.execution.batch_size".to_string(),
                "512".to_string(),
            )]
            .into_iter()
            .collect(),
        );
        assert_eq!(live(&eng, "datafusion.execution.batch_size").await, "512");
    }

    /// **A restart clears the session** — the overlay, the prepared statements and the mirror —
    /// and it does so by construction: the remount builds a new `Engine`, whose [`SessionScope`]
    /// is a fresh `Default`.
    #[tokio::test]
    async fn a_restart_clears_the_overlay_and_the_prepared_statements() {
        let overrides = [("datafusion.execution.batch_size", "4096")];
        let eng = engine(&overrides);
        statement(&eng, "SET datafusion.execution.batch_size = 1024")
            .await
            .expect("set");
        statement(&eng, "PREPARE p AS SELECT 1")
            .await
            .expect("prepared");
        assert_eq!(eng.prepared().len(), 1);

        let restarted = engine(&overrides);
        assert_eq!(
            live(&restarted, "datafusion.execution.batch_size").await,
            "4096",
            "the session value is gone with the engine that held it"
        );
        assert!(restarted.prepared().is_empty());
        assert!(run_err(&restarted, "EXECUTE p")
            .await
            .contains("does not exist"));
    }

    /// **A prepared query executes as an ordinary snapshot-backed result** — same pipeline, same
    /// pages, same everything, which is what `EXECUTE` classifying `Query` buys.
    #[tokio::test]
    async fn a_prepared_query_executes_into_an_ordinary_snapshot() {
        let eng = engine(&[]);
        let report = statement(&eng, "PREPARE p(INT) AS SELECT $1 + 1 AS n")
            .await
            .expect("prepared");
        assert_eq!(report.message, "Prepared 'p' for this session");
        assert_eq!(report.effect, Some(StoreEffect::PreparedChanged));

        let RunOutcome::Rows(output, _) = eng
            .run(WsId(1), RunTag(2), "EXECUTE p(41)".into(), 10)
            .await
            .expect("executed")
        else {
            panic!("EXECUTE did not return rows");
        };
        assert_eq!(output.rows[0][0].text, "42");
        let snap = output.snapshot.expect("a snapshot handle");
        assert!(
            eng.snapshot_live(snap),
            "and it pages like any other result"
        );
    }

    /// **A non-query body is refused at `PREPARE`, because nothing downstream can refuse it.**
    /// `SQLOptions::verify_plan` descends into a `Prepare` node's input but an `Execute` node has
    /// no inputs, so an accepted `PREPARE … AS INSERT` would be a write with no gate in front of
    /// it. The router answers off the parsed statement, before anything is planned.
    #[tokio::test]
    async fn preparing_a_non_query_is_refused() {
        let eng = engine(&[]);
        assert_eq!(
            run_err(&eng, "PREPARE bad AS INSERT INTO t VALUES (1)").await,
            Blocked::PrepareNonQuery.editor_message()
        );
        assert!(eng.prepared().is_empty());
    }

    /// **`DEALLOCATE` is DataFusion's, error included.** A name it does not hold answers in its
    /// own words, which is why the mirror is written after the dispatch and never before it.
    #[tokio::test]
    async fn deallocate_removes_the_statement_and_keeps_datafusions_error() {
        let eng = engine(&[]);
        statement(&eng, "PREPARE p AS SELECT 1")
            .await
            .expect("prepared");
        let report = statement(&eng, "DEALLOCATE p").await.expect("deallocated");
        assert_eq!(report.message, "Deallocated 'p'");
        assert_eq!(report.effect, Some(StoreEffect::PreparedChanged));
        assert!(eng.prepared().is_empty());

        assert!(run_err(&eng, "EXECUTE p(1)")
            .await
            .contains("Prepared statement 'p' does not exist"));
        assert!(statement(&eng, "DEALLOCATE p")
            .await
            .expect_err("gone")
            .contains("does not exist"));
    }

    /// **A duplicate name keeps DataFusion's own error, and the mirror does not learn from it.**
    #[tokio::test]
    async fn preparing_the_same_name_twice_is_datafusions_refusal() {
        let eng = engine(&[]);
        statement(&eng, "PREPARE p AS SELECT 1")
            .await
            .expect("prepared");
        assert!(statement(&eng, "PREPARE p AS SELECT 2")
            .await
            .expect_err("taken")
            .contains("already exists"));
        assert_eq!(eng.prepared().len(), 1);
    }

    /// **Completion offers a prepared name where a prepared name is the only thing that fits**,
    /// and drops it when the statement is deallocated. The mirror exists for exactly this:
    /// `SessionState::prepared_plans` is `pub(crate)`, so there is nothing else to ask.
    #[tokio::test]
    async fn completion_offers_prepared_names_at_execute_and_deallocate() {
        let eng = engine(&[]);
        statement(&eng, "PREPARE spend(INT) AS SELECT $1 AS n")
            .await
            .expect("prepared");
        let catalog =
            |eng: &Engine| Catalog::build([], [], Arc::default(), eng.prepared(), "generic".into());

        let cat = catalog(&eng);
        for sql in ["EXECUTE ", "DEALLOCATE ", "DEALLOCATE PREPARE "] {
            let items = complete(sql, sql.len(), &cat, false);
            assert_eq!(
                items.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
                vec!["spend"],
                "{sql}"
            );
            assert_eq!(items[0].kind, CompletionKind::Function);
            assert_eq!(items[0].insert, "spend", "the bare name, never a call");
            assert_eq!(items[0].detail.as_deref(), Some("(Int32)"));
        }

        statement(&eng, "DEALLOCATE spend").await.expect("gone");
        assert!(complete("EXECUTE ", 8, &catalog(&eng), false).is_empty());
    }
}
