# Workstream — Agent access (AA)

Agent-driven access to a project's data: an AI agent (Claude Code first, an in-app assistant
later) lists the catalog, inspects schemas, and runs read-only SQL — with **every agent query a
real run** on the ordinary press → snapshot machinery, shown in the window's Agents pane and
promotable into a new tab with one press.

**Spec: `docs/AGENT_ACCESS_SPEC.md`** (+ `docs/agent-access-dataflow.mermaid`). Read it first —
it carries the settled decisions (read-only policy, agent-managed query sessions, one app server
with default-to-single-project scoping, cached-stats-only profiling) and the **verified**
Tokio ↔ Freya bridge design every task here builds on.

The architecture in one line: **one read-only tool vocabulary over one UI bridge, with thin
swappable frontends** — MCP server first (any MCP client is the chat surface), native chat pane
as the flagship follow-on, headless CLI host for app-closed use. The core (vocabulary + bridge +
policy gate) is frontend-agnostic; a chat pane's LLM loop needs the same Tokio runtime and the
same UI seam the MCP server does.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| 01 | Core seams: export the DDL-policy verdict · extract the project registration pass | ✅ | — | — |
| 02 | `strata-agent` crate: vocabulary + `Host` trait + rmcp server | ✅ | — | 01 |
| 03 | In-app host: service directory · bridge · agent keepers · server lifecycle | ✅ | — | 02 |
| 03b | The Agents pane: an agent's work is its own surface, not the user's tabs | ✅ | — | 03 |
| 03c | Seam hardening: one identity per session, per client | ✅ | — | 03b |
| 04 | Settings ▸ Agent access (enable · port · token) | ✅ | — | 03 |
| 05 | Headless host: `strata mcp <project>` over stdio | ✅ | — | 01, 02 |
| 06 | Chat pane — **graduated** to `../workstream-assistant/` (AS-01..04) | ➡ | — | 03 |

## Why the order

03c was what AA-03b's review left standing (its first item landed in the same PR), batched
because each changes a *shape* rather than a line and two of them touch the `Host` trait. It
blocked nothing, but **06 inherits its identity finding directly** and whoever starts the chat
pane should read it first: an in-process caller has no transport identity at all, which is
precisely `Caller::Owned` — the arm where a value's lifetime genuinely *is* the connection, so
`Connection`'s RAII retraction stays right. (The finding was originally written as "an
in-process caller has no `Mcp-Session-Id`". That framing was wrong in a way worth remembering:
the header is not rmcp's lifecycle discriminator, and it is absent on the very branch where
identity breaks — see the task file.)

01 is pure `strata-core` and unblocks everything: without the exported policy verdict the tool
layer cannot gate `run` through the editor's own funnel, and without the extracted registration
pass the headless host would have to duplicate the Freya app's project-open sequence. 02 builds
the whole vocabulary against a **mock host** — testable without a renderer or a window — so 03
is wiring a proven surface into the app rather than debugging both halves at once. 04 is the
control for a capability 03 already ships dark (off by default). 05 is deliberately after 02,
not after 03 — it shares the vocabulary and the registration pass but none of the bridge. 06 is
the flagship and the largest, and it graduated: the brain decision it deferred is settled
(app-owned loop over a pluggable `genai` provider seam — decision record in
`../workstream-assistant/README.md`), and the work is decomposed there as AS-01..04, reusing
everything below it unchanged.

## Standing rules this workstream inherits (AGENTS.md §2)

- Agent runs are **real executions on the project's own engine** — same snapshot lifecycle, same
  supersede, same cancel — dispatched against the query session's `WsId`. Never a second results
  pipeline, and (since AA-03b) never a press on one of the user's tabs either.
- The catalog is answered **from the store/defs, never DataFusion introspection**.
- `stopped_on_purpose` is the only thing that knows a stopped run from a failed one — the tool
  error taxonomy maps every such settle to a non-fault outcome.
- One funnel per policy: the DDL gate is the editor's own predicate, exported — never a second
  copy in the tool layer.
- **An agent's action reaches the app through the app's own funnel**: promoting a query is
  `actions::open_sql` — a new tab, focused, holding ordinary editable text. What an
  agent skips is only the *gate* in front of a funnel where that gate is a question for the user
  — never the funnel itself, or the two ways of doing one thing start to drift.
- **An agent that is not in the window does not touch the window's state** (AA-03b, reversing
  spec §1 for the MCP frontend). An MCP client is in a terminal, so its runs get their own
  surface — the Agents pane — rather than the user's tabs, which stealing focus, piling up and
  costing a validation pass each made untenable. Scoping is structural: `StrataTools` *is* one
  agent, and every session-scoped tool is scoped to its id, so an agent is never handed a handle
  on another's work. **Which** agent comes from the request rather than from how long the
  service value happens to live (AA-03c) — rmcp builds one value per *request* on its stateless
  branch, so `agent::Caller` mirrors rmcp's own lifecycle predicate and falls back to `_meta`
  `clientInfo`, refusing the session-scoped tools to a client that identifies itself as nothing
  at all. The chat pane (AA-06) is the other case and keeps the tab gesture, because it is in
  the window and the user is looking at it.

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · ➡ graduated to another workstream ·
`[core ✓]` logic in `strata-core`.
