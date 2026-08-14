# Strata — Freya rewrite task breakdown

The working backlog for finishing the **Dioxus → Freya 0.4** rewrite. This is the *Freya* plan:
what's built, what's a shell, what's missing, and what's next — decomposed into **per-feature
tasks inside each migration phase**, because the phases themselves are very large.

Read this index first, then open only the phase/workstream file you're working in.

**Before the public release, read [`PRE_RELEASE_REVIEW.md`](PRE_RELEASE_REVIEW.md)** — the
six-lens correctness/security review, what it fixed, and the handful of items deliberately left
with the reasoning for each. The "left, deliberately" section is the part to re-read before
deciding something there is a bug.

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
| 3 · Catalog + inspector + drawer | sidebar/catalog · column inspector + profiling · the whole drawer (Problems · Events · History) | ✅ done (folder removed) — the catalog pane is slated for the DB-05 tree redesign |
| 4 · Multi-window | launcher · settings · export · configure · native close · write resiliency | ✅ done (folder removed) |
| **5 · Design polish** | spacing/radius tokens, hover/focus, animation, theme dial-in per surface | 🟡 **P5-01 (spacing/radius scale) + P5-02 (interaction states) + P5-06 (panel overflow) + P5-08 (scroll acceleration) done; the rest open** → [`phase-5-design-polish/`](phase-5-design-polish/README.md) |
| 6 · Platform + parity | keymap/hotkeys · command palette · native menu · parity sweep | ✅ done (folder removed) |

## Cross-cutting workstreams (not in a single phase)

- **Connections + remote object stores** (W7) — ✅ **done (01–04), folder removed**: the
  `ConnectionDef` model in the committed `project.json`, the object stores (`engine::store`, with
  the `aws-config` credential bridge), the registration pass's connections-first phase, the
  sidebar pane, the editor window (`apps/connection/`), and the Configure window's LOCATION
  toggle (`TableDef::connection`, bucket-relative sources composed by `project::resolve_source`).
  W7-01 raised the workspace's effective MSRV to **rustc 1.94.1**. Spec:
  `docs/CONNECTIONS_SPEC.md`; settled record: `docs/reference/SETTLED_TASKS.md`. **The DB
  workstream (below) now extends this**: databases join as a fourth provider arm, the
  no-secrets rule is deliberately rewritten (keystore refs), and the sidebar pane retires
  into DB-05's data-sources tree — read the DB README before treating W7's surfaces as
  settled.
