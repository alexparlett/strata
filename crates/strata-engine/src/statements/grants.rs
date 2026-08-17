//! **Policy** — who may perform what, and the one place a refusal is worded.
//!
//! Two things live here, and the split is the design. [`PolicyProvider`] is the **seam**: an
//! injected async trait answering `Allow` or `Deny(code)` — codes, never prose, so the engine
//! mints every sentence and a provider cannot reword a refusal the parity tests pin. A service
//! deployment (Cognito, AD, OPA) implements it and decides at check time. [`Capability`] is the
//! shipped **data model** behind [`CapabilityPolicyProvider`], the default: a bitset of
//! [`Grant`]s over a [`Locality`] axis, plus a [`RemoteScope`] refining the remote half.
//!
//! **Restriction is explicit data.** An engine built with no policy refuses nothing
//! (`CapabilityPolicyProvider::new(Capability::full())`), which is the DataFusion-native posture;
//! the app opens its editor at [`Capability::full`] and its agent at [`Capability::read_only`],
//! and the headless host builds its engine read-only outright.
//!
//! **A caller narrows, never widens.** The capability a [`Principal`] carries is intersected with
//! the provider's own, so an engine built read-only stays read-only whatever a caller asks for —
//! and an engine built full is exactly as permissive as its callers ask to be, which is what makes
//! one engine serve a full editor and a read-only agent at once.
//!
//! **Two phases.** [`PolicyProvider::admit`] is coarse and runs at classification: may this
//! principal ever perform this family, at any locality? [`PolicyProvider::permit`] is fine and
//! runs at the arm, once the target is resolved. The engine **fails closed** under a provider
//! that answers the two inconsistently: a coarse allow followed by a fine deny refuses at the
//! arm, so an inconsistency can delay a refusal and never grant one.

use std::any::Any;
use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use super::classify::{Form, StmtKind, DROP_UNSUPPORTED, UNSUPPORTED};
use crate::WsId;

/// Where a statement's target lives.
///
/// Shared with the dispatch layer's own target axis, so the fine check is *derived* from the
/// resolved target and an arm never names a scope or checks the wrong one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Locality {
    /// The workspace catalog — a project table, view or internal table.
    #[default]
    Local,
    /// A relation inside a database connection's catalog.
    Remote,
}

/// One thing a caller may do, factored by action × locality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Grant {
    /// Reading, of any source. Held by every preset: reading is what a connection is for, and a
    /// policy that refuses it refuses the product.
    Read,
    /// `INSERT` / CTAS — a workspace internal table, or a connection that opted in.
    Write(Locality),
    /// Table DDL — the workspace's `CREATE` / `DROP` / `CREATE EXTERNAL TABLE`, and the statements
    /// only a server can run. `UPDATE` and `DELETE` are among them: both are refused a workspace
    /// target outright, so the only data they can reach is a server's own.
    Ddl(Locality),
    /// Workspace views — ⌘S's funnel. Split from [`Ddl`](Grant::Ddl) because "may save views,
    /// may not reshape tables" is a plausible policy; a **remote** view is `Ddl(Remote)`, since
    /// there it is the server's schema that changes.
    ViewDdl,
    /// `COPY … TO` and export. Scope-free: it is file egress, and the file is not in any catalog.
    CopyOut,
    /// `SET` / `RESET` and `PREPARE` / `EXECUTE` / `DEALLOCATE` — engine-local session state.
    Session,
    /// `CREATE FUNCTION` / `DROP FUNCTION` — engine-local (DataFusion refuses a qualified
    /// function name itself, so there is no remote half to have).
    Functions,
}

impl Grant {
    /// This grant's bit. Hand-rolled rather than a bitflags dependency: nine bits, one table, and
    /// the payload means the enum cannot be `as`-cast into one.
    fn bit(self) -> u16 {
        let index = match self {
            Grant::Read => 0,
            Grant::Write(Locality::Local) => 1,
            Grant::Write(Locality::Remote) => 2,
            Grant::Ddl(Locality::Local) => 3,
            Grant::Ddl(Locality::Remote) => 4,
            Grant::ViewDdl => 5,
            Grant::CopyOut => 6,
            Grant::Session => 7,
            Grant::Functions => 8,
        };
        1 << index
    }

