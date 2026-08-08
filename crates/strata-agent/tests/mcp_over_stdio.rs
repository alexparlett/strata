//! The headless host (AA-05) end to end: a real project folder on disk, a real engine with
//! the registration pass replayed over it, and rmcp's own MCP client on the other end of the
//! **same framing stdio uses**.
//!
//! The unit tests in `src/headless.rs` drive the `Host` methods directly, which proves what
//! the host answers and nothing about the deployment. This proves the deployment: that one
//! `StrataTools` serves the whole vocabulary over an async read/write transport with no HTTP,
//! no port and no token — and that the policy gate, which lives above the transport, is
//! therefore in front of this one too.
//!
//! The transport is a `tokio::io::duplex` pair rather than the process's real stdin/stdout,
//! and that is the only difference from `strata mcp <project>`: rmcp's stdio transport *is*
//! `(Stdin, Stdout)` through the same `AsyncRead + AsyncWrite` adapter this drives
//! (`transport::async_rw`). Owning the test process's actual stdio would make the harness's
//! own output the protocol.

use std::path::PathBuf;
use std::sync::Arc;
use std::{env, fs, process};

use rmcp::model::CallToolRequestParams;
use rmcp::ServiceExt;
use serde_json::json;
use strata_agent::{HeadlessHost, StrataTools};
use strata_core::project::{save_defs, ProjectDefs};
use strata_model::{SourceFormat, TableDef, TableOrigin, ViewDef};

/// A project folder holding one good table, one whose source is missing, and a view over the
/// good one. `tag` is per test for the reason `strata-core`'s own `scratch` helper takes one:
/// these run concurrently in one process and DataFusion re-LISTs a table's sources at scan
/// time, so a shared folder would let one test delete another's data mid-query.
fn project(tag: &str) -> PathBuf {
    let root = env::temp_dir().join(format!("strata_agent_stdio_{}_{tag}", process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("people.csv"), "id,name\n1,ana\n2,ben\n3,cara\n").unwrap();
    save_defs(
        &root,
        &ProjectDefs {
            name: "sales".into(),
            tables: vec![
                TableDef {
                    name: "people".into(),
                    format: SourceFormat::from_name("csv"),
                    connection: None,
                    sources: vec!["people.csv".into()],
                    partition_cols: Vec::new(),
                    origin: TableOrigin::External,
                },
                TableDef {
                    name: "gone".into(),
                    format: SourceFormat::from_name("parquet"),
                    connection: None,
                    sources: vec!["missing.parquet".into()],
                    partition_cols: Vec::new(),
                    origin: TableOrigin::External,
                },
            ],
            views: vec![ViewDef {
                name: "adults".into(),
                sql: "SELECT id FROM people".into(),
            }],
            saved_queries: Vec::new(),
            ..Default::default()
        },
    )
    .unwrap();
    root
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_drives_a_project_with_no_app_running() {
    let root = project("client");
    let host = HeadlessHost::open(root.clone())
        .await
        .expect("the project opens");

    // The transport: one duplex pair, the server on one end and the client on the other.
    // **The server side is spawned**, because `serve` completes the MCP handshake before it
    // returns and a paired in-memory transport has nobody else to complete it: awaiting the
    // server here would wait for an `initialize` the client below has not been built to send.
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let served = tokio::spawn(StrataTools::new(Arc::new(host)).serve(server_io));
    let client = ().serve(client_io).await.expect("the client initializes against it");
    let _server = served
        .await
        .expect("the server task")
        .expect("the vocabulary serves over an async read/write transport");

    // One project by construction, and it is the folder the process was opened on.
    let projects = client
        .call_tool(CallToolRequestParams::new("list_projects"))
        .await
        .unwrap()
        .structured_content
        .expect("structured content");
    assert_eq!(projects["projects"].as_array().map(Vec::len), Some(1));
    assert_eq!(projects["projects"][0]["name"], "sales");

    // The catalog is the registration pass's own answers: the def the engine refused is a row
    // carrying its error, not a missing row.
    let tables = client
        .call_tool(CallToolRequestParams::new("list_tables"))
        .await
        .unwrap()
        .structured_content
        .expect("structured content");
    let entries = tables["entries"].as_array().expect("entries");
    let states: Vec<(&str, &str)> = entries
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap_or_default(),
                e["state"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        states,
        vec![("gone", "failed"), ("people", "ready"), ("adults", "ready")]
    );
    assert!(
        entries[0]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("missing.parquet"),
        "{entries:?}"
    );

    // A schema is what registration read.
    let described = client
        .call_tool(
            CallToolRequestParams::new("describe_table")
                .with_arguments(json!({ "name": "people" }).as_object().unwrap().clone()),
        )
        .await
        .unwrap()
        .structured_content
        .expect("structured content");
    assert_eq!(described["columns"][0]["name"], "id");
    assert_eq!(described["columns"][1]["name"], "name");

    // The full agent loop: open a query session, run in it, page the settled snapshot.
    let session = client
        .call_tool(CallToolRequestParams::new("open_query_session"))
        .await
        .unwrap()
        .structured_content
        .expect("structured content")["query_session"]
        .as_str()
        .expect("a query-session handle")
        .to_string();

    let run = client
        .call_tool(
            CallToolRequestParams::new("run").with_arguments(
                json!({
                    "query_session": session,
                    "sql": "SELECT id FROM adults ORDER BY id",
                    "page_size": 2,
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .unwrap();
    assert_ne!(run.is_error, Some(true), "{run:?}");
    let ran = run.structured_content.expect("structured content");
    assert_eq!(ran["status"], "ok");
    // Bounded by the page, exact in the total — the view really planned over the table.
    assert_eq!(ran["rows"], json!([["1"], ["2"]]));
    assert_eq!(ran["total"], 3);

    let page = client
        .call_tool(
            CallToolRequestParams::new("read_page").with_arguments(
                json!({ "query_session": session, "page": 2 })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap()
        .structured_content
        .expect("structured content");
    assert_eq!(page["rows"], json!([["3"]]));

    // **The policy gate is above the transport, so it is in front of this one too** — and the
    // refusal is a tool result the model can read and recover from, not a protocol fault.
    let refused = client
        .call_tool(
            CallToolRequestParams::new("run").with_arguments(
                json!({ "query_session": session, "sql": "CREATE TABLE t AS SELECT 1" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(refused.is_error, Some(true));
    let text = format!("{:?}", refused.content);
    assert!(text.contains("Table Config"), "{text}");

    // **Nothing the user owns was written.** The project keeps the one file it arrived with:
    // no `session.json` (an agent holds no tabs), no `history.jsonl` (history is the user's),
    // and no app config anywhere near this process.
    let mut written: Vec<String> = fs::read_dir(root.join(".strata"))
        .expect("the project's own directory")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(written, vec![".gitignore", "project.json"]);

    client.cancel().await.ok();
    let _ = fs::remove_dir_all(&root);
}