- **Chart view** (**Rz2**) — ✅ **done, folder removed**: the results Chart surface end to end —
  the snapshot ordinal, the renderer-first `Engine::chart` read, the plotters/Skia body, the
  encoder strip + `ChartConfig`, the guardrails, the interactivity pass, Copy Image (which grew
  the fork's clipboard an image side), the Tier B marks (heatmap, error band, box plot), the
  scatter trendline (`Engine::trend`) and the **Shape panel** (the aggregation composer, on the
  new shared `Modal` base). Tasks 05/07 — the command-palette chart templates — were **cut**
  (Alex, 2026-08-12): palette quick-chart entries are not wanted, and the Shape panel is the
  constructive answer. Spec: `docs/CHART_SPEC.md`; settled record (including the cut):
  `docs/reference/SETTLED_TASKS.md`.
- **Polymorphic JSON** (WJ) — ✅ **done, folder removed**: the Postgres-style JSON accessors
  (WJ-01) and the union-tolerant `FileFormat` (WJ-02, `engine::json_poly`). Entirely
  `strata-core`; no UI surface of its own.
- **Agent access** (`workstream-agent-access/`, folder removed — settled record in
  `docs/reference/SETTLED_TASKS.md`; AA) —
  agent-driven access to a project's data: one read-only tool vocabulary (`strata-agent`) over a
  verified Tokio↔Freya bridge, with thin swappable frontends. **01–05 (incl. 03b/03c) ✅**: the
  in-app MCP server, the agents satellite (an agent's runs are dispatched straight at the engine
  and never touch the user's tabs), the Settings pane, and the headless `strata mcp <project>`
  stdio host. **The Agents pane and the header's status dot were later removed on request**
  (2026-08-12): the server and the vocabulary are unchanged, and nothing in the app now shows
  who is connected or whether it is listening. **AA-06 (the chat
  pane) graduated to its own workstream** (below). **AA-07 ✅** (built 2026-08-11) closed the
  folder's last gap: the list-shaped tools were unbounded and the assistant's result cap cut
  them positionally while naming a recovery three of them do not have. Now every list answer
  is bounded with its totals stated (an answer with no totals is complete), `describe_table`
  walks a nested schema under a byte budget with path drill-down and name search, the
  assistant caps what it asks `run` for, and a cut result names the cut tool's own recovery.
  Docs: `docs/AGENT_ACCESS_SPEC.md` (as-built, dataflow diagram inlined).
- **Assistant** (`workstream-assistant/`, folder removed — settled record in
  `docs/reference/SETTLED_TASKS.md`; AS) — the native
  chat pane, graduated from AA-06 with its brain decision settled: an app-owned agentic loop
  over a **pluggable provider seam** (the `genai` crate — Anthropic, OpenAI, Gemini, Ollama,
  OpenAI-compatible), driving the AA tool vocabulary in-process. Seven tasks: in-process facade
  + tool manifest, provider seam + loop, **Settings ▸ AI** (Providers · Chat · MCP — one row per
  provider kind saying what *addresses* it; the per-conversation pick of provider/model/effort
  lives on the pane's composer, a split settled 2026-08-09), the pane, the **secret store**
  (OS keystore; config holds references, never keys), the **model listings** a picker reads, and
  **chat persistence**. The workstream is **complete**. **01 ✅** (the ten tools are `StrataTools`' own public methods and
  the `#[tool]` items are wrappers over them; `manifest()` derives the model-facing offer from
  the router that answers `tools/list`), **02 ✅** (`strata_agent::assistant` — one provider
  table every surface reads, the turn loop with its event stream and cancel, and `offer_sql`,
  the assistant's own eleventh tool for handing the user a statement to execute) and **05 ✅**
  (`strata_core::secret` — a `SecretRef` in config, a `Secret` with no serde path at all).
  **06 ✅** (`strata_core::models` — the listings satellite beside the config, refreshed where a
  list is shown rather than at launch; there is no free-text model box left in the app, and the
  offer is *reported ∪ {the current pick}* so an endpoint with no `/models` cannot strand a
  working setup). **04 ✅** (the pane: a **right rail** picking the inspector or the chat, several
  conversations per window behind a switcher, prose through the fork's `MarkdownViewer`, a
  citation card per tool round and an executable card per `offer_sql`, `@`-mentions over the
  catalog store, the three friction entries through one `ask_about` funnel, and the
  cancelled-run drop guard the task owed `agent::directory`). **03 ✅** (closed by 07's
  retention pair) — Providers and MCP are
  done and the keystore round trip works; AI ▸ Chat was two controls short and the model
  `Select` landed with 06, leaving the retention pair that only means something once
  conversations persist (07). **07 ✅** (landed 2026-08-11, closing 03's retention pair with
  it — the closing note below and this line agree now; the 🟡/⬜ marks that used to sit here
  were stale). Two corrections settled with 02 and recorded in that
  workstream's README: the Agents pane was for **headless MCP clients only** (the assistant was
  kept out of it by its minted `AgentId` — the mark survives the pane's removal, and is now what
  lets the close confirm name the assistant as itself), and a runnable statement is a **tool
  call**, not a markdown convention. One more settled with 04: the canvas's "Thought for Ns" line is **AS-02's
  to enable** — its stream loop folds reasoning chunks into the next request rather than emitting
  them, so there is no event a pane could render.
- **Editor statements** (`workstream-editor-statements/`, folder removed — settled record in
  `docs/reference/SETTLED_TASKS.md`; ED) — lifting the managed-DDL policy into a full-statement editor: internal tables persisted
  under `.strata/tables/` (CTAS/INSERT/DROP), typed view DDL, typed `CREATE EXTERNAL TABLE`,
  editor `COPY TO`, session statements + `CREATE FUNCTION`. Providers for identity/visibility,
  interception for lifecycle; the agent surface stays read-only. **01–11 ✅, workstream done** —
  every intercepted statement has a real arm (ED-10 settled how a statement's `OPTIONS` split
  against connections: the reader's keys are the def's, the store's are refused toward
  Connections on the key alone), and ED-11 landed the completion offer over all of them in one
  pass. Docs: `docs/STATEMENTS_SPEC.md` + `docs/COMPLETION_SPEC.md` (the surface as built).
- **Database connections**
  ([`workstream-database-connections/`](workstream-database-connections/README.md), DB) — 🟡
  **DB-01 + DB-02 + DB-03 ✅ (2026-08-13), DB-04 + DB-05 ✅ (2026-08-14), the rest open** —
  federated SQL over remote databases: a Postgres connection as a fourth `Provider` arm that
  registers a DataFusion **catalog** (not an object store), built on
  `datafusion-table-providers` 0.13 + `datafusion-federation` 0.5.5 (both pin our DataFusion
  54), so the editor cross-joins parquet onto live Postgres with same-source subplans pushed
  down to the server. Eight tasks: federation groundwork in `build_context` (DB-01), the
  model + secrets + pool + catalog-provider mechanism with its testcontainers-Postgres
  integration test (DB-02), the statement-policy audit over remote catalogs (DB-03), the
  connection editor's Postgres form (DB-04), the **data-sources tree** — a DataGrip-shaped
  holistic redesign of the catalog pane that absorbs and retires the Connections pane
  (DB-05) — then gestures + completion (DB-06) and inspector + profiling over remote tables
  (DB-07) on the tree, plus the JSON-accessor pushdown rewrite (DB-08), which sits directly
  on DB-02 and can land any time after it. **DB-01 is in** — `build_context` is on
  `SessionStateBuilder` with the federation rule and planner installed, the whole suite green
  with no test edited, and one correction recorded in its file: the rule is a no-op for every
  plan DataFusion can execute, but not structurally (its expression walk refuses
  `Expr::InSubquery` before consulting providers — which only changes the wording of an error
  DataFusion already raised). **DB-02 is in too** — `Provider::Postgres(PgStore)` on the def,
  `SecretRef::derived` beside `mint` (so the committed def carries only the *expectation* of a
  password), `engine::db`: the pool that is its own probe, the one-round-trip `pg_class`
  enumeration, and a lazily-listing, per-relation-cached catalog provider built through
  `PostgresTableFactory` — dispatched from `Engine::connect`/`disconnect` with `register_pass`
  untouched, plus `Engine::db_listing` (scoped and tagged) for DB-05/06 and a real-PostgreSQL
  integration test in the container CI job. One structural correction is recorded in its file:
  DataFusion's `CatalogProviderList` can register a catalog and never remove one, so the engine
  now installs its own list (`StrataCatalogList`) — without it a forgotten connection would go on
  answering for the life of the window. **DB-03 is in** — the audit held (`ddl::bare_name` really
  is the one choke point in front of every arm that resolves a target), so the work was the
  wording it mints, plus three corrections its file records: the `__snap_` namespace is the
  **workspace catalog's** and one predicate says so, a view's dependencies are **two lists**
  (workspace bare, remote qualified — or a cross-source view is indistinguishable from a
  workspace table of the same bare name), and a relation that vanishes server-side is a
  **reconciliation** whose staleness bound is stated where the message is built. The agent's
  two name-answering tools grew the honesty to match: `list_tables` names the database catalogs,
  `describe_table` answers for a three-part name. **DB-04 is in** — the picker offers
  `ProviderId::ALL`, the arm's rows are its own, and the PASSWORD row reports **this machine**
  rather than the def: a committed expectation cannot say whether an entry is here, so the row
  probes the keystore once at mount and keeps its two clearing gestures apart (a local removal is
  not a declaration that the connection has no password). Save's keystore work — migrate, then
  put or delete — runs on a worker **in front of** the store write under a new `Status::Storing`,
  which its file records as the correction to "call it at `def()`-assembly time": `blocker`
  assembles a def per keystroke.
  Read the workstream README first — it records the settled decisions, including
  the big ones: the whole database registers automatically as a catalog (no per-table defs;
  discovery gets catalogs, declaration gets defs; pinning is a view), the pane is redesigned
  while the store/discovery **invariant** underneath is not, and the connections no-secrets
  rule is **rewritten, not routed around** (passwords index the keystore by *derived*
  UUIDv5 refs, so the committed def never changes per machine — Alex, 2026-08-13). Planned
  2026-08-13 from source-verified research on both crates.
- **Updater** ([`workstream-updater/`](workstream-updater/README.md), UP) — ✅ in-app updates.
  **UP-01 ✅** (2026-08-12): the release pipeline grows a `ditto`-zipped `.app` beside the DMG,
  the app compiles in its team identity, and a signed build cross-checks the two so it cannot
  ship signed by a team its own updater would refuse. **v0.3.1 is cut** and carries the zip.
  **UP-02 ✅** (2026-08-12): the window-free mechanism — `strata_core::update` (the GitHub
  Releases check, an https-only download, `ditto` unpack, and verification against Apple's chain
  failing closed) and `state::updates` (the app-global status, the actions, the one startup
  check). The install is a quit: the press records the swap and `main` performs it after `launch`
  returns, which it does. Verified end to end against v0.3.1, refusals included.
  **UP-03 ✅** (2026-08-13): the surfaces — the launcher rail's version line, the Settings row for
  the already-landed `check_updates` field, App ▸ Check for Updates… on the menubar (asked for
  late, and the reason the offer became one pure decision), and one restart confirm mounted at
  both workspace roots. `updater::Affordance` is that decision and `updater::press` the one
  gesture over it, so no surface restates a rule. A palette row was built and then cut.
  **Workstream done.** Hand-rolled rather than Sparkle, deliberately; the workstream README
  records the settled decisions.
- **Query ergonomics**
  ([`workstream-query-ergonomics/`](workstream-query-ergonomics/README.md), QE) — planned
  2026-08-13 from field feedback on deep object-keyed JSON (the `sample/config.json` shape)
  queried through the agent surface; **the two engine UDF tasks and QE-03 are ✅, QE-04..06
  are ⬜**.
  The UDFs are `engine::udfs`, one `register` call:
  the struct family `struct_keys`/`struct_entries`/`struct_get`/`to_json` (QE-01, the whole
  fix for the dynamic-key story: enumeration off the null bitmaps and access by computed
  key, Arrow-side first, JSON text only as the heterogeneous-shape fallback), and
  `regexp_extract_all` (QE-02 — every match where DataFusion's `regexp_match` returns the
  first, so `unnest(regexp_extract_all(…))` replaces the recursive-CTE walk). Then three
  agent-surface tasks, the first of them in: **QE-03 ✅** — `describe_table` collapses N
  same-shaped UUID-keyed siblings into one `<key>` shape with its count and a few real keys,
  and `matching` answers one row with `matched_keys` rather than thousands of paths differing
  in one segment (a *cutting* strategy only: an answer that fits complete is never collapsed,
  and a leaf never joins a set, because there the names are the information). Then the stateless
  idle sweep's 5-minute TTL raised and stated to the model (QE-04), and result export as the
  spec's reserved first **curated write**, its permission model decided (Alex, 2026-08-13:
  always on, agent-supplied path — read access already hands over the data, so the fence is
  the write rules) (QE-05); QE-06 lands the guidance + workaround spellings where the model reads
  them. Two follow-ons were planned 2026-08-14 out of QE-03's review plus a probe of the
  real 62 MB fixture: **QE-07** (the schema bound as a shared `strata-core` mechanism, the
  describe ladder's depth derived rather than pinned, elided-shape counts, and the permanent
  hand-run probe — with the record-vs-map question measured closed) and **QE-08** (the
  catalog pane's cap + collapse rows, fixing the window-freezing `contentBlocks` expand —
  deliberately scheduled **after DB-05's tree**, whose task file was not edited because that
  build is an active session). The workstream README carries the **upstream ledger** — five
  reported gaps that are DataFusion 54 behaviour (pinned by the federation crates), each
  with its workaround, so nobody re-diagnoses them.
- **Internal tables in the UI**
  ([`workstream-internal-tables-ui/`](workstream-internal-tables-ui/README.md), IT) — 🟡
  **IT-01 done, IT-02 open** — an internal table could until now be created **only** by typing
  SQL, while every other verb on one (drop with its data-goes confirm, refresh, profile, ask)
  already had a surface. Two gestures, matching the classifier's own split of the two create
  kinds: **Configure's LOCATION ▸ Internal** (IT-01, `StmtKind::CreateTable`, ✅) and **Save
  results as table** beside Export (IT-02, `StmtKind::Ctas`, ⬜). Nothing ED settled
  moves: `ddl::tables` stays the one implementation and both gestures are second entries into
  it. The decision worth reading before touching the form is IT-01 §2 — the type field is
  **free text validated per row against the planner** (`Engine::column_type`), after deriving a
  picker from Arrow was investigated and rejected on three findings (no Arrow → SQL inverse, a
  config-dependent mapping, and Arrow spellings refused by the planner outright).
- **Assistant memory**
  ([`workstream-assistant-memory/`](workstream-assistant-memory/README.md), AM) — ⬜ **all
  seven tasks open, planned 2026-08-13** — per-project persistent memory for the assistant,
  so a new conversation no longer starts cold: **facts** and **SQL recipes** auto-distilled
  after each settled turn (mem0's consolidation shape — the extraction call is shown the
  related existing memories and answers with ADD/UPDATE/DELETE/NOOP ops), stored in a
  **LanceDB** table at `.strata/memory.lance/` (gitignored; pins verified identical to the
  workspace's — arrow 58 / DataFusion 54 / object_store 0.13 — so no second DataFusion),
  embedded by a **bundled local model** (fastembed/ort, ~25 MB, 384 dims, an app constant —
  chosen over `genai`'s embed API because Anthropic/Groq have none), and recalled two ways:
  a budgeted `Project memory` context block on every send and a `search_memory` tool
  appended like `offer_sql` (assistant-only, never on the MCP router). Retrieval is
  four-signal fusion (vector + BM25 FTS, both Lance's, + table-tag entity boost + recency);
  the non-vector signals are the always-works floor, so nothing in the memory path can ever
  fail a turn. Seven tasks: the store facade (AM-01), the embedder + bundle assets (AM-02,
  carries the release-pipeline risk), extraction + the `Ai::memory_enabled` toggle (AM-03,
  the earliest end-to-end demo), recall (AM-04), the prune panel off the chat header
  (AM-05), lifecycle hardening (AM-06), spec + end-to-end (AM-07). The README records the
  crate evaluations that settled the design — five hobby RAG crates,
  `datafusion-index-provider` (a prototype, wrong problem), redb/rusqlite (storage only) —
  do not re-litigate them.

## Known bugs (carried from the Dioxus-era backlog; re-verify under Freya)

- **Re-opening the already-open project via Open Recent corrupts its saved paths** (relative source
  paths + partition columns mangled on next save). Was in the Dioxus `open_in_current` path — confirm
  whether the Freya session/persistence port reintroduces it (in Freya, `platform::open::decide`
  makes an own-project open a no-op, which should make the path unreachable).

(The old second entry — ⌘S on a view-bound tab saving a new saved-query — was fixed by P2-16:
`editor/actions.rs` dispatches Save on the tab's `Origin` and a view tab re-issues
`CREATE OR REPLACE VIEW`.)

## Rough order

1. **Database connections (DB)** — the new capability workstream, eight tasks: DB-01 ✅,
   DB-02 ✅, DB-03 ✅ and DB-04 ✅, so **DB-05 (the tree redesign, the heaviest task) and DB-08
   are next**; DB-08 sits on DB-02 alone and needs nothing after it, so schedule it early. DB-06
   and DB-07 close on the tree.
2. **Query ergonomics (QE)** — eight tasks; QE-01, QE-02 and QE-03 are in, so QE-04 and
   QE-05 are next in either order, QE-06 lands after them (its guidance names QE-01's
   functions), QE-07 follows QE-03's merge, and QE-08 waits for DB-05's tree.
3. **Internal tables in the UI (IT)** — the one remaining task, IT-02 (Save results as
   table); small, sits on nothing open.
4. **Assistant memory (AM)** — seven tasks; AM-01 → AM-02 → AM-03 is the critical path to
   the first demo (AM-03 works FTS-only if AM-02 lags), then AM-04/AM-05 in either order,
   AM-06, AM-07 closes. AM-02 carries the release-pipeline work (bundled model + ONNX
   runtime) — start it early enough that a release isn't waiting on notarization surprises.
5. **Phase 5 polish** — the consistency + finish pass, largely theme/token work.

(Every other workstream is closed: **Connections/W7**, **Polymorphic JSON**, **Agent
access**, **Editor statements**, the **Assistant** — AS-07 landed 2026-08-11 with AS-03
behind it — the **Chart** workstream, 09/10/11 built and 05/07 cut on 2026-08-12, and the
**Updater**, UP-03 landing its surfaces on 2026-08-13. The open workstreams above are DB,
QE, IT and AM.)

## Sourcing

Derived from the original port plan and the Dioxus-era DEV_TASKS backlog (both since removed
from the repo — what their completed tasks settled is `docs/reference/SETTLED_TASKS.md`),
`docs/FREYA_STATE_ARCHITECTURE.md` (per-window state), the `.dc.html` design canvases, and the
current `strata-freya` tree. The Dioxus app itself has been deleted; the Freya app is the only
frontend.