    /// Every grant there is — what [`Capability::full`] holds, and what a bitset round-trips over.
    pub const ALL: [Grant; 9] = [
        Grant::Read,
        Grant::Write(Locality::Local),
        Grant::Write(Locality::Remote),
        Grant::Ddl(Locality::Local),
        Grant::Ddl(Locality::Remote),
        Grant::ViewDdl,
        Grant::CopyOut,
        Grant::Session,
        Grant::Functions,
    ];

    /// Whether a [`RemoteScope`] refines this grant — true exactly for the remote-locality ones.
    /// [`Read`](Grant::Read) is not among them: a scope that could refuse a read would make
    /// "reading is never refused per source" a claim the type contradicts.
    fn scoped(self) -> bool {
        matches!(
            self,
            Grant::Write(Locality::Remote) | Grant::Ddl(Locality::Remote)
        )
    }
}

/// A set of [`Grant`]s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Grants(u16);

impl Grants {
    /// The empty set.
    pub fn none() -> Self {
        Grants(0)
    }

    /// Every grant.
    pub fn all() -> Self {
        Grant::ALL.into_iter().fold(Grants::none(), Grants::with)
    }

    /// This set plus `grant`.
    pub fn with(self, grant: Grant) -> Self {
        Grants(self.0 | grant.bit())
    }

    /// Whether `grant` is in this set.
    pub fn holds(self, grant: Grant) -> bool {
        self.0 & grant.bit() != 0
    }

    /// The grants both sets hold.
    pub fn intersect(self, other: Grants) -> Self {
        Grants(self.0 & other.0)
    }
}

/// The coarse axis: an action, with the locality taken out. What [`PolicyProvider::admit`] asks
/// about at classification, when nothing has resolved a target yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantFamily {
    Read,
    Write,
    Ddl,
    ViewDdl,
    CopyOut,
    Session,
    Functions,
}

impl GrantFamily {
    /// Every family — what the conformance module walks.
    pub const ALL: [GrantFamily; 7] = [
        GrantFamily::Read,
        GrantFamily::Write,
        GrantFamily::Ddl,
        GrantFamily::ViewDdl,
        GrantFamily::CopyOut,
        GrantFamily::Session,
        GrantFamily::Functions,
    ];

    /// The grant this family needs **at `locality`** — the one derivation, so an arm never names
    /// a grant and cannot check the wrong one.
    ///
    /// A remote view is the server's schema changing, so it takes that connection's DDL grant
    /// rather than the workspace's view grant.
    fn grant(self, locality: Locality) -> Grant {
        match self {
            GrantFamily::Read => Grant::Read,
            GrantFamily::Write => Grant::Write(locality),
            GrantFamily::Ddl => Grant::Ddl(locality),
            GrantFamily::ViewDdl => match locality {
                Locality::Local => Grant::ViewDdl,
                Locality::Remote => Grant::Ddl(Locality::Remote),
            },
            GrantFamily::CopyOut => Grant::CopyOut,
            GrantFamily::Session => Grant::Session,
            GrantFamily::Functions => Grant::Functions,
        }
    }
}

impl Form {
    /// The grant family this form belongs to — the coarse question, any locality. Wildcard-free
    /// on [`StmtKind`], so a new kind is a compile error here rather than a statement that
    /// silently inherits somebody else's policy.
    ///
    /// `Execute` is `Session` because a prepared statement is session state and running one
    /// reaches it: a caller that may not `PREPARE` may not `EXECUTE` either.
    pub fn family(self) -> GrantFamily {
        let kind = match self {
            Form::Read => return GrantFamily::Read,
            Form::Execute => return GrantFamily::Session,
            Form::Statement(kind) => kind,
        };
        match kind {
            StmtKind::Insert | StmtKind::Ctas => GrantFamily::Write,
            StmtKind::CreateTable
            | StmtKind::CreateExternalTable
            | StmtKind::DropTable
            | StmtKind::Update
            | StmtKind::Delete => GrantFamily::Ddl,
            StmtKind::CreateView | StmtKind::DropView => GrantFamily::ViewDdl,
            StmtKind::Copy => GrantFamily::CopyOut,
            StmtKind::Set | StmtKind::Reset | StmtKind::Prepare | StmtKind::Deallocate => {
                GrantFamily::Session
            }
            StmtKind::CreateFunction | StmtKind::DropFunction => GrantFamily::Functions,
        }
    }
}

