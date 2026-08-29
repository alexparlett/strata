//! What the engine does with one statement.
//!
//! [`pipeline`] turns SQL text into an accepted statement; [`classify`] says what a parsed
//! statement is. Who may perform it is [`crate::policy`], which this module maps its own forms
//! onto through [`Form::family`].
//!
//! Past admission the statement is dispatched: [`target`] resolves the relation it manages,
//! [`mechanism`](mod@mechanism) says how a kind reaches one that is not the workspace's, [`ctx`]
//! is what the engine hands every arm, [`arms`] performs it and [`report`] is what comes back.
//! [`copy_job`] is the one write path a `COPY` — typed or composed by the Export window — is
//! gated and driven through.

pub mod arms;
pub mod classify;
pub(crate) mod copy_job;
pub mod ctx;
pub mod mechanism;
pub mod pipeline;
pub mod report;
pub mod target;

pub use classify::{classify_stmt, Classified, Fault, Form, StmtKind};
pub use ctx::{DataRoot, StmtCtx};
pub use mechanism::{mechanism, Mechanism};
pub use pipeline::{accept, Admitted, Parsed, Pipeline, PolicyRefusal, Qualified, Reason, Refusal};
pub use report::{StatementOutcome, StatementReport, StoreEffect};
pub use target::{resolve_target, Remote, Target};
