//! **The assistant's loop, end to end** (AS-02) — a real turn, against a real engine, with no
//! window, no renderer and no vendor account.
//!
//! The model is a stub: a local HTTP server speaking the OpenAI chat-completions wire shape,
//! reached through the roster's own **OpenAI-compatible** kind. That is the point of testing it
//! this way rather than through a resolver rigged for the test — the path exercised here is one
//! a user can configure in Settings, so nothing about the production signature is bent to be
//! testable (AGENTS.md §1).
//!
//! It streams its replies with a delay between chunks, which is what makes the ordering and the
//! cancel assertions mean anything: a body delivered all at once would settle before a stop
//! could ever land.

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{env, fs, process};

use bytes::Bytes;
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use http::{Request, Response};
use http_body_util::combinators::BoxBody;
use http_body_util::Collected;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use strata_agent::assistant::provider::ProviderKind;
use strata_agent::assistant::turn::TurnEvent;
use strata_agent::assistant::{Ask, Assistant, Conversation, Scope, Selection, Settle};
use strata_agent::mock::{MockHost, MockProject};
use strata_agent::wire::ProjectParams;
use strata_agent::{AgentIdentity, StrataTools};
use strata_core::engine::sql::Blocked;
use strata_core::engine::{Engine, TableSpec, WsId};
use strata_model::SourceFormat;
use tokio::net::TcpListener;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// The stub model
// ---------------------------------------------------------------------------

/// How long the stub waits between stream chunks. Long enough that a cancel can land in the
/// middle of a reply, short enough that a whole test is still under a second.
const CHUNK: Duration = Duration::from_millis(40);

type Body = BoxBody<Bytes, Infallible>;

/// A scripted OpenAI-compatible endpoint: one reply per request, in order, plus what it was
/// sent.
struct Stub {
    base_url: String,
    seen: Arc<Mutex<Vec<Sent>>>,
}

/// One request the stub received, in full — **the path and headers as well as the body**,
/// because they are what proves the endpoint resolver and the auth arm reached the wire. A stub
/// that answered 200 to anything could not catch a base-URL join regression at all.
#[derive(Clone)]
struct Sent {
    path: String,
    authorization: Option<String>,
    body: Value,
}

impl Stub {
    /// What the model was asked, on request `at`.
    fn request(&self, at: usize) -> Value {
        self.seen.lock().unwrap()[at].body.clone()
    }

    fn sent(&self, at: usize) -> Sent {
        self.seen.lock().unwrap()[at].clone()
    }