/// Which database connections a capability's **remote** grants reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteScope {
    /// Every connection — [`Capability::full`]'s default.
    All,
    /// Only these, e.g. writes to the sqlite connections and never the RDS postgres.
    Only(BTreeSet<RemoteSel>),
}

impl RemoteScope {
    /// Whether this scope reaches the connection `facts` names. A remote target with neither a
    /// kind nor a url matches nothing but [`All`](RemoteScope::All) — an unidentified connection
    /// is not one a selector can have been written for.
    fn reaches(&self, facts: &TargetFacts) -> bool {
        match self {
            RemoteScope::All => true,
            RemoteScope::Only(selectors) => selectors.iter().any(|sel| match sel {
                RemoteSel::Kind(kind) => facts.kind.as_deref() == Some(kind.as_str()),
                RemoteSel::Connection(url) => facts.connection.as_deref() == Some(url.as_str()),
            }),
        }
    }

    /// The connections both scopes reach.
    fn intersect(&self, other: &RemoteScope) -> RemoteScope {
        match (self, other) {
            (RemoteScope::All, other) => other.clone(),
            (mine, RemoteScope::All) => mine.clone(),
            (RemoteScope::Only(a), RemoteScope::Only(b)) => {
                RemoteScope::Only(a.intersection(b).cloned().collect())
            }
        }
    }
}

/// One way of naming database connections in a [`RemoteScope`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RemoteSel {
    /// Every connection of one backend kind (`"postgres"`, `"mysql"`).
    Kind(String),
    /// One connection, by the exact url it is keyed under.
    Connection(String),
}

/// What a caller may do.
///
/// The two presets are what the app and the hosts use; everything else is composed from them.
///
/// # Example
///
/// An MCP client that may read anything and write only the sqlite connections:
///
/// ```
/// use strata_engine::{Capability, Grant, Locality, RemoteSel};
///
/// let mcp = Capability::read_only()
///     .with(Grant::Write(Locality::Remote))
///     .remote_only([RemoteSel::Kind("sqlite".into())]);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capability {
    grants: Grants,
    remote: RemoteScope,
}

impl Capability {
    /// Every grant, over every connection.
    pub fn full() -> Self {
        Capability {
            grants: Grants::all(),
            remote: RemoteScope::All,
        }
    }

    /// Reading and nothing else.
    pub fn read_only() -> Self {
        Capability {
            grants: Grants::none().with(Grant::Read),
            remote: RemoteScope::All,
        }
    }

    /// This capability plus `grant`.
    pub fn with(mut self, grant: Grant) -> Self {
        self.grants = self.grants.with(grant);
        self
    }

    /// This capability with its remote grants narrowed to `selectors`.
    pub fn remote_only(mut self, selectors: impl IntoIterator<Item = RemoteSel>) -> Self {
        self.remote = RemoteScope::Only(selectors.into_iter().collect());
        self
    }

    /// What **both** capabilities allow — how a caller's ask narrows the engine's own ceiling.
    /// Never a union: the whole point is that a caller cannot widen what the embedder built.
    pub fn intersect(&self, other: &Capability) -> Capability {
        Capability {
            grants: self.grants.intersect(other.grants),
            remote: self.remote.intersect(&other.remote),
        }
    }

    /// Whether this capability holds `grant`.
    pub fn holds(&self, grant: Grant) -> bool {
        self.grants.holds(grant)
    }

