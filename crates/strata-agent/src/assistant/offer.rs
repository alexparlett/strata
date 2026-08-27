//! **The runnable statement** — the assistant's one way to hand the user SQL to execute, as a
//! tool rather than as a formatting convention.
//!
//! **Why a tool.** Most SQL an assistant writes is *explanation*; only some of it is an offer, and
//! the transcript has to tell those apart or it puts a Run press on fragments. A tagged markdown
//! fence was built and withdrawn: prompt-taught formatting is followed unevenly, least reliably by
//! exactly the small local models the Ollama entry exists for. A tool is taught by its **schema**,
//! and it buys what a fence structurally cannot — the statement is checked *before* the card
//! appears, against the **editor's** policy rather than the agent's, because the card runs in the
//! user's editor under their capability.
//!
//! That check is `validate`'s, so a card carries what the editor would not underline — deliberately
//! **not** a promise that the statement parses. `Lang::validate` keeps three silences on purpose
//! (an incomplete trailing statement, an unresolved column where the resolver's scope is
//! incomplete, a `;`-separated batch judged one statement at a time), each right for a live buffer
//! and each letting something through here; the Run press answers for the rest in the editor's own
//! words. Closing the gap needs a parse the vocabulary does not expose, recorded in the AS-02 task
//! file rather than papered over.
//!
//! **Why it is not on the router.** An MCP client has no transcript to draw a card in, so
//! `tools/list` is unchanged and [`StrataTools::manifest`](crate::StrataTools::manifest) stays
//! derived from the router; the loop offers *the manifest plus this*. Nothing here touches
//! [`Host`], the engine or a query session — its whole effect is that the user sees a card.

use rmcp::handler::server::common::schema_for_input;
use rmcp::model::JsonObject;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{from_value, Value};

use crate::host::Host;
use crate::tools::{StrataTools, ToolSpec};
use crate::wire::{SeverityWire, ValidateParams};

use super::dispatch::Scope;

/// The tool's name on the wire the model answers on.
pub const OFFER_SQL: &str = "offer_sql";

/// Is this the assistant's own presentation tool rather than one of the ten?
///
/// The turn asks, because an offer is not a step: it renders as an executable card and must
/// not also produce a tool card describing itself.
pub fn is_offer(name: &str) -> bool {
    name == OFFER_SQL
}

/// What `offer_sql` takes. The doc comments are the schema's descriptions, which is the whole
/// of how the model learns this — see the module note.
///
/// **One argument, and no `project`.** Every other tool takes one because an MCP client can be
/// looking at any of several open windows; this tool exists only in a chat pane, and a chat
/// pane belongs to exactly one project. The [`Scope`] supplies it, so the model is never asked
/// a question that has only one answer.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct OfferParams {
    /// One complete, executable statement. Real table, column and function names from the
    /// catalog only, and no placeholders. Never a fragment, a clause or pseudo-SQL.
    pub sql: String,
}

/// The tool as the model is offered it. Plain data, the same shape a manifest entry has, so the
/// loop's tool list is one list.
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: OFFER_SQL.into(),
        description: "Offer the user one complete SQL statement to run. It appears in the \
                      conversation as a card they can execute or open in their editor. Use it \
                      when you are handing them a statement to run; SQL you are only \
                      explaining belongs in an ordinary sql code block in your reply. The \
                      statement is checked before the card appears, and you are told what is \
                      wrong with it if it does not check out."
            .into(),
        input_schema: schema_for_input::<OfferParams>()
            .map(|schema| Value::Object(JsonObject::clone(&schema)))
            .expect("offer_sql's params must have an object schema"),
    }
}

/// What offering a statement did.
pub struct Offered {
    /// The statement, when it checked out. `None` means no card: the model was told what is
    /// wrong and will offer a corrected one.
    pub sql: Option<String>,
    /// What the model is told either way.
    pub answer: String,
}

