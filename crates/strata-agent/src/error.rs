//! The error taxonomy (`docs/AGENT_ACCESS_SPEC.md`, "Error taxonomy") — every fault an agent can see,
//! typed once and rendered once.
//!
//! Two absences are the design:
//!
//! - **No `Stopped` variant.** A cancel or a supersede is not a fault: `run` reports the engine's
//!   three strings as [`RunResult::Stopped`](crate::wire::RunResult::Stopped), and a copy here
//!   would be the third of a rule that has already drifted twice.
//! - **No `Unauthorized` variant.** A bad token is answered with HTTP 401 by [`crate::server`],
//!   before any tool runs.
//!
//! Everything here becomes an `isError` tool result rather than a JSON-RPC protocol error: these
//! are conditions the agent should read and recover from, not malformed requests.

use std::error::Error;
use std::fmt;

use rmcp::handler::server::tool::IntoCallToolResult;
use rmcp::model::{CallToolResponse, CallToolResult, ContentBlock};
use rmcp::ErrorData;
use strata_core::engine::sql::PolicyRefusal;

use crate::host::{Project, QuerySessionId};

/// What a [`AgentError::Policy`] with no refusals in it says. Not a state the tool layer can
/// produce; a state the type permits, and a refusal with no reason is unactionable.
const UNJUDGED: &str = "The statement was refused, but no reason was recorded.";

/// One of the taxonomy's classes. The `Display` is what the agent reads.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentError {
    /// Blocked DDL/DML. Carries the refusals so the message is the **editor's own**, from
    /// `Blocked::editor_message()` — one predicate, two surfaces, zero copies (AA-01).
    Policy(Vec<PolicyRefusal>),
    /// The engine's `Err` from a real fault, unedited — it already reads like an IDE's.
    /// Also the home of "this did not parse": the gate fails closed on unjudgeable input
    /// and surfaces the engine's own parse wording, the same terminal a Run would reach.
    Query(String),
    /// `read_page` against a snapshot a newer run retired.
    ResultMoved,
    /// Unknown query-session handle, table name, or column path. A plain statement naming
    /// the recovery: the listing tool for a handle or a table; for a column path,
    /// `describe_table`'s own 'matching' — there is no listing tool behind a schema.
    NotFound(String),
    /// More than one project is open and the call named none (or named a colliding name).
    Ambiguous(Vec<Project>),
    /// Nothing is open to address.
    NoProject,
    /// The bridge went while the ask was out — the window closed, or re-rooted.
    WindowGone,
}

impl AgentError {
    /// The one wording for a query-session handle this agent has nothing open under.
    ///
    /// Here rather than at each site because it was written four times across two crates — the
    /// tool layer, the mock and the app's own driver — and `list_query_sessions` being *the*
    /// recovery from this condition only works if every host states it the same way
    /// (AGENTS.md §3: merge near-duplicate messages rather than stack them).
    ///
    /// It is also the answer to a handle belonging to a *different* agent, deliberately: a
    /// distinct "that is not yours" would confirm the session exists, which is a fact an
    /// agent has no business learning and no way to act on.
    pub fn no_such_query_session(session: QuerySessionId) -> AgentError {
        AgentError::NotFound(format!("No open query session '{}'.", session.0))
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::Policy(refusals) => match &refusals[..] {
                [] => f.write_str(UNJUDGED),
                [one] => f.write_str(&one.blocked.editor_message()),
                many => {
                    let mut first = true;
                    for r in many {
                        if !first {
                            writeln!(f)?;
                        }
                        first = false;
                        write!(
                            f,
                            "Statement {}: {}",
                            r.index + 1,
                            r.blocked.editor_message()
                        )?;
                    }
                    Ok(())
                }
            },
            AgentError::Query(message) => f.write_str(message),
            AgentError::ResultMoved => {
                f.write_str("The query session's result was replaced; re-run to read it.")
            }
            AgentError::NotFound(message) => f.write_str(message),
            AgentError::Ambiguous(projects) => {
                f.write_str("More than one project is open. Pass 'project' to choose one:")?;
                for p in projects {
                    write!(f, "\n  {} ({})", p.name, p.root.display())?;
                }
                Ok(())
            }
            AgentError::NoProject => f.write_str("No project is open."),
            AgentError::WindowGone => f.write_str("The project window closed."),
        }
    }
}

impl Error for AgentError {}

impl IntoCallToolResult for AgentError {
    fn into_call_tool_result(self) -> Result<CallToolResponse, ErrorData> {
        Ok(CallToolResult::error(vec![ContentBlock::text(self.to_string())]).into())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use strata_core::engine::sql::Blocked;

    use super::*;

    fn refusal(index: usize, blocked: Blocked) -> PolicyRefusal {
        PolicyRefusal {
            index,
            statement: String::new(),
            blocked,
        }
    }

    /// The whole point of carrying `Blocked` rather than a string: the agent reads exactly
    /// what the editor squiggles.
    #[test]
    fn a_single_refusal_is_the_editors_message_verbatim() {
        let e = AgentError::Policy(vec![refusal(0, Blocked::CreateTable)]);
        assert_eq!(e.to_string(), Blocked::CreateTable.editor_message());
    }

    #[test]
    fn several_refusals_are_indexed_from_one() {
        let e = AgentError::Policy(vec![
            refusal(0, Blocked::Insert),
            refusal(2, Blocked::CreateDatabase),
        ]);
        assert_eq!(
            e.to_string(),
            format!(
                "Statement 1: {}\nStatement 3: {}",
                Blocked::Insert.editor_message(),
                Blocked::CreateDatabase.editor_message()
            )
        );
    }

    /// A refusal with nothing in it is unreachable through the tool layer and expressible in
    /// the type, so it has to say *something* — a blank error is worse than a vague one.
    #[test]
    fn a_refusal_with_no_reason_still_reads_as_something() {
        assert_eq!(AgentError::Policy(Vec::new()).to_string(), UNJUDGED);
    }

    #[test]
    fn ambiguity_lists_the_open_projects() {
        let e = AgentError::Ambiguous(vec![
            Project {
                name: "sales".into(),
                root: PathBuf::from("/w/sales"),
            },
            Project {
                name: "ops".into(),
                root: PathBuf::from("/w/ops"),
            },
        ]);
        let text = e.to_string();
        assert!(text.contains("sales (/w/sales)"), "{text}");
        assert!(text.contains("ops (/w/ops)"), "{text}");
    }
}