    /// The coarse answer: may this capability perform `family` at **any** locality? Derived from
    /// the fine check rather than tabulated beside it, so the two cannot disagree.
    fn admits(&self, family: GrantFamily) -> bool {
        [Locality::Local, Locality::Remote]
            .into_iter()
            .any(|locality| self.grants.holds(family.grant(locality)))
    }

    /// The fine answer: may this capability perform `family` against `facts`? **Two named gates**
    /// in order — the grant the family needs at that locality, then the remote scope — and the
    /// code says which one refused, so an embedder's log can tell "may not write at all" from
    /// "may not write *this* connection".
    fn permits(&self, family: GrantFamily, facts: &TargetFacts) -> Result<(), DenyCode> {
        let grant = family.grant(facts.locality);
        if !self.grants.holds(grant) {
            return Err(DenyCode::NotGranted);
        }
        match !grant.scoped() || self.remote.reaches(facts) {
            true => Ok(()),
            false => Err(DenyCode::OutOfScope),
        }
    }
}

/// What a policy decision may turn on about a statement's resolved target.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TargetFacts {
    pub locality: Locality,
    /// The backend kind of the connection the target is in — `None` for a workspace target.
    pub kind: Option<String>,
    /// The connection's url, the key it is registered under — `None` for a workspace target.
    pub connection: Option<String>,
}

impl TargetFacts {
    /// A target in the workspace catalog.
    pub fn workspace() -> Self {
        TargetFacts::default()
    }

    /// A relation inside the database connection `connection`, of backend kind `kind`.
    pub fn remote(kind: impl Into<String>, connection: impl Into<String>) -> Self {
        TargetFacts {
            locality: Locality::Remote,
            kind: Some(kind.into()),
            connection: Some(connection.into()),
        }
    }
}

/// **Who is asking** — the value every [`PolicyProvider`] decides about.
///
/// Two halves, owned by two different layers. The **capability** is the caller's own ask, and it
/// only ever narrows what the engine's provider allows ([`Capability::intersect`]) — which is
/// what lets one engine serve a full editor and a read-only agent without either being able to
/// promote itself. The **claims** are the embedder's: a bearer token, a tenant, a role, attached
/// verbatim and never read here, for a service provider to downcast and decide from.
///
/// A principal is a **value, not a lease**. It carries no expiry and grants nothing by existing:
/// every statement asks the provider again, so a provider that caches owns its own TTL and a
/// revocation takes effect at the next check rather than at some renewal the engine would have to
/// schedule.
#[derive(Clone)]
pub struct Principal {
    capability: Capability,
    session: Option<WsId>,
    claims: Option<Arc<dyn Any + Send + Sync>>,
}

impl Principal {
    /// A caller asking for `capability` — a narrowing of whatever the engine's policy provider
    /// allows, never a widening of it.
    pub fn new(capability: Capability) -> Self {
        Principal {
            capability,
            session: None,
            claims: None,
        }
    }

    /// The workspace this caller is dispatching on.
    pub fn in_session(mut self, session: WsId) -> Self {
        self.session = Some(session);
        self
    }

    /// The embedder's own facts about this caller, carried opaquely.
    pub fn with_claims(mut self, claims: impl Any + Send + Sync) -> Self {
        self.claims = Some(Arc::new(claims));
        self
    }

    /// What this caller asked for.
    pub fn capability(&self) -> &Capability {
        &self.capability
    }

    /// Which workspace it is dispatching on, where it said.
    pub fn session(&self) -> Option<WsId> {
        self.session
    }

    /// The embedder's claims, as `T` — `None` when none were attached, or when they are of
    /// another type.
    pub fn claims<T: Any>(&self) -> Option<&T> {
        self.claims.as_ref()?.downcast_ref()
    }
}

impl fmt::Debug for Principal {
    /// Hand-written because the claims are the embedder's and may hold a token: they are reported
    /// as present or absent and never rendered.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Principal")
            .field("capability", &self.capability)
            .field("session", &self.session)
            .field("claims", &self.claims.is_some())
            .finish()
    }
}

/// A [`PolicyProvider`]'s answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admit {
    Allow,
    Deny(DenyCode),
}

