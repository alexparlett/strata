//! **Name to method** — the binding between what a model answers with and the vocabulary.
//!
//! A model replies with a tool *name* and a JSON object; [`StrataTools`] offers eleven typed
//! methods. Turning one into the other needs a message for bad arguments that reads well *to a
//! model*, which is why this lives here rather than in AS-01: a crate with no provider in it
//! has no register to write that message in.
//!
//! **It is one match, and a test walks the manifest through it** — so a tool added to the
//! router cannot reach the model with no arm behind it. Deliberately *not* a second tool trait
//! or a name-keyed registry: rmcp's own `ToolRouter` already is that registry (it is what
//! [`StrataTools::manifest`] reads), its dispatch path needs a live `Peer` the in-process
//! caller does not have, and it answers in content blocks rather than typed values. The AS-01
//! file records that survey.
//!
//! Two things happen here that do not happen on the MCP path:
//!
//! - **Every call is bound to the window's project.** The vocabulary's own rule is to refuse rather
//!   than guess with two projects open, which is right for a caller with no context and wrong for a
//!   pane looking at one. Written first as a *default* and that was a hole — a model naming a
//!   different open project was served against it — so the scope **overwrites**, applied to the
//!   arguments object before it is deserialized, and `list_projects` answers with that project
//!   alone.
//! - **A call yields a card as well as an answer.** The model gets the tool's full JSON; the
//!   transcript gets [`Facts`]. Never a second measurement: `elapsed_ms` is the run's, not a
//!   stopwatch wrapped around the call.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{from_value, to_string, Error as JsonError, Map, Value};

use crate::error::AgentError;
use crate::host::{AgentIdentity, Host};
use crate::tools::StrataTools;

use super::offer;
use crate::wire::{
    DescribeTableParams, ExportResultParams, ListFunctionsParams, ListTablesParams, ProjectParams,
    ProjectsResult, QuerySessionParams, ReadPageParams, RunParams, RunResult, ValidateParams,
};

/// The most rows the assistant asks 'run' for, however many the model requests.
///
/// [`crate::tools::MAX_PAGE_SIZE`] (10,000) stays the wire's cap — right for an MCP client,
/// which decides for itself what to do with an 800 KB answer, and 34x wrong for a
/// conversation that re-sends every tool result on every later round and every later turn.
/// 100 rows by 8 columns measures 7,151 bytes and the turn's 24,000-byte result cap crosses
/// at roughly 330 such rows, so 250 keeps a full page under it with room for wide cells.
/// Applied over the same resolution `run` itself uses
/// ([`StrataTools::resolved_page_size`]), so a host whose row-limit setting is 0 ("no
/// limit") lands here rather than at the wire's cap — the second door, and the one no model
/// even has to ask for. A const rather than a [`Scope`] field: nothing sets it differently
/// today, and a field nobody sets is dead configuration. Promote it when a surface needs to.
const MAX_RUN_ROWS: usize = 250;

/// Which project a call lands in when the model names none.
///
/// The project **root**, because the root is the identity (`host::resolve` tries it first and
/// a name is allowed to collide).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub project: Option<String>,
}

/// What the transcript's step card shows beyond the tool's name — the facts a person reads,
/// none of them derived.
///
/// Serde-able because a step card outlives its window (AS-07): every field is a recorded number
/// or the engine's own wording, so a card read back from disk shows what it always showed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Facts {
    /// The statement, for the tools that take one.
    pub sql: Option<String>,
    pub query_session: Option<String>,
    /// The exact total the run reported.
    pub rows: Option<usize>,
    /// The engine's own timing.
    pub elapsed_ms: Option<u64>,
    /// A run that stopped rather than failed — the user cancelled it, or a newer run in the
    /// session replaced it. Not an error, and the card must not dress it as one.
    pub stopped: Option<String>,
}

/// One executed tool call: what the model is told, and what the card shows.
pub struct Called {
    /// The tool's JSON result, or the taxonomy's message. **Either one goes back to the model
    /// as the tool's result** — an error here is the design working, not a turn failing: the
    /// model reads "CREATE TABLE is not supported" and recovers, exactly as an MCP client
    /// would.
    pub answer: String,
    /// Whether that answer was a fault, so the card can dress it as one.
    pub failed: bool,
    pub facts: Facts,
    /// The statement [`offer_sql`](super::offer) accepted, when this call was one — the
    /// transcript renders it as an **executable** card rather than a step card. `None` for
    /// every other tool, and for an offer that did not check out.
    pub offered: Option<String>,
}

