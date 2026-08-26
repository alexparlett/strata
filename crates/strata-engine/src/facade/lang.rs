//! The language service's engine-side face: what a buffer means to this session.

use std::sync::Arc;

use strata_model::Diagnostic;

use crate::policy::{Capability, Principal};
use crate::sql::{self, FunctionCatalog, PreparedSym};
use crate::statements::arms;
use crate::statements::pipeline::{self, Pipeline};
use crate::statements::PolicyRefusal;
use crate::Engine;

/// This engine's language service, from [`Engine::lang`].
///
/// Everything the editor asks about SQL without running it: a buffer's diagnostics, what a
/// read-only caller would be refused, what a type spells to, and the symbols completion offers.
#[derive(Clone, Copy)]
pub struct Lang<'a> {
    pub(super) engine: &'a Engine,
}

impl Lang<'_> {
    /// Validate `sql` against this engine's live session: lexical lints,
    /// managed-DDL policy, and a **dry-plan** of each statement — parse → resolve →
    /// analyze, never execute — so the diagnostics are exactly the errors a Run would
    /// hit. Total by design: faults come back as `Diagnostic`s, not an `Err`.
    pub async fn validate(self, sql: String) -> Vec<Diagnostic> {
        let ctx = self.engine.ctx.clone();
        let policy = self.engine.policy.clone();
        let functions = self.engine.functions.catalog();
        self.engine
            .rt()
            .spawn(async move {
                let pipeline = Pipeline::new(&ctx);
                sql::validate(&pipeline, policy.as_ref(), &functions, &sql).await
            })
            .await
            .unwrap_or_default()
    }

    /// Returns every statement in `sql` that a read-only caller may not perform.
    ///
    /// The same pipeline [`validate`](Self::validate) and
    /// [`Workspace::run`](crate::Workspace::run) use, one capability apart. An empty answer is
    /// a clean pass; a caller refuses dispatch on any other answer, including an `Err`.
    pub async fn policy_verdicts(self, sql: String) -> Result<Vec<PolicyRefusal>, String> {
        let ctx = self.engine.ctx.clone();
        let policy = self.engine.policy.clone();
        self.engine
            .rt()
            .spawn(async move {
                let pipeline = Pipeline::new(&ctx);
                let who = Principal::new(Capability::read_only());
                pipeline::policy_verdicts(&pipeline, policy.as_ref(), &who, &sql).await
            })
            .await
            .map_err(|e| format!("policy task failed: {e}"))?
    }

    /// What this session's planner makes of one **SQL column type** — the empty-table panel's
    /// per-row validation. `Ok` is the Arrow type in the spelling every surface shows
    /// it in; `Err` is the planner's own refusal, verbatim.
    ///
    /// A plan and nothing more, so it is as cheap as a diagnostics pass and has no more effect
    /// than one — see [`column_type`](crate::statements::arms::column_type) for why the offer cannot be authored instead.
    pub async fn column_type(self, sql_type: String) -> Result<String, String> {
        let ctx = self.engine.ctx.clone();
        self.engine
            .rt()
            .spawn(async move { arms::column_type(&ctx, &sql_type).await })
            .await
            .map_err(|e| format!("column type task failed: {e}"))?
    }

    /// The registered SQL functions (the editor's language catalog), as they stand.
    ///
    /// By handle rather than by reference, because the set is swappable: a `CREATE FUNCTION`
    /// replaces it wholesale (`statements::arms::functions`), so a caller that held a borrow would be holding
    /// the engine's lock for as long as it read.
    pub fn functions(self) -> Arc<FunctionCatalog> {
        self.engine.functions.catalog()
    }

    /// The statements `PREPARE` has left in this session, as language-service symbols —
    /// what completion offers at an `EXECUTE` / `DEALLOCATE` operand.
    ///
    /// Off the engine's own mirror, because DataFusion's `SessionState::prepared_plans` is
    /// `pub(crate)` and has no public enumeration.
    pub fn prepared(self) -> Vec<PreparedSym> {
        self.engine.session.prepared()
    }
}
