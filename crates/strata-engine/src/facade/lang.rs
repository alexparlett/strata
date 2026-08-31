//! The language service's engine-side face: what a buffer means to this session.

use std::sync::Arc;

use strata_model::Diagnostic;

use crate::policy::{Capability, Principal};
use crate::sql::{self, FunctionCatalog, LangBundle, PreparedSym};
use crate::statements::arms;
use crate::statements::pipeline::{self, Pipeline};
use crate::statements::PolicyRefusal;
use crate::{Engine, EngineError};

/// This engine's language service, from [`Engine::lang`].
///
/// Everything the editor asks about SQL without running it: a buffer's diagnostics, what a
/// read-only caller would be refused, what a type spells to, and the symbols completion offers.
#[derive(Clone, Copy)]
pub struct Lang<'a> {
    pub(super) engine: &'a Engine,
}

impl Lang<'_> {
    /// Every diagnostic `sql` draws against this engine's live session: lexical lints, the
    /// statement pipeline's own parse/qualify/classify, the native name resolver, and a
    /// **dry-plan** of each statement — never an execution — so the diagnostics are exactly the
    /// errors a Run would hit. Total by design: faults come back as `Diagnostic`s, not an `Err`.
    ///
    /// One of the language service's two doors, and the only asynchronous one: it reads the live
    /// session, where [`complete`](crate::sql::complete) reads a snapshot the caller holds. See
    /// `sql::service` for the tiers and the no-divergence invariant they keep.
    pub async fn analyze(self, sql: String) -> Vec<Diagnostic> {
        let ctx = self.engine.ctx.clone();
        let policy = self.engine.policy.clone();
        let functions = self.engine.functions.catalog();
        self.engine
            .rt()
            .spawn(async move {
                let pipeline = Pipeline::new(&ctx);
                sql::analyze(&pipeline, policy.as_ref(), &functions, &sql).await
            })
            .await
            .unwrap_or_default()
    }

    /// **Everything the language service needs off this engine, as of one moment** — the sync
    /// half of the wiring, folded into a [`Catalog`](crate::sql::Catalog) by the caller.
    ///
    /// Lock-reads only: no I/O, no plan, no round trip. One call rather than four because these
    /// are four reads of one session that must describe one instant, and because the caller has
    /// exactly one reason to take them — [`generation`](LangBundle::generation) moved.
    pub fn bundle(self) -> LangBundle {
        let engine = self.engine;
        LangBundle {
            functions: engine.functions.catalog(),
            prepared: engine.session.prepared(),
            formats: engine.formats(),
            databases: self.engine.sources().database_syms(),
            generation: engine.generation.current(),
        }
    }

    /// Returns every statement in `sql` that a read-only caller may not perform.
    ///
    /// The same pipeline [`analyze`](Self::analyze) and
    /// [`Workspace::run`](crate::Workspace::run) use, one capability apart. An empty answer is
    /// a clean pass; a caller refuses dispatch on any other answer, including an `Err`.
    pub async fn policy_verdicts(self, sql: String) -> Result<Vec<PolicyRefusal>, EngineError> {
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
            .map_err(|e| EngineError::task("policy", e))?
            .map_err(EngineError::from)
    }

    /// What this session's planner makes of one **SQL column type** — the empty-table panel's
    /// per-row validation. `Ok` is the Arrow type in the spelling every surface shows
    /// it in; `Err` is the planner's own refusal, verbatim.
    ///
    /// A plan and nothing more, so it is as cheap as a diagnostics pass and has no more effect
    /// than one — see [`column_type`](crate::statements::arms::column_type) for why the offer cannot be authored instead.
    pub async fn column_type(self, sql_type: String) -> Result<String, EngineError> {
        let ctx = self.engine.ctx.clone();
        self.engine
            .rt()
            .spawn(async move { arms::column_type(&ctx, &sql_type).await })
            .await
            .map_err(|e| EngineError::task("column type", e))?
            .map_err(EngineError::from)
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

#[cfg(test)]
mod tests {
    use crate::sql::Catalog;
    use crate::{Engine, RunTag, WsId};

    /// **One read, one instant.** Every field of the bundle is the same answer the narrower call
    /// gives, and its generation is the catalog's own — which is what lets a consumer key its
    /// snapshot on it and be sure the popup and the squiggles are describing one session rather
    /// than four moments stitched together.
    #[tokio::test]
    async fn the_bundle_is_the_engine_as_of_one_moment() {
        let eng = Engine::builder().build();
        eng.ws(WsId(1))
            .run(RunTag(1), "PREPARE p AS SELECT 1 AS n".into(), 10)
            .await
            .expect("prepare");

        let bundle = eng.lang().bundle();
        assert_eq!(bundle.generation, eng.catalog().generation());
        assert_eq!(bundle.functions, eng.lang().functions());
        assert_eq!(bundle.prepared, eng.lang().prepared());
        assert_eq!(bundle.formats, eng.formats());
        assert_eq!(bundle.databases, eng.sources().database_syms());
    }

    /// The one constructor takes the bundle whole and keeps the caller's own two halves — the
    /// store's rows and the caller's dialect, which the engine is not the author of.
    #[tokio::test]
    async fn the_constructor_folds_the_bundle_and_keeps_what_is_not_the_engines() {
        let eng = Engine::builder().build();
        let bundle = eng.lang().bundle();
        let catalog = Catalog::build(
            [("orders", &[][..], true)],
            [("recent", &[][..])],
            bundle.clone(),
            "postgres".into(),
        );
        assert_eq!(catalog.functions, bundle.functions);
        assert_eq!(catalog.formats, bundle.formats);
        assert_eq!(
            catalog.dialect, "postgres",
            "the caller's setting, not the bundle's"
        );
        assert!(
            catalog.table("orders").is_some_and(|t| t.internal),
            "a store row the registration learned nothing about is still offered by name"
        );
        assert!(catalog.table("recent").is_some_and(|t| t.is_view));
    }
}