/// Execute one tool call by name.
///
/// Total by construction: an unknown name is answered rather than dropped, because a model
/// that hallucinates a tool needs to be told so in the same channel it asked in.
pub async fn call<H: Host>(
    tools: &StrataTools<H>,
    scope: &Scope,
    name: &str,
    arguments: Value,
) -> Called {
    if offer::is_offer(name) {
        let offered = offer::offer(tools, scope, arguments).await;
        return Called {
            failed: offered.sql.is_none(),
            answer: offered.answer,
            facts: Facts {
                sql: offered.sql.clone(),
                ..Facts::default()
            },
            offered: offered.sql,
        };
    }
    let Some((result, facts)) = dispatch(tools, scope, name, arguments).await else {
        return Called {
            answer: format!("There is no tool called '{name}'. Use only the tools you were given."),
            failed: true,
            facts: Facts::default(),
            offered: None,
        };
    };
    match result {
        Ok(answer) => Called {
            answer,
            failed: false,
            facts,
            offered: None,
        },
        Err(e) => Called {
            answer: e.to_string(),
            failed: true,
            facts,
            offered: None,
        },
    }
}

/// The one match. `None` means no such tool — the only thing [`call`] adds is the sentence
/// for that case.
async fn dispatch<H: Host>(
    tools: &StrataTools<H>,
    scope: &Scope,
    name: &str,
    arguments: Value,
) -> Option<(Result<String, AgentError>, Facts)> {
    let arguments = scoped(arguments, scope);
    let plain = Facts::default();
    Some(match name {
        "list_projects" => (
            encode(name, &only(scope, tools.list_projects().await)),
            plain,
        ),
        "list_tables" => match params::<ListTablesParams>(name, arguments) {
            Err(e) => (Err(e), plain),
            Ok(p) => (answer(name, tools.list_tables(p).await.as_ref()), plain),
        },
        "describe_table" => match params::<DescribeTableParams>(name, arguments) {
            Err(e) => (Err(e), plain),
            Ok(p) => (answer(name, tools.describe_table(p).await.as_ref()), plain),
        },
        "list_functions" => match params::<ListFunctionsParams>(name, arguments) {
            Err(e) => (Err(e), plain),
            Ok(p) => (answer(name, tools.list_functions(p).await.as_ref()), plain),
        },
        "validate" => match params::<ValidateParams>(name, arguments) {
            Err(e) => (Err(e), plain),
            Ok(p) => {
                let sql = Some(p.sql.clone());
                (
                    answer(name, tools.validate(p).await.as_ref()),
                    Facts { sql, ..plain },
                )
            }
        },
        "open_query_session" => match params::<ProjectParams>(name, arguments) {
            Err(e) => (Err(e), plain),
            Ok(p) => {
                let opened = tools
                    .open_query_session(AgentIdentity::assistant(), p)
                    .await;
                let query_session = opened.as_ref().ok().map(|r| r.query_session.clone());
                (
                    answer(name, opened.as_ref()),
                    Facts {
                        query_session,
                        ..plain
                    },
                )
            }
        },
        "list_query_sessions" => match params::<ProjectParams>(name, arguments) {
            Err(e) => (Err(e), plain),
            Ok(p) => (
                answer(name, tools.list_query_sessions(p).await.as_ref()),
                plain,
            ),
        },
        "run" => match params::<RunParams>(name, arguments) {
            Err(e) => (Err(e), plain),
            Ok(mut p) => {
                p.page_size = Some(tools.resolved_page_size(p.page_size).min(MAX_RUN_ROWS));
                let sql = Some(p.sql.clone());
                let session = p.query_session.clone();
                let settled = tools.run(p).await;
                let facts = match settled.as_ref() {
                    Ok(RunResult::Ok {
                        query_session,
                        total,
                        elapsed_ms,
                        ..
                    }) => Facts {
                        sql,
                        query_session: Some(query_session.clone()),
                        rows: Some(*total),
                        elapsed_ms: Some(*elapsed_ms),
                        stopped: None,
                    },
                    Ok(RunResult::Plan { query_session, .. }) => Facts {
                        sql,
                        query_session: Some(query_session.clone()),
                        ..plain
                    },
                    Ok(RunResult::Stopped {
                        query_session,
                        reason,
                    }) => Facts {
                        sql,
                        query_session: Some(query_session.clone()),
                        stopped: Some(reason.clone()),
                        ..plain
                    },
                    Err(_) => Facts {
                        sql,
                        query_session: Some(session),
                        ..plain
                    },
                };
                (answer(name, settled.as_ref()), facts)
            }
        },
        "read_page" => match params::<ReadPageParams>(name, arguments) {
            Err(e) => (Err(e), plain),
            Ok(p) => {
                let session = p.query_session.clone();
                let read = tools.read_page(p).await;
                let rows = read.as_ref().ok().map(|r| r.total);
                (
                    answer(name, read.as_ref()),
                    Facts {
                        query_session: Some(session),
                        rows,
                        ..plain
                    },
                )
            }
        },
        "export_result" => match params::<ExportResultParams>(name, arguments) {
            Err(e) => (Err(e), plain),
            Ok(p) => {
                let session = p.query_session.clone();
                let written = tools.export_result(p).await;
                let rows = written.as_ref().ok().map(|r| r.rows);
                (
                    answer(name, written.as_ref()),
                    Facts {
                        query_session: Some(session),
                        rows,
                        ..plain
                    },
                )
            }
        },
        "close_query_session" => match params::<QuerySessionParams>(name, arguments) {
            Err(e) => (Err(e), plain),
            Ok(p) => {
                let session = p.query_session.clone();
                (
                    answer(name, tools.close_query_session(p).await.as_ref()),
                    Facts {
                        query_session: Some(session),
                        ..plain
                    },
                )
            }
        },
        _ => return None,
    })
}

