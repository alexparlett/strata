# Workstream — Assistant (AS)

The native chat surface: a right-side pane in the project window where the user talks to an
assistant that investigates their data. Graduated from **AA-06** (as that task predicted it
might); the AA workstream's README links here. `docs/AGENT_ACCESS_SPEC.md` documents the
system this workstream builds on and records the pane's settled shape under "What is not
built"; the forward design lives in this folder's task files. Everything below AA (vocabulary,
bridge, policy gate, Agents pane, error taxonomy) is reused unchanged.

## The brain decision (settled 2026-08 — do not re-litigate without new facts)

AA-06 deferred one decision: native Anthropic client vs a Claude Agent SDK / CLI sidecar.
Alex resolved it with a third shape that subsumes the first and rejects the second:

**The app owns the agentic loop, and the model behind it is pluggable — one provider
abstraction crate, chosen by the user in Settings (Anthropic, OpenAI, Gemini, Ollama,
any OpenAI-compatible endpoint).**

The loop was always going to be ours: the tool layer is `strata-agent`'s vocabulary, the policy
gate runs before dispatch, runs are real runs — no vendor's loop can own that without putting a
second vocabulary between the model and `StrataTools`. What the deferred decision was really
about was the *client*, and a multi-provider client removes the vendor pin at no architectural
cost. A local Ollama model with zero keys is exactly in character for a local-first app.

### The crate: `genai` (jeremychone/rust-genai)

Chosen over three alternates, all surveyed against the same test — *we need a provider
abstraction, not an agent framework*, because the loop, the memory (the transcript), the tool
vocabulary and the MCP server are already Strata's:

- **`genai`** (v0.6.x, ~1000 commits, active) — **chosen.** Deliberately scoped to
  standardizing chat completion: multi-provider chat, `Tool`/`ToolCall`/`ToolResponse` in the
  message vocabulary, streaming that surfaces tool-call chunks (`ChatStreamEvent` /
  `ToolChunk`), per-message `CacheControl`, and resolvers (`AuthResolver`,
  `ServiceTargetResolver`) that make keys-from-config and custom endpoints (Ollama,
  OpenAI-compatible) first-class. It owns nothing we own. Its README is honest that it
  prioritizes the common surface over deep per-vendor coverage — acceptable, because the loop
  needs exactly the common surface.
- **`llm`** (graniet, v1.3.x) — rejected. The provider list is fine, but the crate's center of
  gravity is a framework: `agent`, `memory`, `chain`, an API-server mode, STT/TTS. All of that
  is either Strata's own (the loop, the transcript) or out of scope, and no example in the repo
  demonstrates streaming combined with tool calls — the one seam we most need to be real.
- **`llm-kernel`** (v0.23, pre-1.0) — rejected. An AI-app toolkit: credential management,
  vector search, knowledge graph, *its own MCP server framework*. Nearly every module
  duplicates a seam Strata already built deliberately (rmcp server, config-owned secrets),
  fewer providers, and tool calling is not clearly documented.
- **`rig`** (v0.36) — rejected, with respect. Mature and active, and it does stream with tool
  calls — but it is an agent framework: its `Agent` orchestration and its `Tool` trait would
  sit as a second loop and a second vocabulary between the model and `StrataTools`, and its
  README warns future updates will break. Same reason the SDK sidecar lost.
