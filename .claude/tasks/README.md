# Strata — Freya rewrite task breakdown

The working backlog for finishing the **Dioxus → Freya 0.4** rewrite. This is the *Freya* plan:
what's built, what's a shell, what's missing, and what's next — decomposed into **per-feature
tasks inside each migration phase**, because the phases themselves are very large.

Read this index first, then open only the phase/workstream file you're working in.

## How this is organised

- **One folder per phase / workstream.** Each has a `README.md` describing the phase and indexing its
  tasks, then **one file per task** — self-contained, with enough context (current state, what to
  build, acceptance, Freya components, source files/specs) that Claude Code can pick it up and
  implement it without loading everything else.
- **Phases** followed the port plan's original 0–6 order. Tasks are **feature-level** — small
  enough to pick up, finish, and verify on a Mac build in one sitting. A **completed** phase or
  workstream has its folder removed; what each finished task settled lives in
  `docs/reference/SETTLED_TASKS.md`.
- **Workstreams** sit *outside* the linear phases — large features that cut across surfaces and
  don't belong to one phase. They have their own folders.
- Tasks carry the old **DEV_TASKS ID** (U3, W1, D4, Rz2…) where one exists. The Dioxus-era
  backlog those IDs indexed is gone from the repo; the IDs survive as feature names, in the task
  files and in `docs/reference/SETTLED_TASKS.md`.