/// Bind every call to the window's project.
///
/// **A boundary, not a default.** Filling the project in only where the model named none meant a
/// model that named a *different* open project was served against it, and the step card carries no
/// project to say which. The pane belongs to one project, so the scope overwrites, and
/// `list_projects` answers with that one alone ([`only`]).
///
/// Applied to the arguments object rather than to each params struct, so it is one rule for every
/// tool taking a `project` and a no-op for the one that does not. Where there is no scope, nothing
/// is invented and the vocabulary's own ambiguity rule answers.
///
/// **A fixed point**, deliberately, so applying it twice says what applying it once says — which is
/// what lets the turn scope the arguments for the step card *before* the call without the card and
/// the call becoming two normalizations to keep in step.
pub(super) fn scoped(arguments: Value, scope: &Scope) -> Value {
    let Some(project) = scope.project.as_deref() else {
        return arguments;
    };
    let mut object = match arguments {
        Value::Object(object) => object,
        Value::Null => Map::new(),
        other => return other,
    };
    object.insert("project".into(), Value::String(project.to_string()));
    Value::Object(object)
}

/// The projects a scoped caller may see: its own, and nothing else.
///
/// The other half of the boundary. Leaving `list_projects` unfiltered would hand the model the
/// name of every other open window — which it cannot address any more, but which is somebody
/// else's work to know about, and an invitation to try.
fn only(scope: &Scope, mut listed: ProjectsResult) -> ProjectsResult {
    let Some(project) = scope.project.as_deref() else {
        return listed;
    };
    listed.projects.retain(|p| p.root == project);
    listed
}

/// Read a tool's arguments, or say what is wrong with them **to a model**: name the tool, quote
/// serde's own complaint, and point at the schema it was given rather than at a Rust type.
fn params<T: DeserializeOwned>(name: &str, arguments: Value) -> Result<T, AgentError> {
    from_value(arguments).map_err(|e| AgentError::Query(bad_arguments(name, &e)))
}

/// The one wording for arguments that did not fit a tool's schema — shared with
/// [`offer_sql`](super::offer), the assistant's own tool, which must teach the model the same
/// recovery as the router's. Here rather than at each site for `AgentError::no_such_query_session`'s
/// reason: a message written twice is a message that drifts the moment either is tuned.
pub(super) fn bad_arguments(name: &str, why: &JsonError) -> String {
    format!(
        "The arguments for '{name}' did not fit its schema: {why}. Send the arguments the \
         tool's schema names."
    )
}

