# AM-02 · The local embedder

**Workstream:** Assistant memory · **Status:** ⬜ · **Depends on:** AM-01

## Goal

Semantic recall for every user, offline, zero config: a quantized MiniLM-class embedding model
(~25 MB, 384 dims) run locally through `fastembed`/`ort`, shipped **inside the app bundle**,
wired into `Memories` at write (embed changed rows) and at query (embed the question, fill the
fusion's semantic slot). This task also carries the workstream's release-pipeline work: the
ONNX runtime and the model become bundle deliverables of `scripts/bundle-macos.sh`, signed and
notarized with the rest.

## Current state (verified 2026-08-13)

- AM-01's store reserves `vector FixedSizeList<Float32, 384>` (nullable, null = unembedded),
  the `EMBED_DIMS` constant, the `embed_model` table-metadata key, and a fusion signature with
  an optional semantic ranking — this task fills all four and changes no shape.
- `fastembed 5.17.x` (2.6M downloads) over `ort 2.0.0-rc` (15.5M) — maturity verified on
  crates.io 2026-08-13. **Verify at build time**: the API for loading a *user-supplied* model
  from a local path (fastembed's default is a Hugging Face download at runtime, which the
  self-contained-bundle rule forbids in the shipped app), and how `ort` links ONNX Runtime
  (static vs `load-dynamic` + a bundled dylib) — pick whichever gives a signable universal
  bundle and record the choice here.
- The bundle is built by `scripts/bundle-macos.sh` (universal; also what the Release workflow
  runs), and **the app bundle is self-contained** is a standing claim each new asset must keep
  (AGENTS §7 — the font precedent: naming a new asset means embedding it in the same change).
  `docs/RELEASING.md` documents the pipeline and must stay true.
- The dependency-justification culture applies doubly here (a native-runtime dep): the
  manifest comment records why local-over-provider (the workstream README's decision) and the
  exact features taken.
- Blocking work off the render thread goes through `strata_freya::task::offload` — but this
  task's embedding all happens inside `Memories`' own runtime, so no Freya changes.

## Build

1. **Model choice + fetch**: pick the quantized 384-dim model (`all-MiniLM-L6-v2` quantized or
   fastembed's equivalent — record the exact artifact and its sha256 here). A
   `scripts/fetch-embed-model.sh` downloads it by pinned URL + checksum into a cached local
   path (dev machines and CI run it; the *app* never fetches). Decide and record where the
   cached artifact lives (`~/.cache/strata/` or under `target/`).
2. **`memory::embed`** (submodule of `strata_core::memory`): `Embedder::load(model_dir:
   &Path)` — one instance per `Memories`, loaded lazily on first use inside the facade's
   runtime (first call pays ~100–300 ms model load; nothing user-visible blocks);
   `embed_batch(texts) -> Vec<[f32; 384]>`, L2-normalized. Model resolution order: an
   explicit override env (`STRATA_EMBED_MODEL_DIR`, for dev/tests), the app bundle's
   `Resources/` (resolve like other bundle lookups), else **absent** — and absent is the FTS
   floor, logged once, never an error surface.
3. **Wire into `Memories`**: after `apply`, embed the rows the op left vector-null and merge
   the vectors in (still inside the facade — callers see nothing); at `search`, embed the
   query and hand Lance's vector ranking into AM-01's fusion slot. Write the `embed_model`
   tag into table metadata on first embed; AM-06 owns what a mismatched tag means. A row that
   fails to embed stays null and scores on the floor.
4. **Bundle + release**: `bundle-macos.sh` copies the model into `Resources/` and the ONNX
   runtime (per the linking decision in step 1 — if a dylib, into `Frameworks/`, both
   architectures, signed); notarization covers them. Update `docs/RELEASING.md` (the asset
   list) and verify a bundled `.app` embeds with the network cable pulled (manually, once —
   record the check here).
5. **CI**: the workflow runs `fetch-embed-model.sh` before tests; the embed tests **fail**
   when the model is absent rather than skipping (the container-runtime precedent: "no
   runtime" must not read as "the code is fine").
6. **Tests**: embed determinism (same text → same vector), normalization, batch = singles;
   end-to-end through `Memories` — a paraphrase query ("order lines in the raw feed") finds a
   memory worded differently ("payload.items in events") that FTS alone missed; the
   absent-model path stays on the floor and still answers.

## Acceptance

- A temp-dir store with the model present answers a paraphrase query that the FTS floor
  provably misses; with the model absent the same call still answers (floor) and logs once.
- `./scripts/bundle-macos.sh --arch arm64` produces an app whose embedder works offline.
- CI runs the embed tests with the fetched model; absence fails loud.
- `docs/RELEASING.md` names the new assets.
- Full check green.

## Files

`crates/strata-core/src/memory/embed.rs` (new) · `crates/strata-core/src/memory.rs` (wiring)
· `crates/strata-core/Cargo.toml` (fastembed + comment) · `scripts/fetch-embed-model.sh`
(new) · `scripts/bundle-macos.sh` · `.github/workflows/*` (fetch step) · `docs/RELEASING.md`
· tests beside the module.

## What is NOT this task

The model-bump re-embed lifecycle and corruption handling (AM-06 — this task only writes the
`embed_model` tag). Any prompt or loop change (AM-03/04). Choosing per-user models or
provider embeddings — deliberately rejected (workstream README); the model is an app
constant.
