# QE-04 · Agent query-session lifetime

**Workstream:** Query ergonomics · **Status:** ⬜ · **Depends on:** nothing

## Goal

An agent mid-investigation stops losing its query sessions to the idle sweep (feedback item
11: three sessions gone in one working session, each taking the result the user was meant to
promote). The fix is a longer stated bound plus telling the model the bound exists — not a
pin, and not a recovery path for retired results, both of which are settled the other way.

## Current state (verified 2026-08-13)

- Five ways a session dies; the two that match the symptom:
  - **The idle sweep**: `STATELESS_IDLE = 300s` (`crates/strata-agent/src/tools.rs:155`),
    swept at `interval(STATELESS_IDLE / 2)` (`server.rs:194-202`), so retraction lands
    1.0–1.5× TTL after the last call. It applies **only to `Caller::Stateless`** agents
    (rmcp's stateless/per-request branch — exactly what claude.ai-style remote MCP clients
    negotiate); connected clients are retracted by `Drop` instead. The `Busy` guard re-stamps
    `seen` when a call **finishes** (`tools.rs:299-328`), so the killer is thinking time
    *between* calls — a model reasoning over a big result, or a human reading it, for >5
    minutes. That is not idleness, but the server cannot see the difference.
  - **The per-agent cap**: `SESSIONS_PER_AGENT = 20` (`apps/project/state/agents.rs:65`),
    oldest non-running evicted on the 21st open. The model is already told
    (`assistant/system.md:41-44`).
- Retirement is total by design: eviction/sweep runs `cleanup_ws`, the snapshot is retired,
  and `read_page` deliberately holds a cache, not a `SnapshotPin` ("pinning would keep a
  result alive past the run that owns it" — `tools.rs:121-123`; AGENTS.md: "`read_page` does
  **not** pin"). Recovery is re-run; the wording deliberately cannot distinguish
  expired-from-never-existed (`error.rs:57-69` — a distinct "not yours" would confirm
  existence). **None of that moves in this task.**
- The 300s figure was "matched to rmcp's own `SessionConfig::keep_alive`" (comment at the
  constant) — the sweep exists so stateless callers, which have no `Drop`, cannot leak
  agents forever. A longer TTL keeps that property; it just bounds the leak at a larger,
  still-finite cost (an idle agent entry + its retained workspaces).

## Build

1. Raise `STATELESS_IDLE` to **30 minutes**, updating the constant's doc comment: the old
   value's rationale (rmcp parity), the field failure that moved it (sessions swept while
   the client was reasoning between calls), and what bounds the new cost (the per-agent
   session cap and `MAX_REMEMBERED_RUNS` still bound state; the sweep still reaps the truly
   departed). Check what rmcp's `keep_alive` actually does on the stateless branch — if
   rmcp itself drops transport state at 5 minutes, a longer Strata TTL is what keeps the
   *sessions* alive across the client's next request, which is the point; note the finding
   either way.
2. **State the bound where the model plans**: the `open_query_session` tool description and
   `assistant/system.md` say sessions idle out after 30 minutes and that `run` re-creates
   cheaply; the recovery line stays `list_query_sessions`.
3. Keep the sweep test (`tools.rs:1561-1600`) green with the new constant; add one asserting
   the interval/TTL relationship rather than magic numbers, if it doesn't already.
4. `docs/AGENT_ACCESS_SPEC.md`'s session-lifecycle section gets the new figure and the
   reasoning sentence.

Explicitly out of scope, with the reason recorded here so it isn't re-litigated: pinning
results past their session (settled — the pin belongs to export-shaped surfaces),
distinguishing expired from unknown in the refusal (settled — non-confirmation of existence),
and raising `SESSIONS_PER_AGENT` (no evidence it was the killer; the model is already told
about it — revisit only if the symptom recurs after the TTL change).

## Acceptance

- A stateless agent silent for 20 minutes between calls keeps its sessions and its results.
- The bound is stated in the tool description, system.md, and the spec — one figure, three
  surfaces, no disagreement (derive the prose from the constant where the harness allows).
- Full check green; the sweep still retires a genuinely departed stateless agent.

## Files

`crates/strata-agent/src/tools.rs` (`STATELESS_IDLE` + tool doc) ·
`crates/strata-agent/src/server.rs` (comment only, the interval derives) ·
`crates/strata-agent/src/assistant/system.md` · `docs/AGENT_ACCESS_SPEC.md` · tests in
`tools.rs`.
