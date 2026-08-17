//! What the engine does with one statement.
//!
//! [`pipeline`] turns SQL text into an accepted statement; [`classify`] says what a parsed
//! statement is. Who may perform it is [`crate::policy`], which this module maps its own forms
//! onto through [`Form::family`].

pub mod classify;
pub mod pipeline;

pub use classify::{classify_stmt, Classified, Fault, Form, StmtKind};
pub use pipeline::{accept, Admitted, Parsed, Pipeline, PolicyRefusal, Qualified, Reason, Refusal};
