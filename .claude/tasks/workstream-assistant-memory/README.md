# Workstream: Assistant memory (AM)

Per-project, persistent memory for the in-app assistant, so a new conversation no longer starts
cold. Today the system prompt is deliberately constant (byte-identical for provider prompt
caches) and carries **no** project knowledge; everything the assistant learned in one chat — how
a nested `payload.items` unnests, what a column means, which table holds what — is gone when the
next chat opens. This workstream distills **facts** and **SQL recipes** from settled turns into a
[LanceDB](https://github.com/lancedb/lancedb) store under `.strata/memory.lance/`, embeds them
with a **bundled local model**, and recalls them into new conversations two ways: a budgeted
context block on every send, and a `search_memory` tool the model can call mid-turn. The
consolidation architecture is [mem0](https://docs.mem0.ai/core-concepts/how-it-works)'s,
adopted as a pattern on Strata's own substrate.

Seven tasks. AM-01 is the store and carries the crate; AM-02 is the embedder and carries the
release-pipeline work; AM-03 (capture) sits on AM-01 and is the earliest end-to-end
demonstration — it works FTS-only if AM-02 hasn't landed; AM-04 (recall) sits on AM-03; AM-05
(the prune panel) sits on AM-03 independently of recall; AM-06 (lifecycle hardening) sits on
AM-02 + AM-03; AM-07 closes with the spec and the end-to-end test.

## Decisions already made (do not re-litigate; the reasoning is recorded here)

- **The store is LanceDB, and its version pins were verified before it was chosen**
  (crates.io, 2026-08-13). `lancedb 0.37.1` pins `arrow ^58.0.0`, `datafusion ^54.0.0`,
  `object_store ^0.13.2` — exactly the workspace's own lockstep set, so no second
  DataFusion/Arrow enters the graph (the `datafusion-variant` rejection, `engine/udfs.rs:26-32`,
  does not apply). It answers the two concerns that killed the first design (a whole-file JSON
  satellite): incremental add/update/delete instead of a rewrite per settled turn, and a real
  index path (ANN) if a store ever outgrows brute force — plus native BM25 full-text search, so
  the lexical retrieval signal comes from the store rather than hand-rolled scoring.
- **The alternatives were evaluated on evidence, not vibes** (all measured 2026-08-13).
  Five RAG/memory crates Alex raised — `adk-rag` (2.4k downloads, a different agent framework's
  RAG module, wants Qdrant/LanceDB/pgvector servers), `wicked-estate-retrieve` (998, one
  person's project tool), `rustmem` (66), `memcontext` (60), `mem0-rust` (55) — are hobby-scale
  ports, each bundling its own LLM client beside `genai`.
  [`datafusion-index-provider`](https://github.com/datafusion-contrib/datafusion-index-provider)
  (16 stars, self-described "prototype approaches to implementing indexes in DataFusion") is
  equality/range index acceleration — no vector, no FTS, no storage; wrong problem. `redb` (9M)
  and `rusqlite` (92M) solve storage only and leave every retrieval signal hand-rolled. mem0's
  **architecture** is what gets adopted; none of its implementations do.
- **The embedder is a bundled local model, not a provider API** (Alex, 2026-08-13, after both
  options were laid out; dependency weight explicitly ruled not a concern). `fastembed` (2.6M
  downloads) over `ort` (15.5M), running a quantized MiniLM-class model (~25 MB, **384 dims**)
  shipped **inside the app bundle** — the self-contained rule (AGENTS §7) makes the model and
  the ONNX runtime first-class bundle deliverables, and forbids runtime downloads. Why not
  `genai`'s embed API (which the pinned 0.7.0-beta.18 does ship, unused): Anthropic and Groq
  have **no** embeddings endpoint, and `Provider::adapter()` routes gpt-5-family models to
  `OpenAIResp`, which `AdapterNotSupported`s embeddings too — so the provider path needs an
  extra embedding-provider Settings surface, goes dark for Anthropic-only setups, dials the
  network on every send, and re-embeds the store whenever config moves. Local kills all four:
  semantic recall for every user, offline, zero config, one embedding space whose identity is
  an **app constant** — it changes only when a release deliberately bumps the bundled model.
- **Retrieval is four signals, fused** — mem0's model
  ([how-it-works](https://docs.mem0.ai/core-concepts/how-it-works) +
  [memory-evaluation](https://docs.mem0.ai/core-concepts/memory-evaluation), read 2026-08-13):
  vector similarity + BM25 full-text (both Lance's) + **entity boost** (each memory carries the
  folded table/column names it touches, matched against what the conversation is about — in a
  SQL tool this is a stronger signal than mem0's generic entities) + recency. Weighted RRF over
  ranks, not score normalization — robust without per-signal calibration. Injection is
  **budgeted** (top-k ≤ 6 under 4 KB), mem0's token-efficiency lesson.
- **Capture is auto-extraction with consolidation in the call.** After a turn settles, a
  background call (the conversation's own provider/model/key — already read for the turn, no
  second keystore read, no new consent surface) is shown a bounded rendering of the exchange
  **plus the top-k existing related memories** under short handles (`m1`…`m10` — models mangle
  UUIDs; the caller keeps the map), and answers through one schema-taught `memory_ops` tool
  with **ADD / UPDATE / DELETE / NOOP** operations — dedup and correction happen inside the
  call, mem0's shape. One parse attempt; failure is a tracing line and an untouched store.
- **Two memory kinds, and a recipe's SQL is verbatim.** `fact` (schema shape, column meaning,
  project conventions) and `recipe` (working SQL exactly as it ran + a retrieval-oriented
  description). Paraphrasing SQL destroys the artifact; the description exists so retrieval
  matches phrasings the SQL itself doesn't contain.
- **Recall is both halves, and the tool is assistant-only.** The injected block rides
  `Ask.context`'s existing fenced, injection-hardened channel (`turn.rs:172,181`) — on the user
  message, not the system prompt, so provider caches hold. `search_memory` is appended in
  `turn::offered()` exactly as `offer_sql` is (`turn.rs:685`) and is **never on the MCP
  router** — the memory is the assistant's own notebook, not the app's data; `tools/list`
  stays byte-identical.
- **Per-project, personal, gitignored.** `.strata/memory.lance/` beside `chats/` — consistent
  with conversations; colleagues' embeddings and stores never have to agree. The prune surface
  is a **modal working panel off the chat pane header** (per-project state lives in the project
  window; Settings is app-global and gets only the `Ai::memory_enabled` toggle, default **on**
  — Alex, 2026-08-13).
- **Nothing in the memory path may ever fail a turn.** Extraction failure is a tracing line,
  never a transcript entry; an unopenable store means memory is off for that window (logged,
  never a dialog); embedding failure leaves rows FTS-only. The tier statement, adapted from
  `chat_store.rs:23-28`: *the worst outcome is losing what the assistant learned, never what
  the user wrote.*
- **The facade is `strata_core::memory::Memories`, in the Engine's own shape** (a private
  runtime, direct-call async methods, callers await — `engine/mod.rs:442`'s pattern), because
  LanceDB is async and Freya's render executor is not Tokio. Embedding happens **inside** the
  facade at write and at query; no caller ever sees a vector. `lancedb` and `fastembed` are
  `strata-core` dependencies (the DataFusion-only-in-core invariant holds — lance's internal
  DataFusion stays inside that boundary).
- **Two prompt deliverables, both static files** so the byte-identical-system-prompt rule
  survives: `system.md` grows a constant "Project memory" section (what the injected block is,
  when to call `search_memory` — AM-04), and the distiller gets its own `distill.md` (what is
  durable, what is never stored — credentials, one-off result values, anything about the user
  rather than the project — and the ops semantics; AM-03).

## Known risks (verify at the named task, not before)

- **`lancedb` pins `lance =10.0.0` exactly**, so the documented lockstep set
  (root `Cargo.toml`) grows by its largest member: a future DataFusion 55 bump waits until
  lancedb tracks it (historically weeks). The dependency comment must say so (AM-01).
- **The Rust SDK's FTS surface** — verify at AM-01 that lancedb's Rust API exposes the
  inverted-index/BM25 search the docs advertise (it is newer than the vector path). If it
  falls short, the fallback is scoring a scanned batch in the facade — record whichever is
  built in the task file.
- **`ort` + model in the universal bundle** is the release-risk item (AM-02):
  the ONNX runtime library must ship for both architectures, signed and notarized with the
  rest (`scripts/bundle-macos.sh`), and `fastembed` must load the model from a **local path**
  (verify its user-supplied-model API) — never the network, in app or in CI. CI fetches the
  model by pinned checksum; a missing model **fails** the embed tests rather than skipping
  (the no-runtime-must-not-look-like-fine rule, CLAUDE.md).
- **`Ai::memory_enabled` rides `settings_merge!`** — the exhaustive macro means forgetting the
  Settings row is a compile error, but the default-on choice means the first release turns
  extraction on for existing users; the toggle row's subtext must say what it spends (one
  small model call per settled turn).

## Tasks

| # | Task | Status | Depends on |
|---|---|---|---|
| AM-01 | The memory store (`strata_core::memory` over LanceDB) | ⬜ | — |
| AM-02 | The local embedder (fastembed + bundle assets) | ⬜ | AM-01 |
| AM-03 | Extraction + consolidation (distill, the hook, the toggle) | ⬜ | AM-01 (AM-02 optional) |
| AM-04 | Recall (injection block + `search_memory`) | ⬜ | AM-01, AM-03 |
| AM-05 | Memory management panel | ⬜ | AM-03 |
| AM-06 | Lifecycle hardening (re-embed, corruption, Clear) | ⬜ | AM-02, AM-03 |
| AM-07 | Spec + end-to-end | ⬜ | all |

Planned 2026-08-13 from source-verified exploration of the assistant loop, the satellite
precedents and the pinned `genai`/`lancedb` crates, plus mem0's architecture docs (read
2026-08-13). Docs to keep true as tasks land, **each owned by a task**: AM-01 —
`docs/reference/MODULE_MAP.md`; AM-02 — `docs/RELEASING.md` (the bundle gains assets);
AM-03/AM-04 — `assistant/distill.md` and `assistant/system.md` themselves, plus
`docs/AGENT_ACCESS_SPEC.md`'s "What is not built" (the assistant-only tool count changes);
AM-07 — `docs/MEMORY_SPEC.md` (new, as-built), its `docs/README.md` index row, and this
folder's closeout in `.claude/tasks/README.md`.
