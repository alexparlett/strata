# P2-02 · Results driven by `use_query` (no runs store)

**Phase:** 2 — Workbench · **Status:** ✅ **built** · **Depends on:** P2-01 · **Unblocks:** P2-05, P2-06, P2-14

> **Built as specced, one placement note:** the `request` slot lives in the **workbench** (its
> element owns both consumers) rather than the results element, threaded as struct-field props to
> the toolbar (writer) and `Results` (reader) — **not** context, and **no** per-tab request
> registry (a root-provided `HashMap<TabId, QuerySpec>` was tried and rejected in-session as a
> runs-store by another name). Per-tab results on switch come from the freya-query cache being
> keyed by the press's `QuerySpec` (which carries the tab), plus a `spec.tab == tab` filter in
> `Results`. Also added: `ResultsState::Error` + an `error.rs` body (the task's step 3 named an
> Error state that didn't exist), Run/Explain/Analyze press wiring in the toolbar (the dispatch
> half of P2-15), and a workbench side effect dropping the slot when the pressed tab closes.

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
- [x] Fresh tab → Empty. Run → Running → Grid/Error. Explain → ExplainPlan. Editing doesn't re-run.
      (Wired + clean build + capability round-trip tests; on-screen walkthrough pending a `cargo run`.)
- [x] Switching tabs shows each tab's own results (query is keyed by the tab's `QuerySpec`; the
      cache re-serves a revisited tab's settled outcome with zero engine traffic while its press
      is current — a press in *another* tab supersedes the slot, by design: one execution per window).

## Freya / references
- `docs/FREYA_STATE_ARCHITECTURE.md` §6. `results/mod.rs`. Query state = freya-query `QueryStateData`.

## Correction (2026-07-26 · background-tab cache lifetime)

The acceptance parenthetical above predates per-tab request slots: each tab owns
`QueryTab::request` now, and tabs run concurrently (engine in-flight is per `WsId`) — there is
no one-execution-per-window slot.

A tab-switching review found a backgrounded press's outcome survived only by accident:
freya-query's `clean_time` eviction (5 min after the last subscriber unmounts) never fired
because its subscriber check counted reactive contexts, which haven't unsubscribed yet when
drop callbacks run — and had it fired, a revisit past it would have silently re-executed the
press (aborting it mid-flight if still running). Settled fix, both halves required
(AGENTS §2, SNAPSHOT_SPEC §6):

- **Fork**: freya-query counts `use_query` subscribers explicitly; cleanup schedules only when
  the last one unmounts and never removes an entry whose execution is in flight
  (`crates/freya-query/tests/query_clean_time.rs`).
- **App**: `views/keeper.rs` mounts one invisible subscriber per open tab's current press,
  so entry lifetime tracks request *currency*, not tab visibility; superseded / cancelled /
  closed presses age out on the default clean time. Every Run subscription is built through
  `QuerySpec::query` — a `Query`'s settings are cache identity, so a hand-built variant is a
  duplicate execution. History recording moved into the keeper's pin, which observes a
  background settle at real completion time.
