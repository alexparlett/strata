# Workstream · Agent access (AA)

Agent-driven access to a project's data: **one read-only tool vocabulary** (`strata-agent`) over
a verified Tokio↔Freya bridge, with thin swappable frontends. Spec:
[`docs/AGENT_ACCESS_SPEC.md`](../../../docs/AGENT_ACCESS_SPEC.md) (as-built, dataflow diagram
inlined).

## Status

**AA-01..05 (incl. 03b/03c) ✅ done** — their files were removed with the folder when the
workstream closed; what each settled is in
[`docs/reference/SETTLED_TASKS.md`](../../../docs/reference/SETTLED_TASKS.md) and the spec. In
short: the in-app MCP server over streamable HTTP, the Agents pane (an agent's runs dispatched
straight at the engine and shown in their own surface, promotable into a **new** tab — never a
press on the user's tabs), the Settings pane, and the headless `strata mcp <project>` stdio host.

**AA-06 (the chat pane) graduated to its own workstream** —
[`workstream-assistant/`](../workstream-assistant/README.md), with its brain decision settled.

**Open:**

| Task | What |
|---|---|
| [AA-07](AA-07-bounded-answers.md) ⬜ | Tools that page or filter, and truncation that names a real recovery |

The folder is back for AA-07 alone. AA-07 is a **vocabulary** change, not an assistant one: it
moves a wire that three deployments share (in-app HTTP, headless stdio, the in-process
assistant), which is why it is not folded into AS. Read
[`docs/AGENT_ACCESS_SPEC.md`](../../../docs/AGENT_ACCESS_SPEC.md) before touching the ten, and
AA-03c's identity finding before touching query sessions.
