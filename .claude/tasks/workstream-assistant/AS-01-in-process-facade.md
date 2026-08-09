# AS-01 · In-process facade + tool manifest

**Workstream:** Assistant · **Status:** ✅ · **Depends on:** AA-03c (all shipped)

## As built

`crates/strata-agent/src/tools.rs` is now two halves. The **public methods on `StrataTools`
are the ten tools** — plain arguments in, the wire result types out, no rmcp type in any
signature — and the `#[tool_router]` block is one wrapper each, doing only what a semantic
call cannot: resolving which agent the *request* is (`Caller`) and holding it against the idle
sweep (`Busy`). Every wrapper is `#[tool(name = "…")]` over a `_tool`-suffixed method, so the
wire names are unchanged and the plain names belong to the facade. A session-scoped tool has a
private `_as` core taking the `AgentId`, which is `open_query_session`'s old split generalized:
the wrapper passes the request's agent, the public method passes `self.connection.agent`.

`StrataTools::manifest() -> Vec<ToolSpec>` is the vocabulary as plain data, **derived from
`Self::tool_router().list_all()`** — the same names, doc-comment descriptions and schemars
schemas `tools/list` advertises. `AgentIdentity::assistant()` is the in-process caller's
constant identity (`strata-assistant`), owned by the crate because there is no protocol for
the assistant to introduce itself over.

**Binding a model's tool call by name to one of these methods is AS-02's**, not this task's:
the string arrives inside `genai`'s tool-call type, argument-deserialization failures need a
message that reads well *to a model*, and neither belongs in a crate that has no provider in
it. Surveyed against `longcipher/bob` and `neuron` while deciding — both build exactly the
object-safe-tool + name-keyed-registry shape rmcp's own `ToolRouter` already is, which is why
the manifest is derived from that router rather than a third registry being introduced. rmcp's
dispatch path itself is unreachable in-process (`ToolCallContext` needs a live `Peer`), and it
would answer in content blocks rather than typed values, losing the typing both crates prize.

The doc-comment audit changed nothing, as the task expected. The one line worth re-reading was
`open_query_session`'s "the user watches your sessions in the Agents pane and can promote any
query you ran into their own editor" — true on the in-process transport too, because the
assistant is one more agent and its sessions appear in that pane (AS-04 says so as well).

Tests: `crates/strata-agent/tests/facade.rs` (the vocabulary end to end with no rmcp import in
the file, the policy refusal, the manifest against the wire's own ten names and each tool's
advertised properties) and `the_manifest_is_derived_from_the_router_that_serves_mcp` in
`tools.rs`, which is the only side of the crate boundary that can see the router.

## Goal

Make the ten-tool vocabulary drivable with no MCP peer: public, rmcp-free methods on
`StrataTools`, plus a **tool manifest** (name · description · JSON schema per tool) derived
from the rmcp router's own registration — so the chat loop (AS-02) hands the model the same
tools an MCP client sees, answered by the same code, with zero copies.

## Current state

`crates/strata-agent/src/tools.rs`: every tool is an `#[tool]` method whose signature is
rmcp-shaped — `caller: Caller`, sometimes `peer: Peer<RoleServer>`, params via
`Parameters<...>`. One method already shows the target pattern: `open_session` /
`open_session_as` are public and rmcp-free, and the `#[tool]` wrapper `open_query_session`
does only what they cannot (read the client's `clientInfo` off the peer). The spec promised
this property from day one (§5: everything after `open_query_session` is addressed by
`AgentId` alone, "which is also what lets the whole vocabulary be driven with no MCP peer at
all — the property the chat pane (§9) needs").

## What to build

1. **Generalize the `open_session` pattern to the whole vocabulary.** For each tool, a public
   method taking plain arguments (the params struct is fine — those are serde types, not rmcp
   types) and returning the existing result/`AgentError` shapes. The `#[tool]` wrappers shrink
   to: resolve `Caller` → delegate. Behaviour must be byte-identical for MCP clients — the
   wrappers keep `touch`/`agent` (the idle-sweep and identity machinery), and the question of
   which of those concerns applies on the in-process path is part of the design:
   - The in-process caller is the **owned** case — the facade holds a `Connection` (minted via
     `StrataTools::connection()`), so its `AgentId` lives exactly as long as the pane's mount
     and retracts by RAII, precisely `Caller::Owned`'s semantics (AA-03c). No stateless map
     entry, no sweeper involvement — mirror how `Caller::Owned` short-circuits `agent()` today
     (`Busy::none`).
   - The assistant's `AgentIdentity` is a constant the facade owns (name it for what it is,
     e.g. `strata-assistant`) so the Agents pane shows its sessions attributed honestly.
2. **The tool manifest.** rmcp's `#[tool_router]` already generates the list the server
   advertises — names, descriptions (the doc comments), input schemas (schemars). Expose that
   list from `strata-agent` as plain data (name, description, `serde_json::Value` schema).
   **Verify from rmcp 3.0 source** how the generated router exposes its tool list (the
   `ToolRouter` value the `#[tool_handler]` uses — it is enumerable; find the real method
   rather than trusting this file). The manifest must be *derived*, never a hand-kept second
   list — a tool added to the router must appear in the manifest with no further edit.
3. **Doc-comment audit, one pass.** The tool doc comments are now model-facing prompts on two
   transports. Read them once with that in mind; they were written for exactly this register
   and likely need nothing.

## What is NOT this task

- No genai dependency, no loop, no Freya. This is `strata-agent` refactoring plus one new
  enumeration, testable entirely against `mock::MockHost`.
- No new tools, and no loosening: the facade exposes exactly the ten, policy gate included
  (the gate lives inside `run`'s body, which the facade shares — verify a blocked statement
  refuses identically through both paths).

## Acceptance

- An integration test drives the full vocabulary (open session → run → read_page → close)
  through the public facade against `MockHost`, with no rmcp types in the test body.
- The manifest test: every router-registered tool appears in the manifest with a non-empty
  description and a schema that names its params; count equals the router's count.
- `tests/mcp_over_http.rs` and `tests/mcp_over_stdio.rs` still pass unchanged — the wrappers
  delegating is invisible on the wire.
- A policy-blocked statement refused through the facade carries the same message text the MCP
  path carries.
