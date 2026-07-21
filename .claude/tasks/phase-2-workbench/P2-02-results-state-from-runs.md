# P2-02 · Results driven by `use_query` (no runs store)

**Phase:** 2 — Workbench · **Status:** ⬜ · **Depends on:** P2-01 · **Unblocks:** P2-05, P2-06, P2-14

## Goal
The results pane runs the active tab's SQL via `use_query` and derives its body from the query's own
loading/settled state — **not** from a hardcoded value or a session run-slice.

## Current state
`results/mod.rs`: `Results::new()` hardcodes `ResultsState::Grid` ("SPIKE"). No `use_query`, no
request signal.

## Build (state-arch §6)
1. In the results element: `let mut request = use_state(|| None::<QuerySpec>)`. **Run** snapshots the
   active tab's editor text → `request.set(Some(QuerySpec { sql, mode: Run, page: first, … }))`.
   Editing the buffer does **not** touch `request` (so it doesn't re-run).
2. `let query = use_query(Query::new(request()?, RunQuery(gateway.captured())))`.
3. Derive the body from `query.read().state()` (`QueryStateData`): `Pending`/`Loading` → **Running**;
   `Settled(Ok(page))` → **Grid** (or **ExplainPlan** when `mode != Run`); `Settled(Err)` → **Error**;
   `request == None` → **Empty**. Remove the `ResultsState::Grid` spike and the enum-only switch.
4. Plan/Explain are the **same** `use_query` pattern with a different `QueryMode` (P2-05, P2-15).

## Acceptance
- [ ] Fresh tab → Empty. Run → Running → Grid/Error. Explain → ExplainPlan. Editing doesn't re-run.
- [ ] Switching tabs shows each tab's own results (query is keyed by the tab's `QuerySpec`).

## Freya / references
- `docs/FREYA_STATE_ARCHITECTURE.md` §6. `results/mod.rs`. Query state = freya-query `QueryStateData`.
