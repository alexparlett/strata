//! **The statement layer** — what the engine does with one statement, and who may ask for it.
//!
//! Three files, one responsibility each:
//!
//! - [`pipeline`] — the typed stages (`Parsed → Qualified → Admitted`) and [`accept`], the one
//!   composition site every surface enters.
//! - [`classify`] — the grammar: what a parsed statement *is*, and the faults no capability makes
//!   well-formed.
//! - [`grants`] — the policy seam and its shipped data model, plus the one table every refusal is
//!   worded from.
//!
//! Statement *execution* is `engine::ddl`; the vocabulary it switches on — [`StmtKind`] — is
//! [`classify`]'s.

pub mod classify;
pub mod grants;
pub mod pipeline;

pub use classify::{classify_stmt, Classified, Fault, Form, StmtKind};
pub use grants::{
    Admit, Capability, CapabilityPolicyProvider, DenyCode, Grant, GrantFamily, Grants, Locality,
    PolicyProvider, Principal, RemoteScope, RemoteSel, TargetFacts,
};
pub use pipeline::{
    accept, Admitted, Parsed, Pipeline, PolicyRefusal, Qualified, Refusal, Refused,
};
