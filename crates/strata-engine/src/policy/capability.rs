//! Grants as data, and the [`PolicyProvider`] that decides from them.
//!
//! A [`Capability`] is a set of [`Grant`]s over a [`Locality`] axis, narrowed for remote targets
//! by a [`RemoteScope`]. Start from [`Capability::full`] or [`Capability::read_only`] and compose.
//!
//! [`CapabilityPolicyProvider`] holds a capability of its own and allows only what both it and the
//! caller allow, so an engine built read-only stays read-only whatever a caller asks for.

use std::collections::BTreeSet;

use async_trait::async_trait;

use super::{Admit, DenyCode, GrantFamily, Locality, PolicyProvider, Principal, TargetFacts};

/// One thing a caller may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Grant {
    /// Reading, of any source.
    Read,
    /// `INSERT` and `CREATE TABLE AS SELECT`.
    Write(Locality),
    /// `CREATE TABLE`, `DROP TABLE`, `CREATE EXTERNAL TABLE`, `UPDATE` and `DELETE`.
    ///
    /// A remote view needs this at [`Locality::Remote`], since there it is the server's schema
    /// that changes.
    Ddl(Locality),
    /// `CREATE VIEW` and `DROP VIEW` over the workspace.
    ViewDdl,
    /// `COPY … TO` and export. Never narrowed by a [`RemoteScope`], the target being a file.
    CopyOut,
    /// `SET`, `RESET`, `PREPARE`, `EXECUTE` and `DEALLOCATE`.
    Session,
    /// `CREATE FUNCTION` and `DROP FUNCTION`.
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

    /// This grant's bit.
    ///
    /// Hand-rolled rather than a bitflags dependency: the payload means the enum cannot be
    /// `as`-cast into an index.
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

    /// Whether a [`RemoteScope`] refines this grant.
    ///
    /// True for the remote-locality grants only. Reading is never narrowed by a scope.
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
}

/// Which data sources a capability's remote grants reach.
///
/// Applied by [`PolicyProvider::permit`] to the grants carrying [`Locality::Remote`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteScope {
    /// Every data source.
    All,
    /// Only these.
    Only(BTreeSet<RemoteSel>),
}

impl RemoteScope {
    /// Whether this scope reaches the data source `facts` names.
    ///
    /// A target carrying neither a kind nor a name matches nothing but [`All`](RemoteScope::All).
    fn reaches(&self, facts: &TargetFacts) -> bool {
        match self {
            RemoteScope::All => true,
            RemoteScope::Only(selectors) => selectors.iter().any(|sel| match sel {
                RemoteSel::Kind(kind) => facts.kind.as_deref() == Some(kind.as_str()),
                RemoteSel::Source(name) => facts.source.as_deref() == Some(name.as_str()),
            }),
        }
    }
}

/// One way of naming data sources in a [`RemoteScope`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RemoteSel {
    /// Every data source of one backend kind (`"postgres"`, `"mysql"`).
    Kind(String),
    /// One data source, by the exact name it is keyed under.
    Source(String),
}

/// What a caller may do.
///
/// Checked in two phases: [`PolicyProvider::admit`] asks whether the caller holds a family at any
/// locality, and [`PolicyProvider::permit`] asks the same of a resolved target, applying the
/// [`Locality`] on each grant and the [`RemoteScope`] narrowing the remote ones.
///
/// # Example
///
/// A client that may read anything and write only the sqlite data sources:
///
/// ```
/// use strata_engine::{Capability, Grant, Locality, RemoteSel};
///
/// let client = Capability::read_only()
///     .with(Grant::Write(Locality::Remote))
///     .remote_only([RemoteSel::Kind("sqlite".into())]);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capability {
    grants: Grants,
    remote: RemoteScope,
}

impl Capability {
    /// Every grant, over every data source.
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
    ///
    /// Narrows the grants carrying [`Locality::Remote`] only. Reading is never narrowed.
    pub fn remote_only(mut self, selectors: impl IntoIterator<Item = RemoteSel>) -> Self {
        self.remote = RemoteScope::Only(selectors.into_iter().collect());
        self
    }

    /// Returns whether this capability holds `grant`.
    pub fn holds(&self, grant: Grant) -> bool {
        self.grants.holds(grant)
    }

    /// Whether this capability may perform `family` at any locality.
    ///
    /// Derived from the same grant table the fine check uses, so the two cannot disagree.
    fn admits(&self, family: GrantFamily) -> bool {
        [Locality::Local, Locality::Remote]
            .into_iter()
            .any(|locality| self.grants.holds(grant_for(family, locality)))
    }