    fn requests(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

/// Start the stub. Each `script` is the sequence of SSE payloads one reply is streamed as;
/// `[DONE]` is appended for you.
async fn stub(scripts: Vec<Vec<String>>) -> Stub {
    serving(scripts, None).await
}

/// The same, but every request is answered with `status` and `body` instead — the transport
/// fault half, which the loop names as the only thing that fails a turn and which a stub that
/// only ever answers 200 cannot exercise.
async fn failing(status: u16, body: &'static str) -> Stub {
    serving(Vec::new(), Some((status, body))).await
}

async fn serving(scripts: Vec<Vec<String>>, fault: Option<(u16, &'static str)>) -> Stub {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .expect("stub listener");
    let addr = listener.local_addr().expect("stub address");
    let scripts = Arc::new(scripts);
    let seen: Arc<Mutex<Vec<Sent>>> = Arc::new(Mutex::new(Vec::new()));
    let next = Arc::new(AtomicUsize::new(0));

    let served = (Arc::clone(&scripts), Arc::clone(&seen), Arc::clone(&next));
    tokio::spawn(async move {
        let (scripts, seen, next) = served;
        loop {
            let Ok((io, _)) = listener.accept().await else {
                return;
            };
            let (scripts, seen, next) = (scripts.clone(), seen.clone(), next.clone());
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let (scripts, seen, next) = (scripts.clone(), seen.clone(), next.clone());
                    let path = req.uri().path().to_string();
                    let authorization = req
                        .headers()
                        .get(AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    async move {
                        let body = req
                            .collect()
                            .await
                            .map(Collected::to_bytes)
                            .unwrap_or_default();
                        seen.lock().unwrap().push(Sent {
                            path,
                            authorization,
                            body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                        });
                        let at = next.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, Infallible>(match fault {
                            Some((status, body)) => broken(status, body),
                            // **An unscripted request is a failure, not an empty reply.**
                            // Defaulting it answered `[DONE]` with no content, which the loop
                            // read as an answered turn — so a regression that made the loop ask
                            // one extra time passed every assertion in this file silently.
                            None => match scripts.get(at) {
                                Some(script) => reply(script.clone()),
                                None => {
                                    broken(500, "the loop made a request this test did not script")
                                }
                            },
                        })
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(io), service)
                    .await;
            });
        }
    });

    Stub {
        base_url: format!("http://127.0.0.1:{}/v1/", addr.port()),
        seen,
    }
}

/// One reply, streamed a chunk at a time so the loop really consumes a stream.
fn reply(script: Vec<String>) -> Response<Body> {
    let mut frames: Vec<String> = script
        .into_iter()
        .map(|payload| format!("data: {payload}\n\n"))
        .collect();
    frames.push("data: [DONE]\n\n".to_string());
    let stream = futures::stream::unfold(frames.into_iter(), |mut frames| async move {
        let frame = frames.next()?;
        tokio::time::sleep(CHUNK).await;
        Some((Ok::<_, Infallible>(Frame::data(Bytes::from(frame))), frames))
    });
    Response::builder()
        .header(CONTENT_TYPE, "text/event-stream")
        .body(BoxBody::new(StreamBody::new(stream)))
        .expect("stub response")
}

fn broken(status: u16, body: &'static str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(BoxBody::new(
            Full::new(Bytes::from(body)).map_err(|e: Infallible| match e {}),
        ))
        .expect("stub fault response")
}

fn says(text: &str) -> String {
    json!({"choices": [{"index": 0, "delta": {"content": text}}]}).to_string()
}

/// One tool call in the stream, at its **own** `tool_calls` index.
///
/// The index is the accumulator's identity for a call, not decoration: two chunks sharing one
/// index are one call being streamed in parts, which is exactly what a parallel-call fixture
/// must not say. Held at 0 once, which quietly merged every "two calls" fixture into one and
/// left the test that checks parallel answering asserting a property of a single call.
fn asks(at: usize, id: &str, tool: &str, arguments: Value) -> String {
    json!({"choices": [{"index": 0, "delta": {"tool_calls": [{
        "index": at,
        "id": id,
        "type": "function",
        "function": {"name": tool, "arguments": arguments.to_string()}
    }]}}]})
    .to_string()
}

fn ends(reason: &str) -> String {
    json!({"choices": [{"index": 0, "delta": {}, "finish_reason": reason}]}).to_string()
}

// ---------------------------------------------------------------------------
// The project under it
// ---------------------------------------------------------------------------