- **Claude Agent SDK / CLI sidecar** (AA-06's option B) — rejected by the new requirement
  itself: a sidecar pins one vendor, adds process management, and an install dependency the
  app must detect and degrade without.

Two standing cautions for every task here: **verify `genai`'s API from its source at the pinned
version before building on a summary of it** (the bar, AGENTS.md §1), and the pin is a
workspace dependency like any other — an upgrade is a deliberate change, not a drift.

## The selection split (settled 2026-08-09 — do not re-merge)

AS-03 was first written as one global provider + model + key, with an explicit "no
per-conversation model switching" line. Alex overturned that after reviewing IntelliJ's AI
Assistant: **Settings owns the roster, the chat surface owns the pick.**

- **Setup (AS-03, config):** what *addresses* each provider — enabled · base URL · key
  reference — one row per `ProviderKind`, plus the defaults a new chat starts with (provider ·
  model · effort). Slow-changing, secret-bearing, machine-scoped. The connections pattern,
  minus the secrets: keys live in the **AS-05 secret store** (OS keystore), config holds only a
  reference. "Stored like the bearer token" was the wrong precedent for third-party billing
  credentials and is withdrawn.
- **Pick (AS-04, composer footer):** provider · model · effort, per conversation, on the
  transcript satellite, seeded from those defaults. Fast-changing intent — the def/runtime
  split applied to the assistant. AS-02 takes the resolved pick **per send** and holds no
  global config, which also made its signature more testable.

**A roster of named entries was built here and withdrawn** (2026-08-10). AS-03 first carried a
`Uuid`-keyed list of entries, each with its own kind, endpoint, key and *default model*, then a
second list for custom OpenAI-compatible endpoints. Both collapsed to one row per kind: naming a
provider twice buys nothing when the thing that varies per conversation is the **model**, and the
model is not a property of the setup at all — it is picked from what the provider reports
(**AS-06**). Anything still describing entries, per-entry models or a custom-endpoint list is
older than this paragraph.

One import caution: in IntelliJ the "agent" slot picks among *external agent processes*
(Junie, Codex — ACP sidecars). Strata's analogue is the **provider row** — one loop, ours,
brains pluggable underneath. The screenshots are not an argument for ACP-style pluggable
agents; that is the sidecar shape rejected above, and per-conversation model choice validates
the `genai` decision rather than pressuring it.

## Two things AS-02 settled (2026-08-09 — do not re-litigate)

**The Agents pane is for headless MCP clients, and the assistant is not in it.** That pane
answers "which external clients are connected to my project right now"; the assistant is part of
the app, and its runs already have a richer home in the transcript. This overturns AS-01's and
AS-04's earlier notes that its sessions belonged there. Everything *below* the pane is unchanged
— it is still one more agent to `Host`, the policy gate and the query sessions. The
discriminator is `Agent::in_app`, a mark the app itself mints, never the identity's name: a
name is a claim any MCP client can make, and a name-keyed rule would let one hide itself.

**A statement the user can run is an `offer_sql` tool call, not a formatting convention.** A
tagged markdown fence was built first and withdrawn: a fence is taught only by a paragraph of
system prompt, and prompt-taught formatting is followed unevenly — least reliably by the small
local models the Ollama entry exists for. A tool is taught by its schema, and it can *check the
statement before the card appears*, which a fence structurally cannot. It is the assistant's
own eleventh tool, never registered on the router, so `tools/list` is unchanged and no MCP
client is offered a tool it has no transcript to use. SQL the assistant merely explains stays an
ordinary code block; the whole point is that the two are told apart.

## Placement and the interaction model (settled 2026-08-09 — survey on record)

Alex debated moving the chat from the right pane to a tab beside the query tabs. A
nine-surface survey settled it — DataGrip AI Assistant · DBeaver · Snowflake Copilot/Cortex
Code · Snowflake Intelligence · Databricks Assistant/Genie Code · Databricks Genie ·
MotherDuck + DuckDB UI · BigQuery Gemini/Data Canvas · Hex Notebook Agent/Threads:

**The pane stays.** No tool in the category puts chat in the tab strip. Every
SQL-author-facing assistant is a side panel or inline, because the daily loop is *anchored* —
the conversation refers to the tab, the result, the error beside it, and a tab cannot refer
to what it displaced. The dedicated-surface chats (Intelligence, Genie, Hex Threads) are
separate products for non-SQL audiences, and their autonomous execution is licensed by an
admin-curated semantic layer standing in for user review — a trust model Strata doesn't need,
because Strata's user reads SQL and the step card shows it.

The survey's residue is folded into the task files: friction-point entries + the `@tab`/
`@result` anchors + the Open / Open-and-run promote split (AS-04), the no-prose-numbers and
draft-never-execute prompt rules (AS-02), the what-leaves-the-machine note (AS-03).

**Deliberate divergences from the field** (recorded so they are not re-litigated as gaps):

- **History = adoption.** DataGrip logs AI-run queries into the shared query history for
  accountability; Strata does not — agent runs never record (`state::agents` refuses to be a
  second history), a promoted tab's own Run press records like any other. The Agents pane is
  the accountability surface. This falls out of the architecture; keep it.
- **Read-only by construction, not consent dialogs.** The field gates agentic writes with
  consent taxonomies (DataGrip's four-way consent, Databricks Allow/Skip, Junie's allowlist).
  Strata's assistant cannot write — the router refuses before dispatch, and write intents are
  drafted for the user to run. Structurally stronger than any dialog; a differentiator, not a
  gap.
- **No grids in the transcript.** The copilot cohort is unanimous (DataGrip renders CSV
  previews, Snowflake routes to the worksheet's pane, Databricks stays code-first); the rich
  inline grids live only in the business-user surfaces. Mini-table + promote stands.

**Banked for a future delegation surface** (arrives with transcript persistence — **AS-07** —
not before, and is its own task file when it does): an investigation workbench — a tab holding the
transcript plus a results pane that *subscribes the assistant's run* (a second surface
subscribes the query again; no second pipeline), for work you delegate rather than steer.
BigQuery's Data Canvas (evidence as a DAG of materialized results the chat deposits into) is
the strongest alternative shape to a linear transcript; Hex's "save Thread as a project" is
the precedent for the graduation gesture. Nothing of it is built early (§5): this paragraph
is the note.

## Architecture in one line

**genai is the mouth, `StrataTools` is the hands, the loop is ours** — one turn = stream the
model's reply; when it asks for tools, execute them through the same `StrataTools` the MCP
router serves (in-process, no MCP hop), append the results, and go again until it answers in
prose. The assistant is *one more agent* to everything below it: its own `AgentId`, its own
query sessions, the same policy gate, the same error taxonomy verbatim — and because it is
*in* the window (AA README: the §1 distinction), the tab gesture is its to keep.

## Tasks

| # | Task | Status | Depends on |
|---|---|---|---|
| 01 | In-process facade + tool manifest: the vocabulary callable without rmcp | ✅ | AA-03c |
| 02 | Provider seam + the loop: `genai`, streaming, tool dispatch, cancel | ✅ | 01 |
| 03 | Settings ▸ AI: Providers · Chat · MCP | 🟡 | 05 |
| 04 | The chat pane: transcript, selector, step cards, @-mentions, promote, stop | ⬜ | 02, 03, 06 |
| 05 | Secret store: OS-keystore-backed keys, references in config | ✅ | — |
| 06 | Model listings: a model is picked from its provider, and the list survives a restart | ⬜ | 03 |
| 07 | Conversations survive the window: the `.strata/chats/` store, the list, retention | ⬜ | 04 |

**03 is 🟡, not ✅.** Providers and MCP are done; AI ▸ Chat is two controls short — a model
`Select` (**06**) and the retention pair that only makes sense once conversations persist
(**07**). Both are additive to a working pane, and 03 closes when they land.

## Why the order

01 is pure `strata-agent` and is the load-bearing move: today every tool method is an rmcp
`#[tool]` wrapper taking `Caller`/`Peer`/`Parameters`, and the chat loop must drive the same
vocabulary with no MCP peer at all — the property the spec (§5) already promises. The
`open_session` method shows the shape: a public rmcp-free method the `#[tool]` wrapper wraps.
01 generalizes that pattern and derives the model-facing tool manifest from the rmcp router's
own tool list, so there is **one** vocabulary with two transports rather than two vocabularies.
02 builds the loop against 01 + `MockHost` — testable with no window, no renderer, and no real
vendor (point `genai`'s OpenAI-compat adapter at a local stub server rather than shaping any
production signature for a test). 05 is a pure mechanism with no dependency into the
workstream — it can land first or in parallel, and 03 consumes only its reference type, so
the pane can start before the store finishes. 03 is a settings pane in AA-04's image and runs
in parallel with 01/02. 04 is the Freya surface and comes last because everything under it is
then proven.

## Standing rules this workstream inherits

All of AA's (its README §"Standing rules"), plus:

- **The assistant is in the window, so the tab gesture is its to keep** — promotion is
  `actions::open_sql`, the same funnel the Agents pane uses. Nothing else of the window's
  state is the assistant's to touch.
- **The model sees the same errors an MCP client does** — §7's taxonomy verbatim, policy
  refusals in the editor's own words. No rewriting, no softening, and never SQL rewriting.
- **No second results pipeline.** Inline mini-results render from the run's own pages; anything
  bigger is a promote. If a sketch needs a parallel pipeline, the sketch is wrong.
- **Key/dependency absence degrades honestly**: the pane says exactly what is missing and where
  to set it (Settings ▸ Assistant), never a dead send button.

## Legend
✅ done · 🟡 partial · ⬜ todo