    /// Whether this capability may perform `family` against `facts`.
    ///
    /// Two gates in order: the grant the family needs at that locality, then the remote scope. The
    /// returned code says which one refused.
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
/// The one derivation, so a caller never names a grant and cannot check the wrong one.
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

    /// The two capabilities every answer is the conjunction of.
    fn both<'a>(&'a self, who: &'a Principal) -> [&'a Capability; 2] {
        [&self.ceiling, who.capability()]
    }
}

#[async_trait]
impl PolicyProvider for CapabilityPolicyProvider {
    async fn admit(&self, who: &Principal, family: GrantFamily) -> Result<Admit, String> {
        Ok(match self.both(who).iter().all(|c| c.admits(family)) {
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
        for capability in self.both(who) {
            if let Err(code) = capability.permits(family, target) {
                return Ok(Admit::Deny(code));
            }
        }
        Ok(Admit::Allow)
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;

    use super::*;
    use crate::policy::conformance::conforms;

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
    fn a_remote_scope_names_the_sources_a_write_may_reach() {
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

    /// A data source selector names one url; the kind selector names a backend.
    #[test]
    fn a_source_selector_names_one_source() {
        let capability =
            Capability::full().remote_only([RemoteSel::Source("postgres://acme/orders".into())]);
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

    /// **The coarse phase is only equivalent to the full check while a capability's localities
    /// agree, and every capability this crate ships is one where they do.**
    ///
    /// `admits` answers "at *any* locality", since a target is not resolved at classification;
    /// `permits` asks the narrower question. So a capability holding `Ddl(Remote)` and not
    /// `Ddl(Local)` is admitted for a workspace `CREATE TABLE` that only `permits` refuses.
    ///
    /// This is what says the gap is unreachable through anything shipped, and it is not luck:
    /// `full()` holds every locality of every grant and `read_only()` holds none, so for both the
    /// coarse answer *is* what the arm would give. Add a third preset that differentiates by
    /// locality and this fails — which is the point, because it would have to be wired at the arm
    /// before it meant anything.
    #[test]
    fn every_shipped_preset_is_locality_symmetric() {
        let remote = TargetFacts::remote("postgres", "postgres://acme/orders");
        for capability in [Capability::full(), Capability::read_only()] {
            for family in GrantFamily::ALL {
                let local = capability
                    .permits(family, &TargetFacts::workspace())
                    .is_ok();
                assert_eq!(
                    local,
                    capability.permits(family, &remote).is_ok(),
                    "{family:?} differs by locality, so the coarse phase would over-admit it"
                );
                assert_eq!(
                    capability.admits(family),
                    local,
                    "so the coarse answer must be exactly what the arm would give for {family:?}"
                );
            }
        }
    }

    /// **Two selectors can name the same data source**, so the ceiling and the caller are asked
    /// separately rather than merged. A `RemoteScope` has no lossless intersection: merging
    /// `Kind("postgres")` with `Data source("postgres://acme/orders")` by selector equality yields
    /// the empty set, and would refuse the one data source both operands reach.
    #[test]
    fn a_kind_ceiling_and_a_source_ask_still_reach_the_source_both_allow() {
        let provider = CapabilityPolicyProvider::new(
            Capability::full().remote_only([RemoteSel::Kind("postgres".into())]),
        );
        let who = Principal::new(
            Capability::full().remote_only([RemoteSel::Source("postgres://acme/orders".into())]),
        );
        assert_eq!(
            block_on(provider.permit(
                &who,
                GrantFamily::Write,
                &TargetFacts::remote("postgres", "postgres://acme/orders")
            )),
            Ok(Admit::Allow),
            "the ceiling reaches it by kind and the caller by url, so both allow it"
        );
        assert_eq!(
            block_on(provider.permit(
                &who,
                GrantFamily::Write,
                &TargetFacts::remote("postgres", "postgres://acme/warehouse")
            )),
            Ok(Admit::Deny(DenyCode::OutOfScope)),
            "and the caller's own narrowing still bites"
        );
        assert_eq!(
            block_on(provider.permit(
                &who,
                GrantFamily::Write,
                &TargetFacts::remote("mysql", "mysql://acme/orders")
            )),
            Ok(Admit::Deny(DenyCode::OutOfScope)),
            "as does the ceiling's"
        );
    }

    /// A remote view is the server's schema changing, so it needs the data source's DDL grant and
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
