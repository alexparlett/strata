//! What the engine hands every arm.
//!
//! One value rather than a parameter list, because it is one thing — the engine, minus everything
//! an arm may not touch. It is what makes the arm contract uniform: every arm takes
//! `(&StmtCtx, &Principal, &Qualified)` and nothing else, so an arm that grows a need reaches for
//! a member here rather than for a signature of its own.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use datafusion::prelude::SessionContext;

use crate::formats::Formats;
use crate::functions::Functions;
use crate::policy::{Admit, PolicyProvider, Principal, TargetFacts};
use crate::sources::{connection_facts, Live};
use crate::statements::classify::{denied, Form};
use crate::statements::target::Target;
use crate::statements::StmtKind;
use crate::{Connections, InternalTables};

use super::arms::session::SessionScope;

/// Where an intercepted statement may write, and what it may write **relative to**.
///
/// The **project folder**, not `.strata/tables` — because a statement that creates an internal
/// table produces two things from it: an absolute path to spool into, and the project-relative
/// source path the def stores, which is what makes the def portable
/// ([`internal_source`](strata_core::project::internal_source)). Handing down only the data directory
/// would leave the def naming an absolute path on the machine that ran the statement.
///
/// `None` is an engine with no project behind it — the agent's headless workspaces before a
/// project is opened, and every test fixture. Nothing that only reads notices; the arms that
/// write refuse politely.
pub type DataRoot = Option<PathBuf>;

/// What an intercepted statement can reach **of the engine**, gathered once in
/// [`Workspace::run`](crate::Workspace::run).
///
/// Every member is a copy — a handle where the state is shared, a clone where it is a value — for
/// one reason: the arms run inside the task `Engine::bookkeep` spawned, and that task must not
/// hold the engine, because the engine's `Drop` is what aborts it. `internal`, `scope` and
/// `functions` hold values only, so they outlive an engine harmlessly; `root` and `baseline` are
/// snapshots taken at dispatch, which is the moment they are true for.
pub struct StmtCtx {
    /// The session every arm plans against.
    pub ctx: SessionContext,
    /// The buffer the statement was parsed from, which the `remote` arm splices the text it
    /// dispatches out of; every other arm works off the parsed statement.
    pub sql: String,
    /// Where an internal table's data may be written.
    pub root: DataRoot,
    /// Which registered tables Strata owns the data of.
    pub internal: InternalTables,
    /// Which object stores this project has a connection to — what a typed
    /// `CREATE EXTERNAL TABLE`'s `LOCATION` may name.
    pub connections: Connections,
    /// The live database connections — what a write into a remote relation goes through,
    /// and what says whether one accepts writes at all.
    pub(crate) sources: Live,
    /// The file formats this engine reads — what `STORED AS` may name, and what builds the
    /// reader a table registers with.
    pub(crate) formats: Formats,
    /// The `SET` overlay and the prepared-statement mirror.
    pub scope: SessionScope,
    /// The function catalog and the names this session created.
    pub functions: Functions,
    /// The engine's `datafusion.*` overrides — what a `RESET` puts a key back to
    /// (`session::reset`), which is the Settings baseline rather than DataFusion's default.
    pub baseline: BTreeMap<String, String>,
    /// Who decides what the caller may do — the engine's own slot
    /// ([`EngineBuilder::with_policy`](crate::EngineBuilder::with_policy)), asked once more at
    /// the arm now that the target has resolved.
    pub(crate) policy: Arc<dyn PolicyProvider>,
}

impl StmtCtx {
    /// Refuses unless `who` may perform `kind` against `target` — **the fine phase**, and the one
    /// entry into it.
    ///
    /// The grant family is derived from the kind and the locality from the target, so an arm
    /// names neither: it cannot ask about the wrong family, and it cannot check a locality other
    /// than the one it resolved. The scope narrowing rides the same answer, which is why the
    /// connection's own facts are gathered here rather than passed in.
    ///
    /// A [`Target::Nowhere`] permits: there is no locality to judge, and the arm refuses it in
    /// its own words on the next line. Refusing here instead would tell a caller they lack a
    /// grant when what they actually named is a catalog that does not exist.
    ///
    /// # Errors
    ///
    /// The policy provider denied, or could not decide — the second surfaced in its own words,
    /// since a provider outage is not a statement the caller may not perform.
    pub(crate) async fn require_target(
        &self,
        who: &Principal,
        kind: StmtKind,
        target: &Target,
    ) -> Result<(), String> {
        let Some(locality) = target.locality() else {
            return Ok(());
        };
        let form = Form::Statement(kind);
        let facts = match target {
            Target::Remote(at) => connection_facts(&self.sources, &at.connection),
            // The second arm is unreachable past the guard above, and is stated rather than
            // wildcarded so a fourth kind of target has to decide what it tells the provider.
            Target::Workspace { .. } | Target::Nowhere { .. } => TargetFacts {
                locality,
                ..Default::default()
            },
        };
        match self.policy.permit(who, form.family(), &facts).await? {
            Admit::Allow => Ok(()),
            Admit::Deny(code) => Err(denied(form, code)),
        }
    }
}