/// Check a statement and offer it.
///
/// The check is the vocabulary's own `validate` — lints, the managed-DDL policy and a dry plan
/// against the real catalog — so a card refuses to carry exactly what the editor would
/// squiggle, in the same words.
///
/// **And it is the *editor's* policy, not the agent's, which is the point of this tool.** A
/// card is executed by the user, in their own editor, under their own capability. So the
/// assistant may offer a `CREATE TABLE` it is itself refused — which is precisely the handover
/// the system prompt asks for ("give them the statement to run in their own editor"). Judging
/// an offer by the read-only gate would make the one case the user most needs impossible.
///
/// **Errors only**: a warning is something the user can read on the card and decide about, and
/// refusing on one would make the assistant unable to offer a statement it is right about.
pub async fn offer<H: Host>(tools: &StrataTools<H>, scope: &Scope, arguments: Value) -> Offered {
    let params: OfferParams = match from_value(arguments) {
        Ok(params) => params,
        Err(e) => {
            return Offered {
                sql: None,
                answer: super::dispatch::bad_arguments(OFFER_SQL, &e),
            }
        }
    };
    let sql = params.sql.trim().to_string();
    if sql.is_empty() {
        return Offered {
            sql: None,
            answer: "There was no statement to offer.".into(),
        };
    }

    let checked = tools
        .validate(ValidateParams {
            sql: sql.clone(),
            project: scope.project.clone(),
        })
        .await;
    let faults: Vec<String> = match checked {
        Err(e) => {
            return Offered {
                sql: None,
                answer: e.to_string(),
            }
        }
        Ok(result) => result
            .diagnostics
            .into_iter()
            .filter(|d| matches!(d.severity, SeverityWire::Error))
            .map(|d| match d.loc {
                Some(loc) => format!("{loc}: {}", d.message),
                None => d.message,
            })
            .collect(),
    };

    match faults.is_empty() {
        true => Offered {
            sql: Some(sql),
            answer: "Offered. The user can run it from the conversation.".into(),
        },
        false => Offered {
            sql: None,
            answer: format!(
                "That statement did not check out, so it was not offered:\n{}\nFix it and \
                 offer it again.",
                faults.join("\n")
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::{env, fs, process};

    use serde_json::json;
    use strata_engine::TableSpec;
    use strata_model::SourceFormat;

    use crate::mock::{MockHost, MockProject};

    use super::*;

    async fn one_project(tag: &str) -> StrataTools<MockHost> {
        let root: PathBuf = env::temp_dir().join(format!("strata_offer_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("people.csv"), "id,name\n1,ana\n").unwrap();
        let project = MockProject::new("sales", &root);
        project
            .engine
            .catalog()
            .register(TableSpec {
                name: "people".into(),
                paths: vec![root.join("people.csv").display().to_string()],
                format: SourceFormat::from_name("csv"),
                partitions: Vec::new(),
                connection: None,
                internal: false,
            })
            .await
            .unwrap();
        StrataTools::new(MockHost::new(vec![project]))
    }

    /// The schema is what teaches the model, so it has to name the argument and describe it.
    #[test]
    fn the_schema_names_the_argument_it_takes() {
        let spec = spec();
        assert_eq!(spec.name, OFFER_SQL);
        let schema = spec.input_schema.to_string();
        assert!(schema.contains("\"sql\""), "{schema}");
        assert!(schema.contains("executable statement"), "{schema}");
        assert!(spec.description.contains("card"));
    }

    #[tokio::test]
    async fn a_statement_that_checks_out_is_offered() {
        let tools = one_project("good").await;
        let offered = offer(
            &tools,
            &Scope::default(),
            json!({"sql": "select id from people"}),
        )
        .await;
        assert_eq!(offered.sql.as_deref(), Some("select id from people"));
        assert!(offered.answer.starts_with("Offered."));
    }

    /// **What the fence could not do.** A card never carries SQL the editor would squiggle,
    /// because the check happens before the card exists.
    #[tokio::test]
    async fn a_statement_that_does_not_check_out_is_not_offered() {
        let tools = one_project("bad").await;
        let offered = offer(
            &tools,
            &Scope::default(),
            json!({"sql": "select nope from people"}),
        )
        .await;
        assert!(offered.sql.is_none());
        assert!(
            offered.answer.contains("did not check out"),
            "{}",
            offered.answer
        );
        assert!(offered.answer.contains("offer it again"));
    }

    /// **The handover this tool exists for.** The assistant is refused a write statement; the
    /// user is not. A card is run in *their* editor under their capability, so a `CREATE TABLE`
    /// the assistant may not execute is one it may still hand over — which is what the system
    /// prompt tells it to do when asked for a write.
    #[tokio::test]
    async fn a_write_the_assistant_may_not_run_is_still_offerable() {
        let tools = one_project("write").await;
        let sql = "create table snapshot as select * from people";
        let offered = offer(&tools, &Scope::default(), json!({ "sql": sql })).await;
        assert_eq!(offered.sql.as_deref(), Some(sql), "{}", offered.answer);

        let session = tools
            .open_query_session(
                crate::host::AgentIdentity::assistant(),
                crate::wire::ProjectParams { project: None },
            )
            .await
            .unwrap()
            .query_session;
        let ran = tools
            .run(crate::wire::RunParams {
                query_session: session,
                sql: sql.into(),
                mode: None,
                page_size: None,
                project: None,
            })
            .await;
        assert!(matches!(ran, Err(crate::AgentError::Policy(_))), "{ran:?}");
    }

    #[tokio::test]
    async fn an_empty_statement_is_nothing_to_offer() {
        let tools = one_project("empty").await;
        let offered = offer(&tools, &Scope::default(), json!({"sql": "   "})).await;
        assert!(offered.sql.is_none());
        assert_eq!(offered.answer, "There was no statement to offer.");
    }
}
