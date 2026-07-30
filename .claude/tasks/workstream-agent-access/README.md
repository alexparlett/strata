# Workstream — Agent access (AA)

Agent-driven access to a project's data: an AI agent (Claude Code first, an in-app assistant
later) lists the catalog, inspects schemas, and runs read-only SQL — with **every agent query
landing as a real query tab** on the ordinary press → snapshot machinery, so the investigation
trail is the tab strip itself.

**Spec: `docs/AGENT_ACCESS_SPEC.md`** (+ `docs/agent-access-dataflow.mermaid`). Read it first —
it carries the settled decisions (read-only policy, agent-managed tab handles, shared
last-writer-wins tabs, one app server with default-to-single-project scoping, cached-stats-only
profiling) and the **verified** Tokio ↔ Freya bridge design every task here builds on.

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
| 03 | In-app host: service directory · bridge · agent keepers · server lifecycle | ⬜ | — | 02 |
| 04 | Settings ▸ Agent access (enable · port · token · status) | ⬜ | — | 03 |
| 05 | Headless host: `strata mcp <project>` over stdio | ⬜ | — | 01, 02 |
| 06 | Chat pane (flagship; may graduate to its own workstream) | ⬜ | — | 03 |

## Why the order

01 is pure `strata-core` and unblocks everything: without the exported policy verdict the tool
layer cannot gate `run` through the editor's own funnel, and without the extracted registration
pass the headless host would have to duplicate the Freya app's project-open sequence. 02 builds
the whole vocabulary against a **mock host** — testable without a renderer or a window — so 03
is wiring a proven surface into the app rather than debugging both halves at once. 04 is the
control for a capability 03 already ships dark (off by default). 05 is deliberately after 02,
not after 03 — it shares the vocabulary and the registration pass but none of the bridge. 06 is
the flagship and the largest: it starts with the deferred brain decision (native Anthropic
client vs Agent SDK sidecar) and reuses everything below it unchanged.

## Standing rules this workstream inherits (AGENTS.md §2)

- Agent runs are **ordinary presses**: `QuerySpec::query` is the only way a Run subscription is
  built; cache-entry lifetime is subscriber presence. The bridge adds observers, never a second
  results pipeline.
- The catalog is answered **from the store/defs, never DataFusion introspection**.
- `stopped_on_purpose` is the only thing that knows a stopped run from a failed one — the tool
  error taxonomy maps every such settle to a non-fault outcome.
- One funnel per policy: the DDL gate is the editor's own predicate, exported — never a second
  copy in the tool layer.

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core ✓]` logic in `strata-core`.
