# QE-05 · Agent result export — the first curated write

**Workstream:** Query ergonomics · **Status:** ⬜ · **Depends on:** nothing

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

## Acceptance

- An MCP agent saves a session's result to a path it names, in the asked format; the file
  carries no `__strata_ord`; the reply's figures are the engine's.
- Every refusal names its reason: owned storage, existing file, missing parent, no result.
- `run`'s policy is untouched — existing classification tests unedited.
- The decision and its reasoning are recorded here and in the spec.

## Files

`crates/strata-agent/src/{tools.rs, host.rs, wire.rs, headless.rs, mock.rs}` ·
`crates/strata-freya/src/agent/directory.rs` + `apps/project/state/agent.rs` (driver arm) ·
`crates/strata-core/src/engine/{export.rs or ddl/copy.rs}` (the engine method) ·
`crates/strata-agent/src/assistant/system.md` · `docs/AGENT_ACCESS_SPEC.md`.
