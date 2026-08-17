//! The shipped [`PolicyProvider`]: grants as data.
//!
//! A [`Capability`] is a bitset of [`Grant`]s over a [`Locality`] axis, plus a [`RemoteScope`]
//! refining the remote half. Two presets carry the app — [`Capability::full`] for an editor,
//! [`Capability::read_only`] for an agent — and everything else is composed from them.
//!
//! [`CapabilityPolicyProvider`]'s own capability is a **ceiling**: every answer is about the
//! caller's capability intersected with it, so an engine built read-only stays read-only whatever
//! a caller asks for, while an engine built full is exactly as permissive as its callers ask to
//! be. That is what lets one engine serve a full editor and a read-only agent at once.

use std::collections::BTreeSet;

use async_trait::async_trait;

use super::{Admit, DenyCode, GrantFamily, Locality, PolicyProvider, Principal, TargetFacts};

/// One thing a caller may do, factored by action and locality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Grant {
    /// Reading, of any source. Held by every preset: reading is what a connection is for, and a
    /// policy that refuses it refuses the product.
    Read,
    /// `INSERT` and `CREATE TABLE AS SELECT`, into a workspace internal table or a connection that
    /// opted in.
    Write(Locality),
    /// Table DDL: the workspace's `CREATE` / `DROP` / `CREATE EXTERNAL TABLE`, and the statements
    /// only a server can run. `UPDATE` and `DELETE` are among them, both being refused a workspace
    /// target outright, so the only data they can reach is a server's own.
    Ddl(Locality),
    /// Workspace views. Split from [`Ddl`](Grant::Ddl) because "may save views, may not reshape
    /// tables" is a plausible policy; a remote view is `Ddl(Remote)`, since there it is the
    /// server's schema that changes.
    ViewDdl,
    /// `COPY … TO` and export. Scope-free: it is file egress, and the file is in no catalog.
    CopyOut,
    /// `SET` / `RESET` and `PREPARE` / `EXECUTE` / `DEALLOCATE`: engine-local session state.
    Session,
    /// `CREATE FUNCTION` and `DROP FUNCTION`. Engine-local, DataFusion refusing a qualified
    /// function name itself, so there is no remote half to have.
    Functions,
}

impl Grant {
    /// Every grant there is.
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

    /// Whether a [`RemoteScope`] refines this grant: true exactly for the remote-locality ones.
    /// [`Read`](Grant::Read) is not among them, or "reading is never refused per source" would be
    /// a claim the type contradicts.
    fn scoped(self) -> bool {
        matches!(
            self,
            Grant::Write(Locality::Remote) | Grant::Ddl(Locality::Remote)
        )
    }
}

/// The grants a [`Capability`] holds.
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

    /// Returns this set plus `grant`.
    pub fn with(self, grant: Grant) -> Self {
        Grants(self.0 | grant.bit())
    }

    /// Returns whether `grant` is in this set.
    pub fn holds(self, grant: Grant) -> bool {
        self.0 & grant.bit() != 0
    }

    /// Returns the grants both sets hold.
    pub fn intersect(self, other: Grants) -> Self {
        Grants(self.0 & other.0)
    }
}

/// Which database connections a capability's remote grants reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteScope {
    /// Every connection.
    All,
    /// Only these.
    Only(BTreeSet<RemoteSel>),
}

impl RemoteScope {
    /// Whether this scope reaches the connection `facts` names. A remote target with neither a
    /// kind nor a url matches nothing but [`All`](RemoteScope::All): an unidentified connection is
    /// not one a selector can have been written for.
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

    /// Returns this capability plus `grant`.
    pub fn with(mut self, grant: Grant) -> Self {
        self.grants = self.grants.with(grant);
        self
    }

    /// Returns this capability with its remote grants narrowed to `selectors`.
    pub fn remote_only(mut self, selectors: impl IntoIterator<Item = RemoteSel>) -> Self {
        self.remote = RemoteScope::Only(selectors.into_iter().collect());
        self
    }

    /// Returns what both capabilities allow.
    ///
    /// Never a union: a caller cannot widen what the embedder built.
    pub fn intersect(&self, other: &Capability) -> Capability {
        Capability {
            grants: self.grants.intersect(other.grants),
            remote: self.remote.intersect(&other.remote),
        }
    }

    /// Returns whether this capability holds `grant`.
    pub fn holds(&self, grant: Grant) -> bool {
        self.grants.holds(grant)
    }

    /// The coarse answer: may this capability perform `family` at any locality? Derived from the
    /// fine check rather than tabulated beside it, so the two cannot disagree.
    fn admits(&self, family: GrantFamily) -> bool {
        [Locality::Local, Locality::Remote]
            .into_iter()
            .any(|locality| self.grants.holds(grant_for(family, locality)))
    }

    /// The fine answer, through two named gates in order: the grant the family needs at that
    /// locality, then the remote scope. The code says which one refused, so an embedder's log can
    /// tell "may not write at all" from "may not write *this* connection".
    fn permits(&self, family: GrantFamily, facts: &TargetFacts) -> Result<(), DenyCode> {
        let grant = grant_for(family, facts.locality);
        if !self.grants.holds(grant) {
            return Err(DenyCode::NotGranted);
        }
        match !grant.scoped() || self.remote.reaches(facts) {
            true => Ok(()),
            false => Err(DenyCode::OutOfScope),
        }
    }
}

/// The grant `family` needs at `locality`.
///
/// The one derivation, so an arm never names a grant and cannot check the wrong one. A remote view
/// is the server's schema changing, so it takes that connection's DDL grant rather than the
/// workspace's view grant.
fn grant_for(family: GrantFamily, locality: Locality) -> Grant {
    match family {
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

/// A [`PolicyProvider`] that decides from [`Capability`] data alone.
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

    /// The clauses every [`PolicyProvider`] keeps, whatever it decides from. `who` is a caller the
    /// provider may or may not allow — the contract is about the shape of the answer, not about
    /// which arm a given provider is in.
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
                if matches!(coarse, Ok(Admit::Deny(_))) {
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
}
