# P2-15 · Run / Explain / Analyze button wiring

**Phase:** 2 — Workbench · **Status:** 🟢 · **DEV_TASKS:** E4 · **Depends on:** P2-01

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

## Acceptance
- [ ] Run executes; Run→Cancel mid-run works. Explain/Analyze produce the plan view without editing the buffer.

## Freya / references
- `editor/toolbar.rs`, `components/run_button.rs`. Core `plan::as_explain` (unit-tested).