/// Why a [`PolicyProvider`] said no — **a code, never prose**. The engine mints every sentence
/// from [`Form`], so a provider cannot reword a refusal the parity tests pin, and an embedder
/// that logs denials gets a value rather than a string to match on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenyCode {
    /// The caller does not hold the grant this family needs.
    NotGranted,
    /// The caller holds it, but not for this connection — the [`RemoteScope`].
    OutOfScope,
}

/// **The policy seam.** What the engine asks before it classifies a statement, and again before
/// an arm performs one.
///
/// Injected through `EngineBuilder::with_policy`; unset, the engine builds
/// `CapabilityPolicyProvider::new(Capability::full())`, so an engine nobody restricted refuses
/// nothing.
///
/// The contract, which the conformance module in this file asserts for every implementation:
///
/// - **Deterministic within a check.** Two identical asks answer identically; a decision that
///   flips mid-statement would refuse an arm the classifier admitted for no reason the user can
///   see.
/// - **`permit` is never more permissive than `admit`.** The fine phase refines the coarse one;
///   it does not overturn it. The engine fails closed if an implementation breaks this — the arm
///   asks last and its answer stands — so an inconsistency can delay a refusal and never grant
///   one.
/// - **`Err` is a fault, not a decision.** An unreachable policy service is not a pass: the
///   engine surfaces the error and refuses the statement.
///
/// **Refresh and revocation are the implementation's.** A [`Principal`] carries no expiry and the
/// engine caches no answer, so both methods are called per statement: a provider backed by a
/// token or a remote decision point owns its own TTL, and a revocation takes effect at the next
/// call rather than at a renewal the engine would have to schedule. An implementation that cannot
/// answer — expired credentials, an unreachable OPA — returns `Err` and the statement is refused.
#[async_trait]
pub trait PolicyProvider: Send + Sync + 'static {
    /// **Coarse**, at classification: may `who` ever perform `family`, at any locality?
    async fn admit(&self, who: &Principal, family: GrantFamily) -> Result<Admit, String>;

    /// **Fine**, at the arm: may `who` perform `family` against this resolved target?
    async fn permit(
        &self,
        who: &Principal,
        family: GrantFamily,
        target: &TargetFacts,
    ) -> Result<Admit, String>;
}

/// The shipped [`PolicyProvider`]: [`Capability`] data and nothing else.
///
/// Its own capability is a **ceiling** — every answer is about the caller's capability
/// intersected with it, so an engine built read-only cannot be talked out of it while an engine
/// built full is exactly as permissive as its callers ask to be.
#[derive(Clone, Debug)]
pub struct CapabilityPolicyProvider {
    ceiling: Capability,
}

impl CapabilityPolicyProvider {
    /// A provider that allows at most `ceiling`.
    pub fn new(ceiling: Capability) -> Self {
        CapabilityPolicyProvider { ceiling }
    }

    /// What the caller actually gets: its own ask, narrowed by this provider's ceiling.
    fn effective(&self, who: &Principal) -> Capability {
        self.ceiling.intersect(who.capability())
    }
}

#[async_trait]
impl PolicyProvider for CapabilityPolicyProvider {
    async fn admit(&self, who: &Principal, family: GrantFamily) -> Result<Admit, String> {
        Ok(match self.effective(who).admits(family) {
            true => Admit::Allow,
            false => Admit::Deny(DenyCode::NotGranted),
        })
    }

    async fn permit(
        &self,
        who: &Principal,
        family: GrantFamily,
        target: &TargetFacts,
    ) -> Result<Admit, String> {
        Ok(match self.effective(who).permits(family, target) {
            Ok(()) => Admit::Allow,
            Err(code) => Admit::Deny(code),
        })
    }
}

