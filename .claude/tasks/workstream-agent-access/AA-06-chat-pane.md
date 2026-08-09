# AA-06 · Chat pane (flagship)

**Workstream:** Agent access · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** AA-03

## Goal
The native conversation surface: a right-side pane in the project window where the user talks
to an assistant that investigates their data — with every assistant query landing as an agent
tab — the gesture a pane *in* the window may offer, where an MCP client's runs stay in
query sessions of their own (AA-03b). Spec §9 is the forward design; this task is large and may
graduate to its own workstream once started (if it does, this file becomes that workstream's
seed and the AA README links out).

## The first decision (deliberately deferred until now): the brain
Nothing below can be built until this is settled, and nothing in AA-01..05 prejudges it —
both options drive the identical tool layer on the same `strata-agent` runtime:

- **Native Anthropic API client** — the app owns the agentic loop (reqwest + the Messages API,
  streaming, tool-use turns). Full control of prompting and context assembly; costs API-key
  management in Settings and our own loop correctness.
- **Claude Agent SDK / CLI sidecar** — spawn `claude` (or the Agent SDK) as a subprocess wired
  back over the in-process tools. Reuses the user's existing subscription and Anthropic's
  context management; costs process management and an install dependency the app must detect
  and degrade honestly without.

Settle it with Alex against what exists *then* (SDK availability moves fast); record the
decision and reasoning here before building.

## Forward design (from the prior-art survey; this file is now the only copy — the spec
records the pane as unbuilt and carries no design)
- **Placement:** right-side pane (the Snowflake Copilot / DataGrip position), toggled from the
  activity rail; conversation-first; streaming responses. Panel layout rides `SessionState`
  (`Chan::Layout` / `Chan::LayoutSize`) like the drawer and sidebar.
- **Context attachment:** `@`-mention catalog objects (tables, views, saved queries) to pin
  their schemas into the conversation — answered from the same `describe_table` path, never a
  second schema serializer.
- **Execution surface:** assistant queries are real runs in their own query sessions, which
  the pane may deliberately promote into a tab (`actions::open_sql`); the transcript
  shows compact step cards (SQL · row count · elapsed) that focus the tab on press; inline
  mini-results for small answers with "open in tab" for anything bigger.
- **Rendering:** the fork ships `freya-markdown` — evaluate it for transcript rendering before
  building anything bespoke (the standard-components-first rule, one level up).
- **Register:** the assistant's user-facing text follows the app's IDE register; the pane's
  chrome follows a canvas once the designer draws one — the pane's *existence* and placement
  are settled, its dress is not.

## What is NOT this task
- No second results pipeline — if a design sketch ever needs one, the sketch is wrong
  (AGENTS.md §2).
- No loosening of the read-only policy for the in-app assistant; it is the same `Host`.
- No MCP hop — the chat loop calls the tool layer in-process.

## Acceptance (to be refined once the brain is settled)
- A conversation can: answer a schema question from `@`-mentioned context without running SQL;
  run a query that appears as a tab and cite it; recover honestly from a policy refusal
  (the assistant sees the same editor-register message an MCP client would).
- Key/dependency absence degrades honestly: the pane says exactly what is missing and where to
  set it (Settings), never a dead send button.
- Streaming cancel works (the send button becomes stop; a cancelled turn leaves the transcript
  truthful about it).