/// A project with a real engine holding a five-row `people` table, and the tools over it.
async fn project(tag: &str) -> (Arc<Engine>, StrataTools<MockHost>) {
    let root = env::temp_dir().join(format!("strata_assistant_it_{}_{tag}", process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("people.csv"),
        "id,name\n1,ana\n2,ben\n3,cara\n4,dev\n5,eli\n",
    )
    .unwrap();
    let project = MockProject::new("sales", &root);
    let engine = Arc::clone(&project.engine);
    engine
        .register(TableSpec {
            name: "people".into(),
            paths: vec![root.join("people.csv").display().to_string()],
            format: SourceFormat::from_name("csv"),
            partitions: Vec::new(),
            internal: false,
        })
        .await
        .unwrap();
    (engine, StrataTools::new(MockHost::new(vec![project])))
}

/// A selection pointed at the stub. The compatible kind needs a URL and nothing else, which is
/// exactly why it is the one a test can use.
fn pointed_at(stub: &Stub) -> Selection {
    Selection::new(ProviderKind::OpenAiCompatible, "stub-model").with_base_url(&stub.base_url)
}

/// Drain a turn, keeping every event.
async fn drain(running: &mut strata_agent::Running) -> Vec<TurnEvent> {
    let mut seen = Vec::new();
    while let Some(event) = running.next().await {
        seen.push(event);
    }
    seen
}

fn deltas(events: &[TurnEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::Delta(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The turn
// ---------------------------------------------------------------------------

/// **The whole loop**: a question, a tool round the model drives, and a prose answer built on
/// its result — with the events arriving in the order the transcript renders them.
#[tokio::test]
async fn a_turn_runs_a_tool_and_answers_from_its_result() {
    let (_engine, tools) = project("full").await;
    let session = tools
        .open_query_session(AgentIdentity::assistant(), ProjectParams { project: None })
        .await
        .unwrap()
        .query_session;

    let stub = stub(vec![
        vec![
            says("Let me count them."),
            asks(
                0,
                "call_1",
                "run",
                json!({"query_session": session, "sql": "select count(*) as n from people"}),
            ),
            ends("tool_calls"),
        ],
        vec![says("There are 5 people."), ends("stop")],
    ])
    .await;

    let assistant = Assistant::new().unwrap();
    let mut running = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::new(Mutex::new(Conversation::new())),
        Ask::new("How many people are there?"),
    );
    let events = drain(&mut running).await;

    // The order the transcript is built in: started, prose, the tool, its settle, more prose,
    // the settle. A delta after the turn settled would mean the pane could render text under
    // a finished turn.
    let shape: Vec<&str> = events
        .iter()
        .map(|e| match e {
            TurnEvent::Started => "started",
            TurnEvent::Delta(_) => "delta",
            TurnEvent::Runnable(_) => "runnable",
            TurnEvent::ToolCall { .. } => "call",
            TurnEvent::ToolSettled { .. } => "settled",
            TurnEvent::Settled(_) => "done",
        })
        .collect();
    assert_eq!(shape.first(), Some(&"started"), "{shape:?}");
    assert_eq!(shape.last(), Some(&"done"), "{shape:?}");
    let call = shape
        .iter()
        .position(|s| *s == "call")
        .expect("a tool call");
    let settled = shape.iter().position(|s| *s == "settled").unwrap();
    assert!(call < settled, "{shape:?}");
    assert!(shape[..call].contains(&"delta"), "{shape:?}");
    assert!(shape[settled..].contains(&"delta"), "{shape:?}");

    assert_eq!(deltas(&events), "Let me count them.There are 5 people.");

    // The step card carries the run's own facts.
    let Some(TurnEvent::ToolSettled { facts, failed, .. }) = events.get(settled).cloned() else {
        unreachable!()
    };
    assert!(!failed);
    assert_eq!(
        facts.sql.as_deref(),
        Some("select count(*) as n from people")
    );
    assert_eq!(facts.rows, Some(1));
    assert_eq!(facts.query_session.as_deref(), Some(session.as_str()));

    assert_eq!(running.settle().await, Settle::Answered);

    // Two round trips, and the second carried the first's result back to the model. The
    // assertion is on the tool message specifically: a `contains("5")` over the whole body
    // would match the session uuid, the schemas and the prompt, and so could never fail.
    assert_eq!(stub.requests(), 2);
    let second = stub.request(1);
    let messages = second["messages"].as_array().expect("messages");
    let tool = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap_or_else(|| panic!("the tool result never reached the model: {second}"));
    // Parsed as the model receives it — the tool's own JSON, not a Rust type: `RunResult` is
    // a wire *result* and serializes only, which is the honest shape to assert against here.
    let answered: Value = serde_json::from_str(tool["content"].as_str().expect("content"))
        .expect("the tool message carries the run's own JSON");
    assert_eq!(answered["status"], "ok");
    assert_eq!(answered["total"], 1);
    assert_eq!(answered["rows"][0][0], "5");

    // And the path and auth the endpoint resolver produced reached the wire.
    let sent = stub.sent(0);
    assert_eq!(sent.path, "/v1/chat/completions");
    // The empty bearer of the anonymous kind; http trims the trailing space.
    assert_eq!(sent.authorization.as_deref(), Some("Bearer"));
}

/// The tool list the model is offered is the manifest **plus** the one presentation tool — one
/// vocabulary, and an eleventh tool that exists only where a transcript does.
#[tokio::test]
async fn the_model_is_offered_the_manifest_and_offer_sql() {
    let (_engine, tools) = project("tools").await;
    let stub = stub(vec![vec![says("Hello."), ends("stop")]]).await;
    let assistant = Assistant::new().unwrap();
    let mut running = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::new(Mutex::new(Conversation::new())),
        Ask::new("hello"),
    );
    drain(&mut running).await;

    let offered = stub.request(0)["tools"].clone();
    let names: Vec<String> = offered
        .as_array()
        .expect("tools were offered")
        .iter()
        .map(|t| {
            t["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    for spec in tools.manifest() {
        assert!(
            names.contains(&spec.name),
            "{} was not offered: {names:?}",
            spec.name
        );
    }
    assert!(names.contains(&"offer_sql".to_string()), "{names:?}");
    assert_eq!(names.len(), tools.manifest().len() + 1);

    // And the system prompt went with it, byte for byte.
    assert_eq!(
        stub.request(0)["messages"][0]["content"].as_str(),
        Some(strata_agent::assistant::turn::SYSTEM)
    );
}

/// **`offer_sql` is an executable card, not a step card.** It renders as one `Runnable` and
/// produces no tool card of its own.
#[tokio::test]
async fn an_offered_statement_arrives_as_a_runnable() {
    let (_engine, tools) = project("offer").await;
    let stub = stub(vec![
        vec![
            says("Here is the query."),
            asks(
                0,
                "call_1",
                "offer_sql",
                json!({"sql": "select name from people order by name"}),
            ),
            ends("tool_calls"),
        ],
        vec![says("Run it when you like."), ends("stop")],
    ])
    .await;

    let assistant = Assistant::new().unwrap();
    let mut running = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::new(Mutex::new(Conversation::new())),
        Ask::new("Give me the names."),
    );
    let events = drain(&mut running).await;

    let runnable: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::Runnable(sql) => Some(sql),
            _ => None,
        })
        .collect();
    assert_eq!(runnable, vec!["select name from people order by name"]);
    assert!(
        !events.iter().any(|e| matches!(
            e,
            TurnEvent::ToolCall { .. } | TurnEvent::ToolSettled { .. }
        )),
        "an offer is not a step, so it gets no step card"
    );
    assert_eq!(running.settle().await, Settle::Answered);
}

/// A statement that does not check out is never shown, and the model is told why — so the card
/// the user eventually sees is one that will run.
#[tokio::test]
async fn an_offer_that_does_not_check_out_shows_nothing() {
    let (_engine, tools) = project("badoffer").await;
    let stub = stub(vec![
        vec![
            asks(
                0,
                "call_1",
                "offer_sql",
                json!({"sql": "select nope from people"}),
            ),
            ends("tool_calls"),
        ],
        vec![says("Sorry, corrected below."), ends("stop")],
    ])
    .await;

    let assistant = Assistant::new().unwrap();
    let mut running = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::new(Mutex::new(Conversation::new())),
        Ask::new("Give me the names."),
    );
    let events = drain(&mut running).await;

    assert!(
        !events.iter().any(|e| matches!(e, TurnEvent::Runnable(_))),
        "a statement that will not run must not reach a card"
    );
    let told = stub.request(1).to_string();
    assert!(told.contains("did not check out"), "{told}");
    assert_eq!(running.settle().await, Settle::Answered);
}