/// **The fine phase, end to end** — a capability the coarse phase admits, refused at the arm
/// because the resolved target is a connection it does not reach.
///
/// Not a unit test on `permits`, which passed before this call site existed: what is under test is
/// that `Engine::run` actually asks. `Capability::full()` admits `DELETE` at *any* locality, so
/// the coarse phase lets both statements through and the only thing that can tell them apart is
/// the connection the target resolved into.
///
/// Two connections of two registered kinds, so the scoped and the unscoped answer come from one
/// fixture and neither can be the fixture's own doing.
#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use std::sync::Arc;

    use crate::policy::{Capability, CapabilityPolicyProvider, GrantFamily, RemoteSel};
    use crate::sources::fake::{fake_def, TestDoc, TestSql};
    use crate::statements::classify::denied;
    use crate::statements::StmtKind;
    use crate::{
        Admit, DenyCode, Engine, Form, PolicyProvider, Principal, RunTag, TargetFacts, WsId,
    };
    use strata_model::ConnectionDef;

    /// `fake_def`, opted in to writes — the def-level gate is the arm's third and is not what
    /// these tests are about.
    fn writable(def: ConnectionDef) -> ConnectionDef {
        let mut def = def;
        if let strata_model::Provider::Source(source) = &mut def.provider {
            source.read_only = false;
        }
        def
    }

    /// An engine whose policy allows everything except remote targets outside `reaches`, holding
    /// two writable connections: `docs` (kind `test-doc`) and `sales` (kind `test-sql`).
    async fn engine(reaches: &str) -> Arc<Engine> {
        let eng = Engine::builder()
            .with_source(TestDoc::holding("docs", &["orders"]))
            .with_source(TestSql::holding("server", &["orders"]))
            .with_policy(CapabilityPolicyProvider::new(
                Capability::full().remote_only([RemoteSel::Kind(reaches.to_string())]),
            ))
            .build();
        for def in [
            fake_def::<TestDoc>("docs", "docs"),
            fake_def::<TestSql>("sales", "server"),
        ] {
            eng.sources()
                .connect(writable(def))
                .await
                .expect("the fixture connects");
        }
        eng
    }

    async fn run(eng: &Engine, sql: &str) -> Result<(), String> {
        eng.ws(WsId(1))
            .run(RunTag(1), sql.into(), 10)
            .await
            .map(|_| ())
    }

    /// A capability scoped to one backend kind refuses a write to a connection of another —
    /// and the sentence is the engine's own table, not the provider's.
    #[tokio::test]
    async fn a_scoped_capability_refuses_a_connection_of_another_kind() {
        let eng = engine("test-sql").await;
        assert_eq!(
            run(&eng, "DELETE FROM docs.public.orders WHERE id = 1")
                .await
                .expect_err("out of scope"),
            denied(Form::Statement(StmtKind::Delete), DenyCode::OutOfScope)
        );
    }

    /// And the same statement against a connection the scope *does* reach gets past the policy —
    /// landing on the source's own refusal, which is what says the gate was the only thing in the
    /// way. `TestDoc` runs no statement of its own, so the sentence is the trait's.
    #[tokio::test]
    async fn the_same_write_passes_when_the_scope_reaches_the_kind() {
        let eng = engine("test-doc").await;
        let why = run(&eng, "DELETE FROM docs.public.orders WHERE id = 1")
            .await
            .expect_err("the source cannot run a statement of its own");
        assert!(
            why.contains("test-doc") && why.contains("run a statement of its own"),
            "the policy refused instead of the source: {why}"
        );
    }

    /// A provider that admits every family and refuses every target — an implementation whose
    /// two phases disagree, which the seam allows and the engine has to survive.
    struct CoarseYesFineNo;

    #[async_trait]
    impl PolicyProvider for CoarseYesFineNo {
        async fn admit(&self, _: &Principal, _: GrantFamily) -> Result<Admit, String> {
            Ok(Admit::Allow)
        }

        async fn permit(
            &self,
            _: &Principal,
            _: GrantFamily,
            _: &TargetFacts,
        ) -> Result<Admit, String> {
            Ok(Admit::Deny(DenyCode::NotGranted))
        }
    }

    /// A provider that cannot decide once the target is known.
    struct FineUnreachable;

    #[async_trait]
    impl PolicyProvider for FineUnreachable {
        async fn admit(&self, _: &Principal, _: GrantFamily) -> Result<Admit, String> {
            Ok(Admit::Allow)
        }

        async fn permit(
            &self,
            _: &Principal,
            _: GrantFamily,
            _: &TargetFacts,
        ) -> Result<Admit, String> {
            Err("the policy service is unreachable".into())
        }
    }

    /// **The arm asks last and its answer stands.** A provider that admits coarsely and denies at
    /// the target refuses the statement: inconsistency can delay a refusal, never grant one.
    #[tokio::test]
    async fn an_inconsistent_provider_refuses_at_the_arm() {
        let eng = Engine::builder().with_policy(CoarseYesFineNo).build();
        assert_eq!(
            run(&eng, "CREATE TABLE t (id BIGINT)")
                .await
                .expect_err("the fine phase denied"),
            denied(Form::Statement(StmtKind::CreateTable), DenyCode::NotGranted)
        );
    }

    /// And a provider that could not decide is a **fault**, surfaced in its own words: "nobody
    /// could say" is not "you may not", and the statement is refused either way.
    #[tokio::test]
    async fn a_provider_that_cannot_decide_at_the_arm_refuses_in_its_own_words() {
        let eng = Engine::builder().with_policy(FineUnreachable).build();
        assert_eq!(
            run(&eng, "CREATE TABLE t (id BIGINT)")
                .await
                .expect_err("the fine phase could not answer"),
            "the policy service is unreachable"
        );
    }

    /// **The workspace is not narrowed by a remote scope.** A scope refines the remote-locality
    /// grants and nothing else, so a project table is still the caller's to create — the refusal
    /// this engine gives is the arm's own (it has no project folder), never the policy's.
    #[tokio::test]
    async fn a_remote_scope_leaves_the_workspace_alone() {
        let eng = engine("test-sql").await;
        let why = run(&eng, "CREATE TABLE t (id BIGINT)")
            .await
            .expect_err("an engine with no project folder cannot store a table");
        assert!(
            why.contains("needs a project folder"),
            "the remote scope reached a workspace target: {why}"
        );
    }
}
