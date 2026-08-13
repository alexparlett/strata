# QE-05 · Agent result export — the first curated write

**Workstream:** Query ergonomics · **Status:** ⬜ (open decision for Alex below) ·
**Depends on:** nothing structurally; the permission decision gates implementation

## Goal

An agent can land a settled result on disk without a Python-CSV detour (feedback item 12) —
as the spec's reserved shape: a **new, separately permissioned tool**, never a loosening of
`run` (docs/AGENT_ACCESS_SPEC.md:437-439: "Curated writes … arrive as new, separately
permissioned tools; `run` never loosens"). This is the first curated write, so the shape it
takes becomes the precedent for any later one — that is why the permission model is decided
before code.

## The open decision (Alex)

A tool call must not block on a dialog (settled — the reason profiling isn't exposed at all,
spec :183-186), so per-call confirmation is out. The candidates:

- **A. Standing consent in Settings** — Settings ▸ AI (or MCP) gains "Agents may export
  results to disk", default **off**, plus a directory the exports land in (default e.g.
  `~/Downloads`, path chosen through the normal picker). The tool exists on the router
  always (a stable manifest), and refuses with "Result export is disabled in Settings" when
  the toggle is off — mirroring how every refusal names its fix. **Recommended**: it is the
  T2 principle ("only a gate that is a question for the user may be skipped") answered once,
  ahead of time, by the user.
- **B. Assistant-only, via the offer funnel** — no MCP tool at all; rely on `offer_sql`
  handing the user a `COPY … TO` card (validated under the **editor's** capability, which is
  what already lets the assistant offer writes it cannot run). Zero new permission surface,
  but headless/MCP agents — where the feedback came from — get nothing.
- **C. Both**: A for MCP, and note in system.md that the assistant should prefer the offer
  card (keeps the user in the loop) unless asked to save directly.

If A (or C), a sub-decision: is the destination the agent's to choose (a path parameter,
fenced inside the consented directory) or always minted by Strata (session-named file in the
consented directory, format parameter only)? Minted is safer and smaller; a path parameter
inside the fence is more useful. Recommend **minted for v1**.

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

## Build (once decided; written for option A/C)

1. Settings: the toggle + directory on the AI/MCP pane, through the existing settings merge
   machinery; both reach the app config store, never per-project.
2. `Host::export_result(session, format) → path` (the driver asks the engine; headless host
   answers the same from its own engine — same vocabulary, second deployment). Formats:
   csv / parquet / ndjson, reusing `EXPORT_OPTIONS`' vocabulary where it fits.
3. `StrataTools::export_result` public method + `#[tool]` wrapper (Busy guard + Caller
   resolution, then delegate — the settled wrapper shape); refusals: toggle off (names
   Settings), no such session (existing wording), no result yet (existing wording).
   `manifest()` picks it up from the router untouched.
4. The answer states the written path and row/byte counts from the write pass — the
   engine's own figures, never restated.
5. Tests: mock-host tool tests (refusal matrix, happy path), one engine test proving
   `__strata_ord` is absent and order survives into the file, one proving the `.strata/`
   fence holds even if the consented directory is inside a project (edge: consented dir
   *is* a project's `.strata` — refused by the resolved-target gate).
6. `docs/AGENT_ACCESS_SPEC.md`: the "What is not built" bullet moves to a "curated writes"
   section describing the permission shape; system.md tells the assistant when to prefer
   the offer card (option C's line).

## Acceptance

- With the toggle off (default), nothing changes anywhere — same manifest minus nothing (the
  tool is listed but refuses), `run` policy untouched, existing tests unedited.
- With consent, an MCP agent saves a session's result to the consented directory in the
  asked format; the file carries no `__strata_ord`; the reply's figures are the engine's.
- The permission decision and its reasoning are recorded here and in the spec.

## Files

`crates/strata-agent/src/{tools.rs, host.rs, wire.rs, headless.rs, mock.rs}` ·
`crates/strata-freya/src/agent/directory.rs` + `apps/project/state/agent.rs` (driver arm) ·
`crates/strata-core/src/engine/{export.rs or ddl/copy.rs}` (the engine method) ·
`crates/strata-core/src/config.rs`-adjacent settings plumbing + the Settings ▸ AI pane ·
`docs/AGENT_ACCESS_SPEC.md`.