/// **A policy refusal round-trips.** The model sends blocked DDL, gets the editor's own message
/// back as the tool's result, and the turn still settles in prose. The refusal is the design
/// working, not the turn failing.
#[tokio::test]
async fn a_policy_refusal_reaches_the_model_and_the_turn_still_answers() {
    let (_engine, tools) = project("policy").await;
    let session = tools
        .open_query_session(AgentIdentity::assistant(), ProjectParams { project: None })
        .await
        .unwrap()
        .query_session;

    let stub = stub(vec![
        vec![
            asks(
                0,
                "call_1",
                "run",
                json!({"query_session": session, "sql": "create table t as select 1"}),
            ),
            ends("tool_calls"),
        ],
        vec![
            says("I cannot create tables. Run it in your editor."),
            ends("stop"),
        ],
    ])
    .await;

    let assistant = Assistant::new().unwrap();
    let mut running = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::new(Mutex::new(Conversation::new())),
        Ask::new("Make me a table."),
    );
    let events = drain(&mut running).await;

    let Some(TurnEvent::ToolSettled { failed, facts, .. }) = events
        .iter()
        .find(|e| matches!(e, TurnEvent::ToolSettled { .. }))
        .cloned()
    else {
        panic!("the refused run should still produce a step card");
    };
    assert!(failed);
    assert_eq!(facts.sql.as_deref(), Some("create table t as select 1"));

    // The editor's own words went back to the model.
    let told = stub.request(1).to_string();
    assert!(
        told.contains(&Blocked::CreateTable.editor_message()),
        "{told}"
    );
    assert_eq!(running.settle().await, Settle::Answered);
}

