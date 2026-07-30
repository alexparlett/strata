# AA-02 · `strata-agent` crate: vocabulary + `Host` trait + rmcp server

**Workstream:** Agent access · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** AA-01

## As built (2026-07-30)

`crates/strata-agent`, a workspace member and not a default one, with **no Freya crate in its
graph** (asserted: `cargo tree -p strata-agent -e normal | grep -ci freya` = 0). Modules:
`host` (the seam + the project-resolution rule), `wire` (the JSON shapes), `tools` (the ten
tools), `error` (§7), `server` (Streamable HTTP + bearer), `mock` (a `Host` over a **real**
engine). 33 unit tests + 3 integration tests over real MCP, all green, clippy clean.

> **Verification.** `cargo test --workspace --locked` green on macOS (875 tests) after rebasing
> onto `main` at 1feeb3e — which is what made it reachable: the branch point predated the
> freya-update adaptation (a12a363 moved `VirtualScrollView`'s builder from `usize` to
> `VirtualItem`), so `strata-code-editor` did not compile until 75609da landed. Clippy clean for
> `strata-agent`.

### What the investigation settled about rmcp

Pinned **3.0.1**, features `server` + `macros` + `schemars` +
`transport-streamable-http-server` + `transport-io` (the last for AA-05, declared here so
both hosts share one pin). Four findings that shaped the code:

- **`#[tool_router]` works on a generic `impl`** (`item_impl.generics.split_for_impl()`), so
  `StrataTools<H: Host>` needs no boxed `dyn Host` adapter. That is what lets `Host` stay an
  ergonomic `async fn` trait rather than a `BoxFuture` one.
- **A tool's error shape is chosen by the error type.** `IntoCallToolResult for Result<T, E>`
  turns an `E` that yields content into an `isError` *result* and an `E` that is `ErrorData`
  into a JSON-RPC *protocol* error. Every §7 class is a condition the agent should read and
  recover from, so `AgentError` implements `IntoCallToolResult` as `CallToolResult::error` —
  protocol errors stay for malformed requests, which is rmcp's own job.
- **`StreamableHttpService` has no listener** and its `handle` is `pub`, so the bearer check
  is a plain hyper `service_fn` in front of it — 401 (and 404 off `/mcp`) answered before the
  router sees anything. No tower layer, no axum.
- Descriptions come from the tool fn's doc comment, the input schema from its
  `Parameters<T>`, the output schema from its `Json<T>` return — so the schema an agent reads
  and the type the code returns cannot drift.

### Settled shapes

1. **The `Host` trait is RPITIT with an explicit `+ Send`**, not `async fn` in trait: rmcp
   polls a handler's future on its own runtime and requires `Send`, which `async fn` in a
   trait does not promise. Implementors still write plain `async fn`.
2. **`Host::run` returns a `Result` inside a `Result`** (`RunSettle`), and the nesting is the
   point: the outer arm is "never dispatched" (no such tab, window gone), the inner one is
   the engine's own settle. Only the inner one can be a *stop*, and `stopped_on_purpose` is
   asked in exactly one place — the `run` tool — so a host cannot grow a second copy of that
   rule. It maps to `RunResult::Stopped`, a **status**, never an error.
3. **`describe_table` is its own `Host` method, not a field on the catalog rows.** A schema
   can be enormous (the 19,311-field struct this repo already has a file for), and a listing
   that carried every one of them would clone all of it to render a name and a state.
4. **`AgentError::NoProject`** is a class §7 did not have: with zero windows open, a
   project-scoped tool has to say something, and an "ambiguous" error listing nothing reads
   as a bug. Spec §7 updated.
5. **`Engine::snapshot_live` (a small `strata-core` addition)** is how `read_page` tells "your
   result was replaced" from a real read failure. A retired snapshot answers with
   DataFusion's own "table not found" prose, and matching prose at a call site is the exact
   copy-of-a-rule `stopped_on_purpose` exists to prevent. It reads `Lifecycle::stats`, which
   has a snapshot's lifetime by construction, and is asked **after** the read fails so it
   cannot race the dispatch that retired it.
6. **`read_page` does not pin.** The export window pins because it owes the user the rows it
   was opened on; an agent's cached snapshot is the opposite case — the spec wants staleness
   honest, and a pin held by a long-lived server would keep dead results alive indefinitely.
   The cache is dropped at dispatch of the tab's next `run` (an explain leaves it, since an
   explain materializes nothing) and on `close_tab`.
7. **`page_size` is `Host::default_page_size()` when unnamed and clamped to `MAX_PAGE_SIZE`
   (10000).** Clamped rather than refused, because the response reports the size actually
   used — a visible clamp, not a silent truncation. The SQL is never rewritten either way.
8. **An explain goes over the wire as text.** `QueryPlan`'s `PlanNode` list exists to be
   *drawn* (accent colours, time-share bars); off-screen it would be the same tree twice, one
   copy in a shape nothing can use. `logical_text` + `physical_text` + `analyze` is what
   `EXPLAIN` prints and what an agent reads. The **host** wraps with `plan::as_explain`,
   exactly as the app's own Run capability does — `RunMode::Explain` means "plan this
   statement", not "the caller already typed EXPLAIN".
