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

- **Roster (AS-03, config):** named provider entries keyed by `Uuid` — kind · default model ·
  endpoint · key reference — plus one default entry. Slow-changing, secret-bearing,
  machine-scoped. The connections pattern, minus the secrets: keys live in the **AS-05 secret
  store** (OS keystore), config holds only a reference. "Stored like the bearer token" was the
  wrong precedent for third-party billing credentials and is withdrawn.
- **Pick (AS-04, composer footer):** entry · model · effort, per conversation, on the
  transcript satellite, seeded from the default. Fast-changing intent — the def/runtime
  split applied to the assistant. AS-02 takes the resolved pick **per send** and holds no
  global config, which also made its signature more testable.

One import caution: in IntelliJ the "agent" slot picks among *external agent processes*
(Junie, Codex — ACP sidecars). Strata's analogue is the **roster entry** — one loop, ours,
brains pluggable underneath. The screenshots are not an argument for ACP-style pluggable
agents; that is the sidecar shape rejected above, and per-conversation model choice validates
the `genai` decision rather than pressuring it.

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
| 01 | In-process facade + tool manifest: the vocabulary callable without rmcp | ⬜ | AA-03c |
| 02 | Provider seam + the loop: `genai`, streaming, tool dispatch, cancel | ⬜ | 01 |
| 03 | Settings ▸ Assistant: the provider roster + default entry | ⬜ | 05 |
| 04 | The chat pane: transcript, selector, step cards, @-mentions, promote, stop | ⬜ | 02, 03 |
| 05 | Secret store: OS-keystore-backed keys, references in config | ✅ | — |

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