/// **Cancel mid-stream** settles as cancelled, never as failed, and never as answered.
#[tokio::test]
async fn a_cancel_mid_stream_settles_as_cancelled() {
    let (_engine, tools) = project("cancelstream").await;
    let stub = stub(vec![vec![
        says("Working"),
        says(" on"),
        says(" it"),
        says("..."),
        says(" nearly"),
        says(" there"),
        ends("stop"),
    ]])
    .await;

    let assistant = Assistant::new().unwrap();
    let conversation = Arc::new(Mutex::new(Conversation::new()));
    let mut running = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::clone(&conversation),
        Ask::new("Take your time."),
    );
    // Stop as soon as the reply has started arriving.
    while let Some(event) = running.next().await {
        if matches!(event, TurnEvent::Delta(_)) {
            running.stop();
            break;
        }
    }
    let rest = drain(&mut running).await;
    assert_eq!(rest.last(), Some(&TurnEvent::Settled(Settle::Cancelled)));
    assert_eq!(running.settle().await, Settle::Cancelled);

    // **The half-answer the user read is what the model remembers.** A stop never reaches the
    // stream's `End`, so the captured content the turn normally commits from does not exist —
    // and without the deltas being kept, a stopped turn committed nothing and the next send
    // carried on from before a question the user can still see the answer to.
    let mut second = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        conversation,
        Ask::new("Carry on."),
    );
    drain(&mut second).await;
    let carried = stub.request(1);
    let listed = carried["messages"].as_array().expect("messages");
    let spoken: Vec<&str> = listed
        .iter()
        .filter(|m| m["role"] != "system")
        .map(|m| m["content"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(spoken.first(), Some(&"Take your time."));
    assert!(
        spoken
            .get(1)
            .is_some_and(|said| said.starts_with("Working")),
        "the stopped turn's prose must be in the model's memory: {spoken:?}"
    );
}

/// **Cancel mid-run** settles as cancelled and leaves the engine clean: dropping the tool
/// future is the engine's own abort (`DispatchGuard`), so nothing is left in flight in the
/// query session's workspace.
#[tokio::test]
async fn a_cancel_mid_run_leaves_no_run_in_flight() {
    let (engine, tools) = project("cancelrun").await;
    let session = tools
        .open_query_session(AgentIdentity::assistant(), ProjectParams { project: None })
        .await
        .unwrap()
        .query_session;
    let ws = WsId::from(strata_agent::QuerySessionId(
        Uuid::parse_str(&session).unwrap(),
    ));

    // Long enough to still be executing when the stop lands.
    let slow = "select count(*) as n from generate_series(1, 400000000)";
    let stub = stub(vec![
        vec![
            asks(
                0,
                "call_1",
                "run",
                json!({"query_session": session, "sql": slow}),
            ),
            ends("tool_calls"),
        ],
        vec![says("done"), ends("stop")],
    ])
    .await;

    let assistant = Assistant::new().unwrap();
    let mut running = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::new(Mutex::new(Conversation::new())),
        Ask::new("Count a lot of rows."),
    );
    while let Some(event) = running.next().await {
        if matches!(event, TurnEvent::ToolCall { .. }) {
            // `ToolCall` is emitted *before* the dispatch, so wait for the engine's own state
            // rather than for a fixed interval — a wall-clock budget is a race on a loaded
            // machine, and the thing being waited for is observable.
            let deadline = Instant::now() + Duration::from_secs(10);
            while !engine.is_running(ws) {
                assert!(
                    Instant::now() < deadline,
                    "the run never reached the engine"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            running.stop();
            break;
        }
    }
    let rest = drain(&mut running).await;
    assert_eq!(rest.last(), Some(&TurnEvent::Settled(Settle::Cancelled)));
    assert_eq!(running.settle().await, Settle::Cancelled);
    assert!(
        !engine.is_running(ws),
        "a cancelled turn must not leave a run on the engine"
    );
    // The second request was never made: the turn stopped before it could ask again.
    assert_eq!(stub.requests(), 1);
}

/// **A selection that cannot make a client fails before a socket is opened**, naming the field
/// and the pane it is set in.
#[tokio::test]
async fn an_unconfigured_selection_names_its_field_and_never_dials() {
    let (_engine, tools) = project("unconfigured").await;
    let stub = stub(vec![vec![says("never asked"), ends("stop")]]).await;

    let assistant = Assistant::new().unwrap();
    // The compatible kind with no base URL: there is no address to call.
    let mut running = assistant.send(
        tools.clone(),
        Selection::new(ProviderKind::OpenAiCompatible, "stub-model"),
        Scope::default(),
        Arc::new(Mutex::new(Conversation::new())),
        Ask::new("anything"),
    );
    let events = drain(&mut running).await;

    let Some(TurnEvent::Settled(Settle::Failed(why))) = events.last().cloned() else {
        panic!("expected a failed settle, got {events:?}");
    };
    assert_eq!(
        why,
        "OpenAI-compatible needs a base URL. Set one in Settings > Assistant."
    );
    // Nothing was sent, and no partial turn was reported either.
    assert_eq!(stub.requests(), 0);
    assert!(!events.iter().any(|e| matches!(e, TurnEvent::Delta(_))));
}

// ---------------------------------------------------------------------------
// The shapes the loop puts on the wire
// ---------------------------------------------------------------------------

/// How much conversation a send carries. The conversation is opaque on purpose, so what a turn
/// left in it is observed the way it matters: through the next request. The system prompt is
/// not conversation — genai puts it in the same array — so it is not counted, and a turn that
/// contributed nothing therefore shows exactly one message: its own question.
fn conversation_of(stub: &Stub, at: usize) -> usize {
    stub.request(at)["messages"]
        .as_array()
        .map(|m| m.iter().filter(|m| m["role"] != "system").count())
        .unwrap_or_default()
}

/// **Every call in a round is answered, once, in order.** The loop stages one `ChatMessage`
/// for the whole round rather than one per call, because genai's Anthropic adapter emits a
/// `user` entry per Tool-role message with no merging — so N results in N messages leave the
/// entry after the assistant turn answering only the first, which Anthropic refuses.
///
/// The *framing* is invisible from here: this stub speaks OpenAI's wire, where one Tool-role
/// message is correctly flattened into one `tool` entry per call either way. What this proves
/// is the half that survives the flattening and that the bug actually cost — each call
/// answered exactly once, matched by id, with nothing between the calls and their answers.
#[tokio::test]
async fn a_round_of_parallel_calls_answers_in_one_message() {
    let (_engine, tools) = project("parallel").await;
    let stub = stub(vec![
        vec![
            asks(0, "call_1", "list_tables", json!({})),
            asks(1, "call_2", "list_functions", json!({})),
            ends("tool_calls"),
        ],
        vec![says("Both read."), ends("stop")],
    ])
    .await;

    let assistant = Assistant::new().unwrap();
    let mut running = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::new(Mutex::new(Conversation::new())),
        Ask::new("Read the catalog and the functions."),
    );
    drain(&mut running).await;
    assert_eq!(running.settle().await, Settle::Answered);

    let second = stub.request(1);
    let listed = second["messages"].as_array().expect("messages");
    // **Both calls really arrived**, checked before the property under test: every assertion
    // below is satisfied trivially by one call, so a fixture that merged the two into one
    // would report this test passing while proving nothing. It did, once — both chunks shared
    // a `tool_calls` index, which is the accumulator's identity for a call.
    let called: Vec<String> = listed
        .iter()
        .filter_map(|m| m["tool_calls"].as_array())
        .flatten()
        .map(|c| c["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        called,
        ["call_1", "call_2"],
        "the round must carry two calls"
    );

    let answered: Vec<String> = listed
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["tool_call_id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        answered, called,
        "each call is answered once, in order: {second}"
    );
    // Nothing between the assistant turn and its answers. A round whose results are separated
    // from their calls is the shape every provider rejects, whatever the framing.
    let calling = listed
        .iter()
        .position(|m| m["tool_calls"].is_array())
        .expect("the assistant's tool call");
    assert!(
        listed[calling + 1..calling + 1 + answered.len()]
            .iter()
            .all(|m| m["role"] == "tool"),
        "the answers must follow the calls with nothing between them: {second}"
    );
}

/// **The prose the model wrote alongside its tool call stays in its own memory.** genai's
/// tool-use handoff keeps signatures and calls and drops every text part, so this used to reach
/// the pane and vanish from the model.
#[tokio::test]
async fn the_models_own_words_survive_a_tool_round() {
    let (_engine, tools) = project("prose").await;
    let stub = stub(vec![
        vec![
            says("Checking the catalog first."),
            asks(0, "call_1", "list_tables", json!({})),
            ends("tool_calls"),
        ],
        vec![says("One table."), ends("stop")],
    ])
    .await;

    let assistant = Assistant::new().unwrap();
    let mut running = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::new(Mutex::new(Conversation::new())),
        Ask::new("What is in the catalog?"),
    );
    drain(&mut running).await;

    let second = stub.request(1).to_string();
    assert!(
        second.contains("Checking the catalog first."),
        "the model's narration never reached the next request: {second}"
    );
}

/// **A reply that said nothing is not an answer**, and nothing is recorded for it: an empty
/// assistant message is refused by Anthropic on every later send and cannot be removed.
#[tokio::test]
async fn an_empty_reply_fails_the_turn_and_records_nothing() {
    let (_engine, tools) = project("emptyreply").await;
    let stub = stub(vec![
        vec![ends("stop")],
        vec![says("second time lucky"), ends("stop")],
    ])
    .await;
    let conversation = Arc::new(Mutex::new(Conversation::new()));
    let assistant = Assistant::new().unwrap();

    let mut first = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::clone(&conversation),
        Ask::new("anything"),
    );
    drain(&mut first).await;
    assert_eq!(
        first.settle().await,
        Settle::Failed("The model returned an empty reply.".into())
    );

    // The next send builds on a clean conversation: its own question and nothing else.
    let mut again = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::clone(&conversation),
        Ask::new("anything"),
    );
    drain(&mut again).await;
    assert_eq!(again.settle().await, Settle::Answered);
    assert_eq!(
        conversation_of(&stub, 1),
        1,
        "the failed turn left something behind: {:#?}",
        stub.request(1)["messages"]
    );
}