/// A tool's own `Result`, with the success half encoded and the failure half left exactly as
/// the taxonomy wrote it.
fn answer<T: Serialize>(name: &str, result: Result<&T, &AgentError>) -> Result<String, AgentError> {
    match result {
        Ok(value) => encode(name, value),
        Err(e) => Err(e.clone()),
    }
}

fn encode<T: Serialize>(name: &str, value: &T) -> Result<String, AgentError> {
    to_string(value)
        .map_err(|e| AgentError::Query(format!("The '{name}' result could not be encoded: {e}.")))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::{env, fs, process};

    use serde_json::json;
    use strata_engine::{DenyCode, Form, Reason, StmtKind};
    use strata_engine::{TableSpec, CANCELLED};
    use strata_model::SourceFormat;

    use crate::host::{CatalogEntry, RegState};
    use crate::mock::{MockHost, MockProject};

    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata_assistant_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn one_project(tag: &str) -> (PathBuf, StrataTools<MockHost>) {
        let root = scratch(tag);
        fs::write(root.join("people.csv"), "id,name\n1,ana\n2,ben\n").unwrap();
        let project = MockProject::new("sales", &root);
        project
            .engine
            .register(TableSpec {
                name: "people".into(),
                paths: vec![root.join("people.csv").display().to_string()],
                format: SourceFormat::from_name("csv"),
                partitions: Vec::new(),
                internal: false,
            })
            .await
            .unwrap();
        let project = project.with_catalog(vec![CatalogEntry::Table {
            name: "people".into(),
            format: "csv".into(),
            sources: vec!["people.csv".into()],
            reg: RegState::Ready,
        }]);
        (root, StrataTools::new(MockHost::new(vec![project])))
    }

    /// **The binding's own guarantee.** Every tool the router advertises has an arm here, so a
    /// tool added to the vocabulary cannot be offered to a model with nothing behind it.
    #[tokio::test]
    async fn every_manifest_tool_has_an_arm() {
        let (_root, tools) = one_project("manifest").await;
        let scope = Scope::default();
        for spec in tools.manifest() {
            assert!(
                dispatch(&tools, &scope, &spec.name, json!({}))
                    .await
                    .is_some(),
                "'{}' is advertised to the model with no arm behind it",
                spec.name
            );
        }
    }

    #[tokio::test]
    async fn an_invented_tool_is_answered_rather_than_dropped() {
        let (_root, tools) = one_project("invented").await;
        let called = call(&tools, &Scope::default(), "drop_table", json!({})).await;
        assert!(called.failed);
        assert!(called.answer.contains("no tool called 'drop_table'"));
    }

    /// Bad arguments are a message the model can act on, not a Rust type name.
    #[tokio::test]
    async fn bad_arguments_name_the_tool_and_its_schema() {
        let (_root, tools) = one_project("badargs").await;
        let called = call(&tools, &Scope::default(), "describe_table", json!({})).await;
        assert!(called.failed);
        assert!(
            called.answer.contains("arguments for 'describe_table'"),
            "{}",
            called.answer
        );
        assert!(called.answer.contains("schema"), "{}", called.answer);
    }

    /// **The pane's project is a boundary, not a default.** A call that names none lands in it,
    /// and a call that names *another open project* is overruled rather than served — the
    /// vocabulary resolves any project the host lists, so defaulting-only let a pane scoped to
    /// one window run SQL against another window's data.
    #[tokio::test]
    async fn every_call_is_bound_to_the_windows_project() {
        let scope = Scope {
            project: Some("/w/sales".into()),
        };
        assert_eq!(
            scoped(json!({"sql": "select 1"}), &scope),
            json!({"sql": "select 1", "project": "/w/sales"})
        );
        assert_eq!(
            scoped(json!({"project": "/w/ops"}), &scope),
            json!({"project": "/w/sales"})
        );
        assert_eq!(scoped(json!({}), &Scope::default()), json!({}));
    }

    /// The other half of that boundary: a scoped caller is told about its own project and no
    /// other. Knowing another window is open is not this agent's business.
    #[tokio::test]
    async fn a_scoped_caller_sees_only_its_own_project() {
        let tools = StrataTools::new(MockHost::new(vec![
            MockProject::new("sales", "/w/sales"),
            MockProject::new("ops", "/w/ops"),
        ]));
        let scope = Scope {
            project: Some("/w/sales".into()),
        };
        let listed = call(&tools, &scope, "list_projects", json!({})).await;
        assert!(listed.answer.contains("/w/sales"), "{}", listed.answer);
        assert!(!listed.answer.contains("/w/ops"), "{}", listed.answer);

        let unscoped = call(&tools, &Scope::default(), "list_tables", json!({})).await;
        assert!(unscoped.failed, "{}", unscoped.answer);
        let bound = call(&tools, &scope, "list_tables", json!({"project": "/w/ops"})).await;
        assert!(!bound.failed, "{}", bound.answer);
    }

    /// A card carries the run's own facts: the SQL attempted, the session it ran in, the exact
    /// total and the engine's elapsed. Nothing measured a second time.
    #[tokio::test]
    async fn a_run_yields_the_facts_its_card_shows() {
        let (root, tools) = one_project("facts").await;
        let scope = Scope {
            project: Some(root.display().to_string()),
        };
        let opened = call(&tools, &scope, "open_query_session", json!({})).await;
        let session = opened.facts.query_session.clone().unwrap();

        let ran = call(
            &tools,
            &scope,
            "run",
            json!({"query_session": session, "sql": "select * from people"}),
        )
        .await;
        assert!(!ran.failed, "{}", ran.answer);
        assert_eq!(ran.facts.sql.as_deref(), Some("select * from people"));
        assert_eq!(ran.facts.query_session.as_deref(), Some(session.as_str()));
        assert_eq!(ran.facts.rows, Some(2));
        assert!(ran.facts.elapsed_ms.is_some());
        assert!(ran.facts.stopped.is_none());
    }

    /// The assistant asks for less than the wire's cap: a model asking for thousands of
    /// rows runs at [`MAX_RUN_ROWS`], and the answer echoes the size actually used — the
    /// clamp is visible, never a silent truncation.
    #[tokio::test]
    async fn a_runs_page_is_capped_at_the_assistants_ceiling() {
        let (root, tools) = one_project("ceiling").await;
        let scope = Scope {
            project: Some(root.display().to_string()),
        };
        let opened = call(&tools, &scope, "open_query_session", json!({})).await;
        let session = opened.facts.query_session.clone().unwrap();

        let ran = call(
            &tools,
            &scope,
            "run",
            json!({"query_session": session, "sql": "select * from people", "page_size": 5000}),
        )
        .await;
        assert!(!ran.failed, "{}", ran.answer);
        let answer: Value = serde_json::from_str(&ran.answer).unwrap();
        assert_eq!(answer["page_size"].as_u64(), Some(MAX_RUN_ROWS as u64));
    }

    /// A policy refusal reaches the **model**, in the editor's own words, and the card still
    /// shows the statement that drew it.
    #[tokio::test]
    async fn a_refusal_is_the_tools_answer_and_keeps_its_sql() {
        let (root, tools) = one_project("refusal").await;
        let scope = Scope {
            project: Some(root.display().to_string()),
        };
        let opened = call(&tools, &scope, "open_query_session", json!({})).await;
        let session = opened.facts.query_session.clone().unwrap();

        let sql = "create table t as select 1";
        let ran = call(
            &tools,
            &scope,
            "run",
            json!({"query_session": session, "sql": sql}),
        )
        .await;
        assert!(ran.failed);
        assert_eq!(ran.facts.sql.as_deref(), Some(sql));
        assert_eq!(
            ran.answer,
            Reason::Policy {
                form: Form::Statement(StmtKind::CreateTable),
                code: DenyCode::NotGranted,
            }
            .message()
        );
    }

    /// A stop is a status, and the card must not dress it as a fault.
    #[tokio::test]
    async fn a_stopped_run_is_not_a_failure() {
        let root = scratch("stopped");
        let project = MockProject::new("sales", &root).settling(CANCELLED);
        let tools = StrataTools::new(MockHost::new(vec![project]));
        let scope = Scope {
            project: Some(root.display().to_string()),
        };
        let opened = call(&tools, &scope, "open_query_session", json!({})).await;
        let session = opened.facts.query_session.clone().unwrap();

        let ran = call(
            &tools,
            &scope,
            "run",
            json!({"query_session": session, "sql": "select 1"}),
        )
        .await;
        assert!(!ran.failed);
        assert_eq!(ran.facts.stopped.as_deref(), Some(CANCELLED));
    }
}
