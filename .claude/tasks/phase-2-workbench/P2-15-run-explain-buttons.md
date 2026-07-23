# P2-15 · Run / Explain / Analyze button wiring

**Phase:** 2 — Workbench · **Status:** ✅ **built** · **DEV_TASKS:** E4 · **Depends on:** P2-01

## Goal
The three editor-toolbar run controls dispatch, and Run flips to a red Cancel while running.

## Current state
`editor/toolbar.rs` renders `RunButton(RunState::Idle)` + Explain + Analyze buttons; actions stubbed.
`components/run_button.rs` has the three-state control.

## Build
1. **Run** (⌘↵) sets `request` (P2-02); while `Loading`, `RunButton` shows red **Cancel** →
   `query.cancel()` + engine cancel.
2. **Explain plan** / **Explain analyze** route the SQL through `plan::as_explain` (strip + reapply
   `EXPLAIN [ANALYZE]`) via the shared explain path — **editor buffer untouched** (like Save-as-view).
3. Each control sets `request` with the matching `QueryMode` (Run / Plan / Explain); the results
   body follows from the query state (P2-02).

**As built:** dispatch (all three controls → fresh-nonce `QuerySpec` in the `request` slot, with
`as_explain` applied inside the `RunQuery` capability, buffer untouched) had landed with P2-02.
P2-15 added the state flips. The toolbar can't observe "running" from `request` (it stays `Some`
after settle to keep the grid mounted) and **must not** subscribe the run's `use_query` itself —
freya-query re-runs *stale* entries when a subscriber mounts and an in-flight entry reads as
stale, so a second enabled subscriber would double-execute the run. Instead the workbench holds a
second component-local slot, `running: State<Option<RunId>>`, threaded as props beside `request`;
`ResultsBody` (the sole query subscriber) resolves it via `use_side_effect` — the press's nonce
while Pending/Loading, `None` on settle — with a nonce-guarded `use_drop` so a stale unmount
(cancel / supersede / tab close) can't clobber a newer press's flag. The toolbar shows
`RunState::Running` when the current request for its tab matches the mirrored nonce; that press is
Cancel (`engine.cancel(tab, run)` + `request = None`, the same action as the Running body's
control). A blank buffer (whitespace-only rope, checked without materialising a `String`) gates
Run to `RunState::Disabled`; Explain/Analyze keep the press-time blank guard. ⌘↵ itself is P2-20.

## Acceptance
- [x] Run executes; Run→Cancel mid-run works. Explain/Analyze produce the plan view without editing the buffer.

## Freya / references
- `editor/toolbar.rs`, `components/run_button.rs`. Core `plan::as_explain` (unit-tested).