/// A reply cut off by the model's output limit keeps its text and says so — presenting it as
/// finished would be the transcript claiming what the provider denied.
#[tokio::test]
async fn a_truncated_reply_settles_as_truncated() {
    let (_engine, tools) = project("truncated").await;
    let stub = stub(vec![vec![says("The answer is fo"), ends("length")]]).await;

    let assistant = Assistant::new().unwrap();
    let mut running = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::new(Mutex::new(Conversation::new())),
        Ask::new("count them"),
    );
    let events = drain(&mut running).await;
    assert_eq!(deltas(&events), "The answer is fo");
    assert_eq!(running.settle().await, Settle::Truncated);
    assert_eq!(
        Settle::Truncated.note().as_deref(),
        Some("The reply hit the model's output limit.")
    );
}

/// **A transport fault fails the turn in the provider's own words** — the one path that does
/// fail a turn, and the one a stub that only ever answers 200 could not reach.
#[tokio::test]
async fn a_provider_fault_fails_the_turn_with_its_own_message() {
    let (_engine, tools) = project("faulted").await;
    let stub = failing(401, r#"{"error":{"message":"invalid api key"}}"#).await;

    let assistant = Assistant::new().unwrap();
    let mut running = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::new(Mutex::new(Conversation::new())),
        Ask::new("anything"),
    );
    let events = drain(&mut running).await;
    let Some(TurnEvent::Settled(Settle::Failed(why))) = events.last().cloned() else {
        panic!("a 401 must fail the turn, got {events:?}");
    };
    assert!(
        why.contains("invalid api key") || why.contains("401"),
        "the provider's own words, not a type name: {why}"
    );
}

