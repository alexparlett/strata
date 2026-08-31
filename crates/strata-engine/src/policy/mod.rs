//! Who may perform what.
//!
//! [`PolicyProvider`] is the seam: an injected async trait the engine asks once per statement,
//! answering in [`DenyCode`]s rather than prose so the engine mints every refusal itself. A
//! service deployment (Cognito, AD, OPA) implements it and decides at check time.
//!
//! [`capability`] is the shipped default: grants over a local/remote axis, as data. An engine
//! built with no policy allows everything, so restriction is something an embedder says rather
//! than something it has to switch off.
//!
//! Nothing here knows what a statement is. The statement layer maps its own forms onto
//! [`GrantFamily`] and words every refusal; this module answers about callers and targets.

pub mod capability;

pub use capability::{Capability, CapabilityPolicyProvider, Grant, Grants, RemoteScope, RemoteSel};

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::WsId;

/// Where a statement's target lives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Locality {
    /// The workspace catalog: a project table, view or internal table.
    #[default]
    Local,
    /// A relation inside a data source's catalog.
    Remote,
}

/// An action, with the locality taken out.
///
/// What [`PolicyProvider::admit`] asks about, before anything has resolved a target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantFamily {
    /// Reading.
    Read,
    /// Writing rows.
    Write,
    /// Creating and dropping tables.
    Ddl,
    /// Creating and dropping views.
    ViewDdl,
    /// Writing a file with `COPY … TO`.
    CopyOut,
    /// Moving the session — `SET`, `RESET`, `PREPARE`, `DEALLOCATE`.
    Session,
    /// Creating and dropping functions.
    Functions,
}

impl GrantFamily {
    /// Every family, in order.
    pub const ALL: [GrantFamily; 7] = [
        GrantFamily::Read,
        GrantFamily::Write,
        GrantFamily::Ddl,
        GrantFamily::ViewDdl,
        GrantFamily::CopyOut,
        GrantFamily::Session,
        GrantFamily::Functions,
    ];
}

/// What a policy decision may turn on about a statement's resolved target.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TargetFacts {
    /// Where the target lives.
    pub locality: Locality,
    /// The backend kind of the data source the target is in; `None` for a workspace target.
    pub kind: Option<String>,
    /// The data source's name, the key it is registered under; `None` for a workspace target.
    pub source: Option<String>,
}

impl TargetFacts {
    /// A target in the workspace catalog.
    pub fn workspace() -> Self {
        TargetFacts::default()
    }

    /// A relation inside the data source `source`, of kind `kind`.
    pub fn remote(kind: impl Into<String>, source: impl Into<String>) -> Self {
        TargetFacts {
            locality: Locality::Remote,
            kind: Some(kind.into()),
            source: Some(source.into()),
        }
    }
}

/// Who is asking.
///
/// The capability a principal carries only ever narrows what the provider allows, so one engine
/// can serve callers of differing authority without any of them promoting itself. Claims are the
/// embedder's own facts about the caller and are never read here.
#[derive(Clone)]
pub struct Principal {
    capability: Capability,
    session: Option<WsId>,
    claims: Option<Arc<dyn Any + Send + Sync>>,
}

impl Principal {
    /// A caller asking for `capability`.
    pub fn new(capability: Capability) -> Self {
        Principal {
            capability,
            session: None,
            claims: None,
        }
    }

    /// Sets the workspace this caller is dispatching on, defaults to unset.
    pub fn in_session(mut self, session: WsId) -> Self {
        self.session = Some(session);
        self
    }

    /// Sets the embedder's own facts about this caller, defaults to none.
    pub fn with_claims(mut self, claims: impl Any + Send + Sync) -> Self {
        self.claims = Some(Arc::new(claims));
        self
    }

    /// Returns what this caller asked for.
    pub fn capability(&self) -> &Capability {
        &self.capability
    }

    /// Returns which workspace it is dispatching on, where it said.
    pub fn session(&self) -> Option<WsId> {
        self.session
    }

    /// Returns the embedder's claims as `T`, or `None` when none were attached or they are of
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

/// Whether a caller may proceed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admit {
    /// They may.
    Allow,
    /// They may not, for this reason.
    Deny(DenyCode),
}

/// Why a [`PolicyProvider`] said no.
///
/// A code rather than prose, so the engine words every refusal and a caller logging denials has a
/// value to match on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenyCode {
    /// The caller does not hold the grant this family needs.
    NotGranted,
    /// The caller holds it, but not for this data source.
    OutOfScope,
}

/// Decides what a caller may do.
///
/// Set with `EngineBuilder::with_policy`, defaulting to
/// `CapabilityPolicyProvider::new(Capability::full())`.
///
/// An implementation must keep three clauses:
///
/// - Two identical asks answer identically.
/// - [`permit`](Self::permit) is never more permissive than [`admit`](Self::admit).
/// - An `Err` refuses the statement. It is never read as an allow.
///
/// Both methods are called once per statement and no answer is cached, so an implementation that
/// consults a token or a remote service owns its own caching, and a revocation takes effect at the
/// next call.
#[async_trait]
pub trait PolicyProvider: Send + Sync + 'static {
    /// Returns whether `who` may ever perform `family`, at any locality.
    ///
    /// # Errors
    ///
    /// The implementation could not decide. The engine surfaces the message and refuses the
    /// statement.
    async fn admit(&self, who: &Principal, family: GrantFamily) -> Result<Admit, String>;

    /// Returns whether `who` may perform `family` against a resolved target.
    ///
    /// Refines [`admit`](Self::admit) once the statement's target is known, and is never more
    /// permissive than it.
    ///
    /// # Errors
    ///
    /// As [`admit`](Self::admit).
    async fn permit(
        &self,
        who: &Principal,
        family: GrantFamily,
        target: &TargetFacts,
    ) -> Result<Admit, String>;
}
