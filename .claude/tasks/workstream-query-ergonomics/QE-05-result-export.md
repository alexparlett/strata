# QE-05 · Agent result export — the first curated write

**Workstream:** Query ergonomics · **Status:** ✅ · **Depends on:** nothing

## Goal

An agent can land a settled result on disk without a Python-CSV detour (feedback item 12):
`export_result(query_session, path, format)`, always available, never a loosening of `run`
(docs/AGENT_ACCESS_SPEC.md:437-439). This is the first curated write, so its shape is the
precedent for any later one.

## The decision (Alex, 2026-08-13)

**Always available, no consent surface, agent-supplied path.** The candidate Settings toggle
was declined on a correct observation: `read_page` already hands the agent every byte of the
result, so a gate on writing those bytes to the user's own disk protects no data the read
surface has not already exposed. (Per-call confirmation was never a candidate — a tool call
must not block on a dialog, the settled reason profiling isn't exposed at all, spec
:183-186.)

What still needs protecting is the **write**, not the data, so the whole fence is the path
rules:

- The settled resolved-target gate: never into `.strata/` or the snapshot spool (a stray
  file under an internal table's directory is phantom rows on its next scan) — the same gate
  `ddl::copy` runs, reached the same way.
- **Refuse overwriting an existing file**, by name, with no overwrite flag in v1. This is
  the one genuinely new risk of an agent-supplied path (a driven client clobbering the
  user's files); minting a fresh name is the agent's job and a refusal is cheap.
- Parent directory must exist — export creates a file, never a tree.

This deliberately relaxes the spec's reserved wording ("separately permissioned tools"):
the permission turned out to be a fiction once read access is total, and the spec's curated-
writes paragraph is rewritten in this task to say what the real fence is. `run`'s policy,
`Blocked::CopyTo` and the read-only classification are untouched — the write is a new tool
whose only effect is a file on the user's own disk, outside Strata-owned storage.

## Current state (verified 2026-08-13)

- No write/export method exists anywhere on the seam: `Host` has ten methods, none write;
  `Blocked::CopyTo` refuses the agent's `COPY` at classification
  (`sql/validate.rs:532`, agent arm `:510-514`), and stays as-is — this task adds a tool, it
  does not touch `run`'s policy.
- The engine already owns everything the tool needs:
  - The session's result is a snapshot the agent already addresses (`LastRun.snapshot`,
    `tools.rs:124-145`) — the export source is "this query session's settled result",
    which dodges re-running anything.
  - `COPY`'s target gate is settled and reusable: the *resolved* target may not land in
    `.strata/` or the snapshot spool (`ddl::copy`), `__strata_ord` must be projected away
    (AGENTS.md: "export must never write it"), and a write that outlives its caller holds
    the export guard (`ExportHold` / `Lifecycle::background`) so a closing window asks
    before taking the runtime away.
  - The Export window's engine side and `ddl::copy::copy_to` are the two existing funnels;
    the tool is a third gesture into one of them, never a third implementation. Likely
    shape: an `Engine` method that plans `SELECT … FROM __snap_n ORDER BY __strata_ord`
    internally (engine-minted, so the `ReservedName` fence — which exists to stop *user*
    SQL reaching snapshots — is not in the path) and drives the existing COPY body at the
    fenced destination.
- Session/snapshot lifetime interacts with QE-04: an export tool reads the session's
  snapshot, and the `Busy` guard on the call already holds the agent against the sweep for
  the duration.

## Build

1. `Host::export_result(session, path, format) → written path` (the driver asks the engine;
   the headless host answers the same from its own engine — same vocabulary, second
   deployment). Formats: csv / parquet / ndjson, reusing `EXPORT_OPTIONS`' vocabulary where
   it fits.
2. `StrataTools::export_result` public method + `#[tool]` wrapper (Busy guard + Caller
   resolution, then delegate — the settled wrapper shape); refusals: no such session
   (existing wording), no result yet (existing wording), owned storage / existing file /
   missing parent (each by name, the path fences above). `manifest()` picks it up from the
   router untouched.
3. The answer states the written path and row/byte counts from the write pass — the
   engine's own figures, never restated.
4. Tests: mock-host tool tests (refusal matrix, happy path), one engine test proving
   `__strata_ord` is absent and order survives into the file, one proving the `.strata/`
   fence and the refuse-overwrite each hold (edge: a path *inside* a project's `.strata` —
   refused by the resolved-target gate, not by string prefix).
5. `docs/AGENT_ACCESS_SPEC.md`: the "What is not built" bullet becomes a "curated writes"
   section recording the decision above and its reasoning; system.md gains one line — the
   assistant may still prefer an `offer_sql` COPY card when the user should stay in the
   loop, and `export_result` when asked to save directly.

## As built (2026-08-14) — four corrections to the plan above

1. **No `Host` method, and no driver arm.** Build step 1 asked for
   `Host::export_result`; there is none, and the `strata-freya` files in the Files list are
   untouched. The export's source is the query session's own snapshot, which the *tool layer*
   holds (`LastRun`) and no host has ever seen, and the write touches no window state — so this
   is `read_page`'s case exactly, and it takes `read_page`'s route: straight at the
   `Arc<Engine>` every `Host` already hands over for the data plane. That is what makes "the
   headless host answers the same from its own engine" true with **zero** lines in
   `headless.rs`, and it keeps `Host` to what its own doc says it is ("do not grow it
   speculatively"). A `Host` method would have needed a channel hop to the window to fetch a
   snapshot id the caller already had.
2. **Four formats, not three.** `csv` / `ndjson` / `parquet` / **`arrow`** — the four
   `export::Format` writes. "The formats the export funnel supports" is a rule; "three of its
   four" would have been a hardcoded subset with nothing behind the omission. No write options
   ride with the choice: each format's `Default` (new, in `engine::export`, beside the types)
   is what a caller with no dialog gets, and the Export window is still where a user picks
   others.
3. **The path fence is eight refusals, not three.** The three named in the decision hold as
   written (owned storage, existing file, missing parent). Five more make them *answerable*
   rather than adding caution. Two are about the caller: an absolute path (a relative one
   resolves against a process cwd the agent cannot see, so "the parent exists" would be about a
   folder it never meant) and a local one (a remote target has no local file, so the
   no-overwrite promise could not be kept there). Three are read off DataFusion's own
   `FileOutputMode::single_file_output` and `ListingTableUrl::parse`, which between them decide
   what the target *is* — **no glob character, no trailing separator, and an extension on the
   last segment**. The last two were added in review after measuring the engine (below). Each
   refuses by name. The typed `COPY` keeps only the ownership one — a statement the user typed
   may overwrite their own file and may ask for a directory of part files.
4. **`refuse_owned_target` moved to `engine::export`** (with the subject parameterised, so
   `COPY`'s wording is byte-identical). `ddl::copy` reaches for it exactly as it already reached
   for `partition_columns_are_bare_words` and `partition_null_refusal`; three surfaces now write
   a result to a path and the user reads one rule. Its two tests moved with it.

**Two bugs review found by measuring DataFusion rather than reading it.** `COPY`'s default
`FileOutputMode::Automatic` writes a single file only when the target is not a collection **and**
its last segment carries an extension, and `ListingTableUrl::parse` splits a path containing
`?`, `*` or `[` into a prefix plus a glob. So, before the two rules above existed:

- `export_result(path=".../results", csv)` created a **directory** holding `<random>_0.csv`, and
  reported `bytes: Some(96)` — the directory inode's size — as the exported file's size, while
  the tool description promised it never creates folders.
- `export_result(path=".../report[1].csv")` reported **success at a path where no file exists**;
  the rows went to `.../<random>_0.csv` beside it.

Both are now refusals in `check_destination`, each naming its own repair. Setting
`single_file_output='true'` in the COPY options was the other candidate and was not taken: it
would have to thread through `ExportSpec` and change the Export window's construction site, and
it fixes only the first of the two — a glob is resolved at URL-parse time, before any sink mode
matters.

**A bug the fence found in the code it reused.** `resolve`'s `real.join(rest)` left a trailing
separator when `rest` was empty, and `stat("some-file/")` is `ENOTDIR` on Unix — so a resolved
path naming an existing *file* answered `exists() == false`, and the no-overwrite rule read it
as a free name. `starts_with` is component-wise, which is why the typed `COPY` never noticed.
Fixed in `resolve` and recorded there.

**And a gap in the test rig:** `MockProject::new` never called `Engine::set_data_dir`, which
every real host does — so the engine's `.strata/` fence had no project to fence and a path
inside it read as an ordinary folder that happens not to exist. The mock is "the executable
statement of what a host owes", so it says so now.

Answer shape: `{query_session, path, rows, bytes?}`. `rows` is `COPY`'s own count, `bytes` the
written file's size (absent only when the stat fails after a write that already succeeded).
The assistant's step card shows the row count through the existing `Facts`; no new card field.

## Acceptance

- An MCP agent saves a session's result to a path it names, in the asked format; the file
  carries no `__strata_ord`; the reply's figures are the engine's.
- Every refusal names its reason: owned storage, existing file, missing parent, no result.
- `run`'s policy is untouched — existing classification tests unedited.
- The decision and its reasoning are recorded here and in the spec.

## Files (as built)

`crates/strata-core/src/engine/export.rs` (the gates, the format defaults, `ExportReport`) ·
`crates/strata-core/src/engine/mod.rs` (`Engine::export_result`) ·
`crates/strata-core/src/engine/ddl/copy.rs` (reaches the moved fence) ·
`crates/strata-agent/src/{tools.rs, wire.rs, error.rs, mock.rs, lib.rs}` ·
`crates/strata-agent/src/assistant/{dispatch.rs, system.md}` ·
`crates/strata-agent/tests/{facade.rs, mcp_over_http.rs}` ·
`docs/AGENT_ACCESS_SPEC.md` · `docs/reference/{INVARIANTS.md, MODULE_MAP.md}` · `AGENTS.md`.

Untouched, deliberately: `host.rs`, `headless.rs`, and everything in `strata-freya` — see
correction 1.