9. **`AgentServer::start` returns `Result`** (the task's sketch did not) — a taken port must
   be something AA-04 can show, not a server that silently never listens. It binds with
   `std::net::TcpListener` rather than `rt.block_on`, because blocking on the new runtime
   panics when the caller is already inside one; the spawned task adopts it with `from_std`.
   `port: 0` is allowed and `addr()` reports what the OS chose.
10. **`mock` is public, not `#[cfg(test)]`.** The integration test lives in `tests/`, where a
    `cfg(test)` item is invisible, and the two hosts that follow are written against this
    contract. Its engine is real — a mock project registers actual tables and `run` actually
    executes — so only *what a host is* is faked. `MockProject::settling` is the one lever,
    and it exists to make "the user cancelled this" assertable without racing a real cancel.

### Wiring notes for AA-03 / AA-05

- The in-app `Host` must honour AA-01's **registration window**: `Engine::register`
  deregisters before it re-infers, so `validate` / `run` served during a scan pass answer a
  false transient "not found". Gate them behind the same `CatalogState::Scanning` claim the
  app's own validation driver uses.
- `Host::default_page_size` is where `Settings::row_limit` lands (read per call, so a change
  in Settings needs no restart). `AgentServer::start`'s port and token are AA-04's settings.
- `StrataTools::new(host)` is the reuse point: AA-05 serves the same value over
  `rmcp::transport::io::stdio`, AA-06 calls it in-process. Only `server.rs` is HTTP-specific.

## Goal
The new workspace member that owns everything frontend-agnostic: the tool vocabulary
(spec §5), the `Host` trait, the error taxonomy (spec §7), and the rmcp server (Streamable
HTTP + bearer token). Fully testable against a **mock host** — no Freya dependency, no window,
no renderer. AA-03 then wires it into the app; AA-05 reuses it headless.

## Current state
Nothing exists. `docs/AGENT_ACCESS_SPEC.md` §3–§7 is the contract; AA-01 provides the exported
policy verdict.

## What to build

### The crate
`crates/strata-agent`, added to the root workspace `members` (not `default-members`). Depends on
`strata-core`, `strata-model`, `rmcp`, `tokio`. **No Freya dependency** — that is the property
that keeps the vocabulary reusable by the chat pane (AA-06) and testable here.

Verify `rmcp` from its source before building against it (the §1 bar): pin the current release,
note the feature flags used (server + streamable-HTTP transport + stdio transport for AA-05),
and check how its tool router macros declare tools and schemas.

### The `Host` trait
The seam between the vocabulary and whoever answers it. Shape it from the spec's two planes:

- Control-plane methods (async): resolve project → list catalog → open/list/close tab →
  dispatch run and await settle. In-app (AA-03) these travel the bridge; headless (AA-05) they
  hit the engine + defs directly.
- Data-plane access: the host hands back what the server needs for engine-direct reads
  (`fetch_page`, `validate`, `functions`) — an engine handle per project.

Two impls will exist (AA-03, AA-05) plus the mock here. Don't speculate beyond what the
vocabulary needs — the trait is exactly the union of the tools' questions.

### The vocabulary
The ten tools of spec §5, with their schemas, semantics and notes-that-are-rules:

- `run` gates every statement through **AA-01's exported policy verdict before dispatch**, and
  never rewrites SQL (no injected LIMIT — response bounded by `page_size` + paging, totals
  exact).
- `list_tables` is answered by the host from store/defs — the tool layer must not offer an
  introspection fallback.
- Project resolution: default to the single open project; with more than one, error listing
  them (`project` param disambiguates). One resolution helper, used by every project-scoped
  tool.
- Error taxonomy of §7 as a typed enum mapped to MCP errors — with `stopped_on_purpose`
  (strata-core) as the only judge of stopped-vs-failed, mapped to a non-fault outcome shape.

### The server
rmcp Streamable-HTTP server bound to `127.0.0.1:<port>`, bearer-token check on every request
(401 before any tool runs). Runs on the crate's own small Tokio runtime behind a plain handle
(`AgentServer::start(port, token, host) -> AgentServer`, stop on drop) — the Engine pattern, so
the app (AA-03) starts/stops it without owning a runtime.

## Acceptance
- Unit tests over a mock `Host`: every tool's happy path; policy refusal with the editor's
  verbatim message; ambiguous-project error lists projects; unknown tab/table; stopped-on-purpose
  mapped to the non-fault shape; 401 without the token.
- One integration test speaking real MCP over the HTTP transport to a served mock host
  (rmcp provides the client side).
- `cargo test -p strata-agent` green without any Freya crate in its dependency graph
  (assert via `cargo tree -p strata-agent`).

## Notes
- Whether `explain` is `run(mode: "explain")` on the wire or its own tool: pick from spec §5
  (`mode` param) unless rmcp's schema ergonomics argue otherwise; record the choice in the spec
  if it changes.
- The port default and token format are AA-04's settings surface; here they are plain
  constructor inputs.
