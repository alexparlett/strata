//! **The vocabulary with no transport under it** (AS-01) — the ten tools driven as plain
//! methods, from outside the crate, with no rmcp type anywhere in this file.
//!
//! `mcp_over_http.rs` and `mcp_over_stdio.rs` prove the two MCP deployments; this proves the
//! third, which is the one the assistant pane uses: an in-process caller holds a
//! [`StrataTools`], opens a query session under an identity of its own, runs, pages and
//! closes — and is refused by the same policy gate, in the same words, because it reaches the
//! same body. The `#[tool]` wrappers add exactly two things this path does not need (which
//! agent the *request* is, and holding that agent against the idle sweep), so an absent
//! import of `rmcp` here is the claim under test rather than tidiness.

use std::path::PathBuf;
use std::{env, fs, process};

use strata_agent::mock::{MockHost, MockProject};
use strata_agent::wire::{
    DescribeTableParams, EntryWire, ListFunctionsParams, ListTablesParams, ProjectParams,
    QuerySessionParams, ReadPageParams, RunParams, RunResult, StateWire, ValidateParams,
};
use strata_agent::{AgentError, AgentIdentity, CatalogEntry, Described, RegState, StrataTools};
use strata_core::engine::sql::Blocked;
use strata_core::engine::TableSpec;
use strata_model::SourceFormat;

/// A project whose engine really holds a `people` table of five rows, plus the catalog rows
/// an app would have folded from the same registration. `tag` is per test because these run
/// concurrently in one process and DataFusion re-LISTs a table's sources at scan time.
async fn project(tag: &str) -> (PathBuf, StrataTools<MockHost>) {
    let root = env::temp_dir().join(format!("strata_agent_facade_{}_{tag}", process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("people.csv"),
        "id,name\n1,ana\n2,ben\n3,cara\n4,dev\n5,eli\n",
    )
    .unwrap();

    let project = MockProject::new("sales", &root);
    let meta = project
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
    let project = project
        .with_catalog(vec![CatalogEntry::Table {
            name: "people".into(),
            format: "csv".into(),
            sources: vec!["people.csv".into()],
            reg: RegState::Ready,
        }])
        .with_described(Described::Table {
            name: "people".into(),
            format: "csv".into(),
            sources: vec!["people.csv".into()],
            partitions: Vec::new(),
            rows: meta.rows,
            columns: meta.columns,
        });
    (root, StrataTools::new(MockHost::new(vec![project])))
}

fn here() -> ProjectParams {
    ProjectParams { project: None }
}

#[tokio::test]
async fn the_whole_vocabulary_answers_with_no_mcp_peer() {
    let (root, tools) = project("vocabulary").await;

    // --- the catalog plane -------------------------------------------------

    let projects = tools.list_projects().await.projects;
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "sales");

    let entries = tools
        .list_tables(ListTablesParams::default())
        .await
        .unwrap()
        .entries;
    match &entries[..] {
        [EntryWire::Table {
            name,
            state: StateWire::Ready,
            ..
        }] => assert_eq!(name, "people"),
        other => panic!("{other:?}"),
    }

    let described = tools
        .describe_table(DescribeTableParams {
            name: "people".into(),
            ..DescribeTableParams::default()
        })
        .await
        .unwrap();
    assert_eq!(
        described
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "name"]
    );

    let functions = tools
        .list_functions(ListFunctionsParams::default())
        .await
        .unwrap();
    assert!(functions.aggregate.iter().any(|f| f.name == "count"));

    let checked = tools
        .validate(ValidateParams {
            sql: "SELECT id FROM nope".into(),
            project: None,
        })
        .await
        .unwrap();
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|d| d.message.contains("nope")),
        "{checked:?}"
    );

    // --- open → run → read_page → close ------------------------------------

    // The identity is the caller's own, because there is no peer to read one off. The
    // assistant's is `AgentIdentity::assistant()`; a test says what it is.
    let session = tools
        .open_query_session(
            AgentIdentity {
                name: "facade-test".into(),
                version: "1".into(),
            },
            here(),
        )
        .await
        .unwrap()
        .query_session;

    let listed = tools.list_query_sessions(here()).await.unwrap();
    assert_eq!(listed.query_sessions.len(), 1);
    assert_eq!(listed.query_sessions[0].query_session, session);

    let ran = tools
        .run(RunParams {
            query_session: session.clone(),
            sql: "SELECT id FROM people ORDER BY id".into(),
            mode: None,
            page_size: Some(2),
            project: None,
        })
        .await
        .unwrap();
    let RunResult::Ok { rows, total, .. } = ran else {
        panic!("{ran:?}");
    };
    // Bounded by the page, exact in the total — no `LIMIT` was injected here either.
    assert_eq!(rows, vec![vec![Some("1".into())], vec![Some("2".into())]]);
    assert_eq!(total, 5);

    let page = tools
        .read_page(ReadPageParams {
            query_session: session.clone(),
            page: 3,
            sort: None,
            project: None,
        })
        .await
        .unwrap();
    // The page size follows the run that settled it, so paging is consistent.
    assert_eq!(page.page_size, 2);
    assert_eq!(page.rows, vec![vec![Some("5".into())]]);

    tools
        .close_query_session(QuerySessionParams {
            query_session: session,
            project: None,
        })
        .await
        .unwrap();
    assert!(tools
        .list_query_sessions(here())
        .await
        .unwrap()
        .query_sessions
        .is_empty());

    let _ = fs::remove_dir_all(&root);
}