/// Nothing to send is refused before a socket opens, the way a bad selection is.
#[tokio::test]
async fn an_empty_question_never_reaches_the_provider() {
    let (_engine, tools) = project("emptyask").await;
    let stub = stub(vec![vec![says("never asked"), ends("stop")]]).await;

    let assistant = Assistant::new().unwrap();
    let mut running = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::new(Mutex::new(Conversation::new())),
        Ask::new("   "),
    );
    drain(&mut running).await;
    assert_eq!(
        running.settle().await,
        Settle::Failed("There was nothing to send.".into())
    );
    assert_eq!(stub.requests(), 0);
}

/// A cancel retires the step card it had already opened, or the transcript is left with a step
/// that never settles.
#[tokio::test]
async fn a_cancel_retires_the_card_it_opened() {
    let (engine, tools) = project("cancelcard").await;
    let session = tools
        .open_query_session(AgentIdentity::assistant(), ProjectParams { project: None })
        .await
        .unwrap()
        .query_session;
    let ws = WsId::from(strata_agent::QuerySessionId(
        Uuid::parse_str(&session).unwrap(),
    ));
    let slow = "select count(*) as n from generate_series(1, 400000000)";
    let stub = stub(vec![vec![
        asks(
            0,
            "call_1",
            "run",
            json!({"query_session": session, "sql": slow}),
        ),
        ends("tool_calls"),
    ]])
    .await;

    let assistant = Assistant::new().unwrap();
    let mut running = assistant.send(
        tools.clone(),
        pointed_at(&stub),
        Scope::default(),
        Arc::new(Mutex::new(Conversation::new())),
        Ask::new("count a lot"),
    );
    let mut seen = Vec::new();
    while let Some(event) = running.next().await {
        let opened = matches!(event, TurnEvent::ToolCall { .. });
        seen.push(event);
        if opened {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !engine.is_running(ws) {
                assert!(
                    Instant::now() < deadline,
                    "the run never reached the engine"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            running.stop();
            break;
        }
    }
    seen.extend(drain(&mut running).await);

    let Some(TurnEvent::ToolSettled { failed, facts, .. }) = seen
        .iter()
        .find(|e| matches!(e, TurnEvent::ToolSettled { .. }))
        .cloned()
    else {
        panic!("the card opened for the cancelled call was never retired: {seen:?}");
    };
    assert!(!failed, "a stop is not a fault");
    // The engine's own word for a user cancel — the same vocabulary a stopped `run` reports
    // through, rather than a sentence typed at this one call site.
    assert_eq!(facts.stopped.as_deref(), Some("cancelled"));
    assert_eq!(running.settle().await, Settle::Cancelled);
}
