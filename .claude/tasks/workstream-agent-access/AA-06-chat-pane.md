# AA-06 · Chat pane — graduated to its own workstream

**Workstream:** Agent access · **Status:** ➡ graduated · **Depends on:** AA-03

This task predicted it might graduate once started; it has. The redesign lives in
**`../workstream-assistant/`** — read its `README.md` first.

## What was settled at graduation (2026-08)

The deferred brain decision — native Anthropic client vs Claude Agent SDK / CLI sidecar — was
resolved with a third shape: **the app owns the agentic loop, and the provider is pluggable**
via the `genai` crate (Anthropic · OpenAI · Gemini · Ollama · OpenAI-compatible), chosen in
Settings. The full decision record, including the surveyed-and-rejected alternates (`llm`,
`llm-kernel`, `rig`, the SDK sidecar), is in the assistant workstream's README; the reasoning
is not restated here so it cannot drift.

Everything this file used to carry — placement, @-mentions, step cards, promotion through
`actions::open_sql`, the honest-degradation and streaming-cancel acceptance — moved into
AS-01..04 with the detail an implementing session needs. The doc records the decision in
`docs/AGENT_ACCESS_SPEC.md` under "What is not built".