/// **The gate is inside the body, so it is in front of this path too** — and it says the same
/// thing. Not "an equivalent message": the assistant's model reads the editor's own words,
/// which is what makes a refusal something it can act on rather than a wall.
#[tokio::test]
async fn a_blocked_statement_is_refused_in_the_editors_own_words() {
    let (root, tools) = project("policy").await;
    let session = tools
        .open_query_session(AgentIdentity::assistant(), here())
        .await
        .unwrap()
        .query_session;

    let Err(refused) = tools
        .run(RunParams {
            query_session: session,
            sql: "CREATE TABLE copy AS SELECT * FROM people".into(),
            mode: None,
            page_size: None,
            project: None,
        })
        .await
    else {
        panic!("expected a policy refusal");
    };
    assert!(matches!(refused, AgentError::Policy(_)), "{refused:?}");
    assert_eq!(refused.to_string(), Blocked::CreateTable.editor_message());

    let _ = fs::remove_dir_all(&root);
}

/// The manifest is what an in-process loop hands its model, and it has to be the same offer
/// an MCP client gets: the ten names `mcp_over_http.rs` pins off `tools/list`, each with the
/// description and argument schema that listing carries.
///
/// **The order is asserted rather than sorted away, and the difference from
/// `mcp_over_http.rs` is deliberate.** That test sorts because it reads names back off the
/// wire, where the order is the transport's; this one reads `manifest()`, which sorts for
/// itself and says so — a model-facing tool list that reordered per process would invalidate
/// the provider's prompt cache every turn, so the ordering is a promise worth a test rather
/// than a detail worth tolerating.
#[test]
fn the_manifest_offers_exactly_what_the_wire_advertises() {
    let manifest = StrataTools::new(MockHost::new(Vec::new())).manifest();

    let names: Vec<&str> = manifest.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "close_query_session",
            "describe_table",
            "list_functions",
            "list_projects",
            "list_query_sessions",
            "list_tables",
            "open_query_session",
            "read_page",
            "run",
            "validate",
        ]
    );

    for tool in &manifest {
        assert!(
            !tool.description.is_empty(),
            "{} must carry its doc comment as a description",
            tool.name
        );
        assert_eq!(
            tool.input_schema.get("type").and_then(|t| t.as_str()),
            Some("object"),
            "{}'s arguments are an object: {}",
            tool.name,
            tool.input_schema
        );
    }

    // The arguments a tool takes are the ones it advertises. A wrapper that lost its
    // `Parameters<T>` would still list, still describe itself, and quietly tell every model
    // it takes none — so the properties are named rather than counted.
    let properties = |name: &str| -> Vec<String> {
        let tool = manifest.iter().find(|t| t.name == name).expect(name);
        let mut keys: Vec<String> = tool.input_schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name} advertises a properties object"))
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    };
    assert_eq!(properties("list_projects"), Vec::<String>::new());
    assert_eq!(
        properties("list_tables"),
        vec!["matching", "page", "project"]
    );
    assert_eq!(
        properties("describe_table"),
        vec!["matching", "name", "page", "path", "project"]
    );
    assert_eq!(properties("list_functions"), vec!["matching", "project"]);
    assert_eq!(properties("validate"), vec!["project", "sql"]);
    assert_eq!(properties("open_query_session"), vec!["project"]);
    assert_eq!(properties("list_query_sessions"), vec!["project"]);
    assert_eq!(
        properties("run"),
        vec!["mode", "page_size", "project", "query_session", "sql"]
    );
    assert_eq!(
        properties("read_page"),
        vec!["page", "project", "query_session", "sort"]
    );
    assert_eq!(
        properties("close_query_session"),
        vec!["project", "query_session"]
    );
}
