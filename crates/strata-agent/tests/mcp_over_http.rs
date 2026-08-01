//! One end-to-end pass through the real thing: an [`AgentServer`] over a mock host, dialled
//! by rmcp's own MCP client over Streamable HTTP.
//!
//! The unit tests in `src/tools.rs` call the vocabulary directly, which proves the semantics
//! and nothing about the wire. This proves the wire: that the tools are advertised with
//! schemas a client can read, that a call round-trips through the transport, that structured
//! content arrives as structured content, and that the bearer check answers **401 before any
//! tool runs**.

use std::time::Duration;
use std::{env, fs, process};

use reqwest::Client;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde_json::json;
use strata_agent::host::{CatalogEntry, RegState};
use strata_agent::mock::{MockHost, MockProject};
use strata_agent::{AgentServer, MCP_PATH};
use strata_core::engine::TableSpec;
use strata_model::SourceFormat;
use tokio::net::TcpStream;
use tokio::time::sleep;

const TOKEN: &str = "test-token-a1b2c3";

/// A served mock project holding a real two-row `people` table.
///
/// `tag` is per test, and it is load-bearing: these run concurrently in one process, each
/// begins by removing its scratch directory, and DataFusion re-LISTs a table's sources at
/// **scan** time rather than caching the file set at registration — so a shared directory
/// would let one test delete `people.csv` out from under another test's in-flight query.
/// (The same reason `strata-core`'s own `scratch(tag)` helper takes one.)
async fn serve(tag: &str) -> (AgentServer, String) {
    let root = env::temp_dir().join(format!("strata_agent_http_{}_{tag}", process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("people.csv"), "id,name\n1,ana\n2,ben\n").unwrap();

    let project = MockProject::new("sales", &root);
    project
        .engine
        .register(TableSpec {
            name: "people".into(),
            paths: vec![root.join("people.csv").display().to_string()],
            format: SourceFormat::from_name("csv"),
            partitions: Vec::new(),
        })
        .await
        .unwrap();
    let project = project.with_catalog(vec![CatalogEntry::Table {
        name: "people".into(),
        format: "csv".into(),
        sources: vec!["people.csv".into()],
        reg: RegState::Ready,
    }]);

    // Port 0: the OS picks, so concurrent test binaries never collide on a fixed one.
    let server = AgentServer::start(0, TOKEN.into(), MockHost::new(vec![project]))
        .expect("the agent server binds");
    let url = format!("http://{}{MCP_PATH}", server.addr());
    (server, url)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_lists_the_tools_and_calls_them() {
    let (_server, url) = serve("client").await;
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url).auth_header(TOKEN),
    );
    let client = ().serve(transport).await.expect("the client initializes against the server");

    // The whole vocabulary is advertised, and the instructions the handler carries reach the
    // client's view of the server.
    let mut names: Vec<String> = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    names.sort();
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
    let info = client.peer_info().expect("server info");
    assert!(
        info.instructions
            .as_deref()
            .unwrap_or_default()
            .contains("Read-only"),
        "{:?}",
        info.instructions
    );

    // A project-scoped read, over the wire, as structured content.
    let tables = client
        .call_tool(CallToolRequestParams::new("list_tables"))
        .await
        .unwrap();
    assert_ne!(tables.is_error, Some(true), "{tables:?}");
    let structured = tables.structured_content.expect("structured content");
    assert_eq!(structured["entries"][0]["name"], "people");

    // Open a query session, run in it, and read the second page back — the full agent loop.
    let opened = client
        .call_tool(CallToolRequestParams::new("open_query_session"))
        .await
        .unwrap()
        .structured_content
        .expect("structured content");
    let session = opened["query_session"]
        .as_str()
        .expect("a query-session handle")
        .to_string();

    let run = client
        .call_tool(
            CallToolRequestParams::new("run").with_arguments(
                json!({ "query_session": session, "sql": "SELECT id FROM people ORDER BY id", "page_size": 1 })
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
    assert_eq!(ran["total"], 2);
    assert_eq!(ran["rows"], json!([["1"]]));

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
    assert_eq!(page["rows"], json!([["2"]]));

    // A refusal is a tool result the model can read and recover from, not a protocol fault.
    let refused = client
        .call_tool(
            CallToolRequestParams::new("run").with_arguments(
                json!({ "query_session": session, "sql": "DROP TABLE people" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(refused.is_error, Some(true));
    let text = format!("{:?}", refused.content);
    assert!(
        text.contains("DROP is not supported in the editor"),
        "{text}"
    );

    client.cancel().await.ok();
}

/// The token is checked in front of the router, so a request without it never reaches a
/// tool — it is an HTTP 401, before any MCP framing is even read.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_without_the_token_is_401_before_any_tool_runs() {
    let (_server, url) = serve("unauthorized").await;
    let http = Client::new();
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
    });

    let anonymous = http
        .post(&url)
        .header("accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
    assert_eq!(
        anonymous.headers().get("www-authenticate").unwrap(),
        "Bearer"
    );

    let wrong = http
        .post(&url)
        .header("authorization", "Bearer not-the-token")
        .header("accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);

    // And a path that is not the MCP endpoint is a 404 even with the token — the auth check
    // comes first, so this proves the routing rather than the guard.
    let elsewhere = http
        .get(format!("http://{}/", url.split('/').nth(2).unwrap()))
        .header("authorization", format!("Bearer {TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(elsewhere.status(), 404);
}

/// Dropping the handle stops the listener — the Engine pattern's other half, and the only
/// way AA-03 turns the setting off.
#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_server_stops_listening() {
    let (server, url) = serve("drop").await;
    let addr = server.addr();
    drop(server);

    // The runtime shuts down in the background, so give the listener a moment to go.
    for _ in 0..50 {
        if TcpStream::connect(addr).await.is_err() {
            return;
        }
        sleep(Duration::from_millis(20)).await;
    }
    panic!("{url} still accepts connections after the server was dropped");
}