/// **The one refusal message table.** What a policy denial reads as, keyed by what was refused
/// and why.
///
/// The `code` says *why* and the [`Form`] says *what*, and it is the what that the user is owed:
/// a refusal names the statement and the surface that owns the capability, which is actionable,
/// where the code is bookkeeping. So both arms of `code` land on the same sentence today —
/// deliberately, rather than inventing a second wording for a scope refusal nobody has read yet.
/// The arms are spelled out rather than wildcarded so a third code has to come here and decide.
pub(super) fn denied(form: Form, code: DenyCode) -> String {
    match code {
        DenyCode::NotGranted | DenyCode::OutOfScope => refusal_for(form),
    }
    .into()
}

/// The sentence each form is refused with.
///
/// A refused read is reachable only through a provider that denies [`GrantFamily::Read`], which
/// neither preset does; it has no owning surface to point the user at, and says so plainly.
fn refusal_for(form: Form) -> &'static str {
    let kind = match form {
        Form::Read => return "Reading is not permitted",
        Form::Execute => return UNSUPPORTED,
        Form::Statement(kind) => kind,
    };
    match kind {
        StmtKind::CreateExternalTable => {
            "CREATE EXTERNAL TABLE is not supported in the editor. Register tables in Table Config"
        }
        StmtKind::Copy => "COPY TO is not supported in the editor. Use Export",
        StmtKind::Reset => {
            "RESET is not supported in the editor. Engine options are set in Settings"
        }
        StmtKind::CreateView => {
            "CREATE VIEW is not supported in the editor. Write the query and use Save as view"
        }
        StmtKind::DropView => {
            "DROP VIEW is not supported in the editor. Drop views from the catalog"
        }
        StmtKind::DropTable => DROP_UNSUPPORTED,
        StmtKind::CreateTable | StmtKind::Ctas => {
            "CREATE TABLE is not supported in the editor. Register tables in Table Config"
        }
        StmtKind::Insert => "INSERT is not supported in the editor. Load data through Table Config",
        StmtKind::Set => "SET is not supported in the editor. Engine options are set in Settings",
        StmtKind::Prepare
        | StmtKind::Deallocate
        | StmtKind::CreateFunction
        | StmtKind::DropFunction
        | StmtKind::Update
        | StmtKind::Delete => UNSUPPORTED,
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;

    use super::*;

    /// A provider that cannot decide, ever — an unreachable decision point.
    struct Unreachable;

    #[async_trait]
    impl PolicyProvider for Unreachable {
        async fn admit(&self, _: &Principal, _: GrantFamily) -> Result<Admit, String> {
            Err("the policy service is unreachable".into())
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

    /// The clauses every [`PolicyProvider`] keeps, whatever it decides from. `who` is a caller
    /// the provider may or may not allow — the contract is about the shape of the answer, not
    /// about which arm a given provider is in.
    fn conforms(provider: &dyn PolicyProvider, who: &Principal) {
        let targets = [
            TargetFacts::workspace(),
            TargetFacts::remote("postgres", "postgres://acme/orders"),
        ];
        for family in GrantFamily::ALL {
            let coarse = block_on(provider.admit(who, family));
            assert_eq!(
                coarse,
                block_on(provider.admit(who, family)),
                "two asks about {family:?} must agree"
            );
            if let Err(why) = &coarse {
                assert!(!why.trim().is_empty(), "a fault must say what went wrong");
            }
            for target in &targets {
                let fine = block_on(provider.permit(who, family, target));
                assert_eq!(
                    fine,
                    block_on(provider.permit(who, family, target)),
                    "two asks about {family:?} at {target:?} must agree"
                );
                if coarse == Ok(Admit::Deny(DenyCode::NotGranted))
                    || coarse == Ok(Admit::Deny(DenyCode::OutOfScope))
                {
                    assert!(
                        !matches!(fine, Ok(Admit::Allow)),
                        "{family:?} is denied coarsely, so the arm must not permit it at \
                         {target:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_provider_answers_the_same_way_twice_and_never_widens_at_the_arm() {
        for ceiling in [Capability::full(), Capability::read_only()] {
            for ask in [Capability::full(), Capability::read_only()] {
                conforms(
                    &CapabilityPolicyProvider::new(ceiling.clone()),
                    &Principal::new(ask),
                );
            }
        }
        conforms(
            &CapabilityPolicyProvider::new(
                Capability::full().remote_only([RemoteSel::Kind("sqlite".into())]),
            ),
            &Principal::new(Capability::full()),
        );
        conforms(&Unreachable, &Principal::new(Capability::full()));
    }

    /// The presets, as sets — the claim every parity assertion rests on.
    #[test]
    fn the_presets_are_everything_and_reading() {
        let full = Capability::full();
        for grant in Grant::ALL {
            assert!(full.holds(grant), "{grant:?}");
        }
        let read_only = Capability::read_only();
        for grant in Grant::ALL {
            assert_eq!(read_only.holds(grant), grant == Grant::Read, "{grant:?}");
        }
        for family in GrantFamily::ALL {
            assert!(full.admits(family), "{family:?}");
            assert_eq!(
                read_only.admits(family),
                family == GrantFamily::Read,
                "{family:?}"
            );
        }
    }

    /// **A caller narrows and never widens.** The whole reason one engine can serve a full editor
    /// and a read-only agent — and the reason a read-only engine cannot be talked out of it.
    #[test]
    fn a_callers_capability_only_ever_narrows_the_providers() {
        let restrictive = CapabilityPolicyProvider::new(Capability::read_only());
        let asking_for_everything = Principal::new(Capability::full());
        assert_eq!(
            block_on(restrictive.admit(&asking_for_everything, GrantFamily::Write)),
            Ok(Admit::Deny(DenyCode::NotGranted))
        );

        let permissive = CapabilityPolicyProvider::new(Capability::full());
        assert_eq!(
            block_on(permissive.admit(&asking_for_everything, GrantFamily::Write)),
            Ok(Admit::Allow)
        );
        assert_eq!(
            block_on(
                permissive.admit(&Principal::new(Capability::read_only()), GrantFamily::Write)
            ),
            Ok(Admit::Deny(DenyCode::NotGranted))
        );
    }

    /// The remote scope refines the remote grants and leaves reading alone — the RDS scenario,
    /// checked from both sides.
    #[test]
    fn a_remote_scope_names_the_connections_a_write_may_reach() {
        let capability = Capability::read_only()
            .with(Grant::Write(Locality::Remote))
            .remote_only([RemoteSel::Kind("sqlite".into())]);
        let provider = CapabilityPolicyProvider::new(Capability::full());
        let who = Principal::new(capability);

        let sqlite = TargetFacts::remote("sqlite", "sqlite:///tmp/local.db");
        let rds = TargetFacts::remote("postgres", "postgres://rds/orders");
        assert_eq!(
            block_on(provider.permit(&who, GrantFamily::Write, &sqlite)),
            Ok(Admit::Allow)
        );
        assert_eq!(
            block_on(provider.permit(&who, GrantFamily::Write, &rds)),
            Ok(Admit::Deny(DenyCode::OutOfScope))
        );
        assert_eq!(
            block_on(provider.permit(&who, GrantFamily::Read, &rds)),
            Ok(Admit::Allow),
            "a scope never refuses a read"
        );
        assert_eq!(
            block_on(provider.permit(&who, GrantFamily::Write, &TargetFacts::workspace())),
            Ok(Admit::Deny(DenyCode::NotGranted)),
            "and it holds no local write at all"
        );
    }

    /// A connection selector names one url; the kind selector names a backend.
    #[test]
    fn a_connection_selector_names_one_connection() {
        let capability = Capability::full()
            .remote_only([RemoteSel::Connection("postgres://acme/orders".into())]);
        let provider = CapabilityPolicyProvider::new(Capability::full());
        let who = Principal::new(capability);
        assert_eq!(
            block_on(provider.permit(
                &who,
                GrantFamily::Ddl,
                &TargetFacts::remote("postgres", "postgres://acme/orders")
            )),
            Ok(Admit::Allow)
        );
        assert_eq!(
            block_on(provider.permit(
                &who,
                GrantFamily::Ddl,
                &TargetFacts::remote("postgres", "postgres://acme/warehouse")
            )),
            Ok(Admit::Deny(DenyCode::OutOfScope))
        );
    }

    /// A remote view is the server's schema changing, so it needs the connection's DDL grant and
    /// not the workspace's view grant.
    #[test]
    fn a_remote_view_needs_the_remote_ddl_grant() {
        let capability = Capability::read_only().with(Grant::ViewDdl);
        let provider = CapabilityPolicyProvider::new(Capability::full());
        let who = Principal::new(capability);
        assert_eq!(
            block_on(provider.permit(&who, GrantFamily::ViewDdl, &TargetFacts::workspace())),
            Ok(Admit::Allow)
        );
        assert_eq!(
            block_on(provider.permit(
                &who,
                GrantFamily::ViewDdl,
                &TargetFacts::remote("postgres", "postgres://acme/orders")
            )),
            Ok(Admit::Deny(DenyCode::NotGranted))
        );
    }

    /// Every form has a family, and the two read forms differ — which is what keeps `EXECUTE`
    /// refused on a read-only surface that cannot `PREPARE`.
    #[test]
    fn a_read_and_an_execute_are_different_families() {
        assert_eq!(Form::Read.family(), GrantFamily::Read);
        assert_eq!(Form::Execute.family(), GrantFamily::Session);
        assert_eq!(
            Form::Statement(StmtKind::Ctas).family(),
            GrantFamily::Write,
            "a CTAS writes rows; a column-list CREATE TABLE only shapes a catalog"
        );
        assert_eq!(
            Form::Statement(StmtKind::CreateTable).family(),
            GrantFamily::Ddl
        );
    }

    /// **The agent path's messages, pinned verbatim.** These variants are unreachable from a full
    /// capability, so a future task rewording one would silently change the agent surface with
    /// every other test green. `strata-agent`'s own parity tests cannot catch it either: they
    /// compare `AgentError`'s rendering against this table, so both sides would move together.
    /// These are the literals.
    #[test]
    fn the_agent_paths_messages_are_pinned_verbatim() {
        for (form, message) in [
            (
                Form::Statement(StmtKind::CreateExternalTable),
                "CREATE EXTERNAL TABLE is not supported in the editor. Register tables in \
                 Table Config",
            ),
            (
                Form::Statement(StmtKind::Copy),
                "COPY TO is not supported in the editor. Use Export",
            ),
            (
                Form::Statement(StmtKind::Reset),
                "RESET is not supported in the editor. Engine options are set in Settings",
            ),
            (
                Form::Statement(StmtKind::CreateView),
                "CREATE VIEW is not supported in the editor. Write the query and use Save as view",
            ),
            (
                Form::Statement(StmtKind::DropView),
                "DROP VIEW is not supported in the editor. Drop views from the catalog",
            ),
            (
                Form::Statement(StmtKind::DropTable),
                "DROP is not supported in the editor. Deregister tables from the catalog",
            ),
            (
                Form::Statement(StmtKind::CreateTable),
                "CREATE TABLE is not supported in the editor. Register tables in Table Config",
            ),
            (
                Form::Statement(StmtKind::Ctas),
                "CREATE TABLE is not supported in the editor. Register tables in Table Config",
            ),
            (
                Form::Statement(StmtKind::Insert),
                "INSERT is not supported in the editor. Load data through Table Config",
            ),
            (
                Form::Statement(StmtKind::Set),
                "SET is not supported in the editor. Engine options are set in Settings",
            ),
            (
                Form::Statement(StmtKind::Prepare),
                "This statement is not supported in the editor. Only SELECT, EXPLAIN, SHOW and \
                 DESCRIBE can run here",
            ),
            (
                Form::Execute,
                "This statement is not supported in the editor. Only SELECT, EXPLAIN, SHOW and \
                 DESCRIBE can run here",
            ),
        ] {
            assert_eq!(denied(form, DenyCode::NotGranted), message, "{form:?}");
            assert_eq!(
                denied(form, DenyCode::OutOfScope),
                message,
                "and the code is bookkeeping, not a second wording: {form:?}"
            );
        }
    }
}
