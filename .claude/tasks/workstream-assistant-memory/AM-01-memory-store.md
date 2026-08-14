# AM-01 · The memory store

**Workstream:** Assistant memory · **Status:** ⬜ · **Depends on:** nothing

## Goal

`strata_core::memory`: a `Memories` facade over a LanceDB table at `.strata/memory.lance/` —
the record vocabulary, the one `apply(ops)` write funnel, and hybrid `search` (FTS + entity +
recency fusion, with a vector slot that stays inert until AM-02). Pure infrastructure: no
genai, no UI, no engine involvement. After this task the store can be created, written,
searched and reopened from a test with nothing else built.

## Current state (verified 2026-08-13)

- No memory/vector/similarity code exists anywhere in the workspace (grepped). `strata-core`'s
  modules are `ai, config, engine, keymap, models, project, register, secret, theme, update,
  util` — this module sits beside `models.rs`, whose doc (models.rs:7-12) states the satellite
  rule being followed: a cached/derived artifact is never a config field.
- Satellite path helpers live in `project.rs` (`history_path:278`, `chats_dir:404`);
  `ensure_gitignore` (:517) writes `.strata/.gitignore`'s entry list; `tidy_strata_dir` (:495)
  sweeps `write_atomic` temps — Lance manages its own directory contents, so it needs a
  gitignore line but **no** temp-sweep entry.
- The facade shape to copy is the Engine's: a private multi-thread Tokio runtime
  (`engine/mod.rs:442`), each public method cloning what it needs and
  `self.rt().spawn(async move {…}).await` — executor-agnostic, so Freya's non-Tokio executor
  awaits it directly (no `task::offload` needed for calls; only for anything blocking at
  construction).
- `lancedb 0.37.1` pins `arrow ^58.0.0`, `datafusion ^54.0.0`, `object_store ^0.13.2`,
  `lance =10.0.0` (crates.io, 2026-08-13) — the workspace's exact set. The dependency comment
  must record the lockstep consequence (a DF bump now waits on lancedb) beside the existing
  lockstep note in the root `Cargo.toml`.
- `fold_ident` is the identifier folding every name-keyed map in the engine uses — the
  `tables` tags must store folded names so the entity match agrees with how the catalog keys.

## Build

1. **The dependency**: `lancedb = "0.37"` in `crates/strata-core/Cargo.toml`, with the
   manifest-culture justification comment: why this crate (evaluated field is in the
   workstream README), the pin lockstep (`lance =10.0.0` exact → joins the DataFusion
   lockstep set as its largest member), and which features are off. Extend the root
   `Cargo.toml` lockstep comment.
2. **The vocabulary** in `crates/strata-core/src/memory.rs`:
   `MemoryKind { Fact, Recipe }` (stable serde spellings, asserted like `ai.rs:265`);
   `Memory { id: Uuid, kind, text: String, sql: Option<String>, tables: Vec<String>
   (folded), source_chat: Uuid, created_ms: u64, updated_ms: u64 }`;
   `MemoryOp { Add {kind, text, sql, tables}, Update {id, kind, text, sql, tables},
   Delete {id}, Noop }`. The Arrow schema constant beside them: the columns above plus
   `vector FixedSizeList<Float32, EMBED_DIMS>` (nullable — null = not yet embedded;
   `EMBED_DIMS = 384`, the constant AM-02 fills) and table metadata keys for the embedder
   tag (`embed_model`, written by AM-02, absent until then).
3. **The facade**: `Memories::open(root: &Path) -> Result<Memories, String>` — private
   2-worker runtime (Engine's builder shape), `lancedb::connect` on `memory_path(root)`
   (new helper in `project.rs` beside `chats_dir`; add the `.gitignore` line to
   `ensure_gitignore`'s array), create-table-if-absent with the schema, create/refresh the
   FTS index on `text` (+ `sql`) — **verify here** that the Rust SDK exposes the
   inverted-index/BM25 search; if it does not, score a scanned candidate batch in the facade
   instead and record which was built in this file. An open failure returns the error string;
   the *caller* decides it means memory-off-for-this-window (AM-03/AM-06) — `open` never
   panics and never creates on an unreadable root.
4. **The write funnel**: `Memories::apply(ops: Vec<MemoryOp>) -> Result<Applied, String>` —
   the only writer. Validates (unknown `Update`/`Delete` target → op skipped, counted in
   `Applied`), stamps `created_ms`/`updated_ms`, inserts/merges/deletes through Lance's own
   incremental ops (no whole-table rewrite), leaves `vector` null on add/update (a text
   change **nulls** the row's vector — stale semantics AM-02 relies on). Returns what
   changed so callers can log and AM-02 can embed.
5. **Search**: `Memories::search(query: &str, context_tables: &[String], k: usize)
   -> Result<Vec<Hit>, String>` where `Hit { memory: Memory, score: f32 }`. Signals: FTS
   rank over `text`+`sql`; entity boost — overlap between `context_tables` (folded) and the
   row's `tables`; recency — `updated_ms` decay as the tie-break tier. Fusion is weighted
   RRF over ranks with the weights as named constants in one place, and a **vector-rank
   slot that is `None` until AM-02** wires it (the §5 inert-seam rule — the fusion signature
   takes the optional semantic ranking now so AM-02 changes no shape). A candidate with no
   signal at all never surfaces.
6. **Tests** (temp-dir store, no network, no runtime beyond the facade's own): apply
   add/update/delete/noop + unknown-target skip; text change nulls the vector; reopen sees
   the same rows; search finds by exact identifier (`events`, `sku`), by entity overlap
   when the text doesn't match, and ranks a recently-updated hit over a stale equal; the
   fusion function unit-tested with and without a semantic ranking.

## Acceptance

- A test creates a store in a temp dir, applies ops, reopens, and searches — incremental
  writes verified by applying 100 single ops without a rewrite-per-op blowup.
- `search` with `context_tables` surfaces a memory tagged for the conversation's table even
  when the query text shares no words with it.
- The vector column and the fusion's semantic slot exist and are exercised as absent —
  AM-02 fills them without changing any signature.
- The dependency comments record the lockstep consequence.
- Full check green.

## Files

`crates/strata-core/src/memory.rs` (new) · `crates/strata-core/src/lib.rs` (module) ·
`crates/strata-core/src/project.rs` (`memory_path`, `ensure_gitignore` entry) ·
`crates/strata-core/Cargo.toml` + root `Cargo.toml` (dep + lockstep comments) ·
`docs/reference/MODULE_MAP.md` (the new module) · tests beside the module.

## What is NOT this task

Embedding and vectors (AM-02 — this task only reserves the column, the dims constant and
the fusion slot). The distill call, any hook into the chat loop (AM-03). Any UI (AM-05).
Re-embed/corruption lifecycle beyond `open`'s honest error (AM-06).
