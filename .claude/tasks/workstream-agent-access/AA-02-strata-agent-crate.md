# AA-02 · `strata-agent` crate: vocabulary + `Host` trait + rmcp server

**Workstream:** Agent access · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** AA-01

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
