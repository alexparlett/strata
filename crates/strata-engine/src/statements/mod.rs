//! What the engine does with one statement.
//!
//! - [`pipeline`] — the typed stages (`Parsed → Qualified → Admitted`) and [`accept`], which
//!   composes all three for one statement.
//! - [`classify`] — the grammar: what a parsed statement is, the faults no capability makes
//!   well-formed, and the sentence every refusal is worded from.
//!
//! Who may perform what is [`crate::policy`]; this module maps its own forms onto that vocabulary
//! ([`Form::family`]) and never the other way round. Statement execution is `engine::ddl`, which
//! switches on [`StmtKind`].

pub mod classify;
pub mod pipeline;

pub use classify::{classify_stmt, Classified, Fault, Form, StmtKind};
pub use pipeline::{accept, Admitted, Parsed, Pipeline, PolicyRefusal, Qualified, Reason, Refusal};