- The **design source of truth** is the `.dc.html` canvases in `.claude/design-handoff/` (read the
  source, don't screenshot).

## The big framing (read before sizing anything)

- **The workbench is built.** Editor → run → engine → results is live end to end: the datagrid
  and tab strip, the results state machine (empty / running / grid / explain / error / statement)
  driven by freya-query off the tab's SQL, the running body, the explain-plan view, the
  status-bar pager, and the Table/Chart switcher, find, record view and copy surfaces all exist
  in the tree (`views/workbench/`). What each of those slices settled is in
  `docs/reference/SETTLED_TASKS.md`.
- **The core logic survives.** The DataFusion engine (now a direct-call async facade), the SQL
  language service (`sql`), `serialize`, `plan`, `profile`, view-deps/validity, config, and
  `.strata` persistence all live in **`strata-core`/`strata-model`** and are done. So most
  remaining Freya work is **UI + wiring**, not rebuilding logic. A task tagged `[core ✓]` means
  "the hard logic exists; build the Freya surface and wire it."
- **Freya has been a slow, learn-as-we-go build** (the datagrid alone — hover, selection, resize,
  autofit — took many iterations against Freya's reactivity/event model). Size tasks accordingly; a
  "simple" surface often carries a Freya-idiom discovery cost. Prefer **reusing + theming Freya
  built-ins** (AGENTS.md §3) over hand-rolling.

## Status legend

- ✅ **done** — built and wired in Freya (or awaiting a green Mac build).
- 🟢 **UI only** — the view exists but is a shell: on fixture data, decorative, or not dispatched.
- 🟡 **partial** — some of it works; specifics in the task.
- ⬜ **todo** — not started in Freya.
- ➡ **graduated** — the task moved to its own workstream; its file is the pointer.
- `[core ✓]` — the underlying logic already exists in `strata-core`; only Freya UI/wiring remains.

## Where we are

Completed phases have had their task folders removed; their settled record — including the
corrections that must not be re-litigated — is `docs/reference/SETTLED_TASKS.md`.

| Phase | Scope | State |
|---|---|---|
| 0 · Core extraction | `strata-model` / `strata-core` split; both frontends on the shared core | ✅ done |
| 1 · Skeleton + engine round-trip | window shell, per-window state scaffold, direct-call facade + freya-query | ✅ done |
| 2 · Workbench | editor · results grid · tabs · run/explain · find/record/copy · Table/Chart · toolbar · status bar | ✅ done (folder removed) |
| 3 · Catalog + inspector + drawer | sidebar/catalog · column inspector + profiling · the whole drawer (Problems · Events · History) | ✅ done (folder removed) |
| 4 · Multi-window | launcher · settings · export · configure · native close · write resiliency | ✅ done (folder removed) |
| **5 · Design polish** | spacing/radius tokens, hover/focus, animation, theme dial-in per surface | 🟡 **P5-06 (panel overflow) done; the rest open** → [`phase-5-design-polish/`](phase-5-design-polish/README.md) |
| 6 · Platform + parity | keymap/hotkeys · command palette · native menu · parity sweep | ✅ done (folder removed) |

## Cross-cutting workstreams (not in a single phase)

- **Connections + remote object stores** (W7) — ✅ **done (01–04), folder removed**: the
  `ConnectionDef` model in the committed `project.json`, the object stores (`engine::store`, with
  the `aws-config` credential bridge), the registration pass's connections-first phase, the
  sidebar pane, the editor window (`apps/connection/`), and the Configure window's LOCATION
  toggle (`TableDef::connection`, bucket-relative sources composed by `project::resolve_source`).
  W7-01 raised the workspace's effective MSRV to **rustc 1.94.1**. Spec:
  `docs/CONNECTIONS_SPEC.md`; settled record: `docs/reference/SETTLED_TASKS.md`.
- **Chart view** ([`workstream-chart-view/`](workstream-chart-view/README.md), **Rz2**) — the
  results Chart surface. **The core is built (00–04 ✅)**: the snapshot ordinal, the
  renderer-first `Engine::chart` read, the plotters/Skia body, the encoder strip + `ChartConfig`,
  the guardrails, Copy Image (08 ✅, which grew the fork's clipboard an image side) and the
  interactivity pass (06 ✅ — bin count, legend hide/isolate, log value axis, crosshair). Open:
  the remaining follow-ons (05, 07, 09–11 — presets, templates, shape panel, Tier B marks,
  trendline). Spec: `docs/CHART_SPEC.md`.
- **Polymorphic JSON** (WJ) — ✅ **done, folder removed**: the Postgres-style JSON accessors
  (WJ-01) and the union-tolerant `FileFormat` (WJ-02, `engine::json_poly`). Entirely
  `strata-core`; no UI surface of its own.
- **Agent access** ([`workstream-agent-access/`](workstream-agent-access/README.md), AA) —
  agent-driven access to a project's data: one read-only tool vocabulary (`strata-agent`) over a
  verified Tokio↔Freya bridge, with thin swappable frontends. **01–05 (incl. 03b/03c) ✅**: the
  in-app MCP server, the Agents pane (an agent's runs are dispatched straight at the engine and
  shown in their own surface, promotable into a **new** tab — never a press on the user's tabs),
  the Settings pane, and the headless `strata mcp <project>` stdio host. **AA-06 (the chat
  pane) graduated to its own workstream** (below). **AA-07 ⬜** reopened the folder: four of the
  ten tools answer with a list bounded only by the user's data, and the assistant's result cap
  cuts them positionally while naming a recovery three of them do not have. Docs:
  `docs/AGENT_ACCESS_SPEC.md` (as-built, dataflow diagram inlined).
- **Assistant** ([`workstream-assistant/`](workstream-assistant/README.md), AS) — the native
  chat pane, graduated from AA-06 with its brain decision settled: an app-owned agentic loop
  over a **pluggable provider seam** (the `genai` crate — Anthropic, OpenAI, Gemini, Ollama,
  OpenAI-compatible), driving the AA tool vocabulary in-process. Seven tasks: in-process facade
  + tool manifest, provider seam + loop, **Settings ▸ AI** (Providers · Chat · MCP — one row per
  provider kind saying what *addresses* it; the per-conversation pick of provider/model/effort
  lives on the pane's composer, a split settled 2026-08-09), the pane, the **secret store**
  (OS keystore; config holds references, never keys), the **model listings** a picker reads, and
  **chat persistence**. **01 ✅** (the ten tools are `StrataTools`' own public methods and
  the `#[tool]` items are wrappers over them; `manifest()` derives the model-facing offer from
  the router that answers `tools/list`), **02 ✅** (`strata_agent::assistant` — one provider
  table every surface reads, the turn loop with its event stream and cancel, and `offer_sql`,
  the assistant's own eleventh tool for handing the user a statement to execute) and **05 ✅**
  (`strata_core::secret` — a `SecretRef` in config, a `Secret` with no serde path at all).
  **06 ✅** (`strata_core::models` — the listings satellite beside the config, refreshed where a
  list is shown rather than at launch; there is no free-text model box left in the app, and the
  offer is *reported ∪ {the current pick}* so an endpoint with no `/models` cannot strand a
  working setup). **03 🟡** — Providers and MCP are done and the keystore round trip works; AI ▸
  Chat was two controls short and the model `Select` landed with 06, leaving the retention pair
  that only means something once conversations persist (07). **04, 07 ⬜.** Two corrections settled with 02 and recorded in that workstream's README: the
  Agents pane is for **headless MCP clients only** (the assistant is kept out of it by its
  minted `AgentId`), and a runnable statement is a **tool call**, not a markdown convention.
  The doc records the pane as not built (`docs/AGENT_ACCESS_SPEC.md`, "What is not built").
- **Editor statements** ([`workstream-editor-statements/`](workstream-editor-statements/README.md),
  ED) — lifting the managed-DDL policy into a full-statement editor: internal tables persisted
  under `.strata/tables/` (CTAS/INSERT/DROP), typed view DDL, typed `CREATE EXTERNAL TABLE`,
  editor `COPY TO`, session statements + `CREATE FUNCTION`. Providers for identity/visibility,
  interception for lifecycle; the agent surface stays read-only. **01–11 ✅, workstream done** —
  every intercepted statement has a real arm (ED-10 settled how a statement's `OPTIONS` split
  against connections: the reader's keys are the def's, the store's are refused toward
  Connections on the key alone), and ED-11 landed the completion offer over all of them in one
  pass. Docs: `docs/STATEMENTS_SPEC.md` + `docs/COMPLETION_SPEC.md` (the surface as built).

## Known bugs (carried from the Dioxus-era backlog; re-verify under Freya)

- **Re-opening the already-open project via Open Recent corrupts its saved paths** (relative source
  paths + partition columns mangled on next save). Was in the Dioxus `open_in_current` path — confirm
  whether the Freya session/persistence port reintroduces it (in Freya, `platform::open::decide`
  makes an own-project open a no-op, which should make the path unreachable).

(The old second entry — ⌘S on a view-bound tab saving a new saved-query — was fixed by P2-16:
`editor/actions.rs` dispatches Save on the tab's `Origin` and a view tab re-issues
`CREATE OR REPLACE VIEW`.)

## Rough order

1. **Assistant AS-04 → AS-07**, with AS-03 closing behind them. The loop and the provider seam
   are built (AS-02), Settings ▸ AI is standing (AS-03) and the model listings are in (AS-06 —
   the pane consumes `Listings::offer` and **moves** `probe::refresh` rather than writing a
   second fetch); read AS-02's "What AS-04 gets" for the event vocabulary, and AA-03c's identity
   finding before touching query sessions. Then persistence, which needs a transcript to
   persist.
2. **Agent access AA-07** — paging/filtering for the tools whose answers have no bound but the
   user's data. Independent of the one above; it is a shared-wire change, so it wants doing
   before more surfaces are built on the vocabulary as it stands.
3. **Chart follow-ons** (05, 07, 09–11) — presets, templates, shape panel, Tier B marks,
   trendline.
4. **Phase 5 polish** — the consistency + finish pass, largely theme/token work; can interleave
   with the above.

## Sourcing

Derived from the original port plan and the Dioxus-era DEV_TASKS backlog (both since removed
from the repo — what their completed tasks settled is `docs/reference/SETTLED_TASKS.md`),
`docs/FREYA_STATE_ARCHITECTURE.md` (per-window state), the `.dc.html` design canvases, and the
current `strata-freya` tree. The Dioxus app itself has been deleted; the Freya app is the only
frontend.
