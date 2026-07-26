# P3-12 · Drawer — Problems tab

**Phase:** 3 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** U10 · **Depends on:** P3-11, P2-18, P2-01

## Goal
Live per-tab diagnostics in the Problems tab.

## Current state
**P3-11 left three shared pieces to this task** (see its file): the header's **count label**
(`drawerCountLabel` — errors here), the **Clear show/hide rule** (hidden on Problems, so this task
owns the rule and P3-13/14 own the action), and the **list frame** — a scroll container and a
centred empty state, which is all the three tabs genuinely share. Build them with Problems as the
first consumer; P3-13/14 reuse. The drawer header already carries the title, the expand/restore
toggle and the collapse ×, and the **rail's bottom group is the tab switcher** — do not add a pill
row to the header.

The validation half is **already flowing** (P2-18 ✅): each debounced pass writes
`QueryTab::diagnostics` on its own **`Chan::Diagnostics(id)`** channel (read via
`SessionState::diagnostics(id)`). Diagnostics carry severity + message + `loc` (`line L:C`) — the
exact row shape below — plus a byte `span` for a future click-to-jump. The query-error half is the
tab's settled `RunQuery` Err (P2-01), synthesized at render via `Diagnostic::from_query_error`.

## Build
- Render `diagnostics(tab) ∪ query_error(tab)` for the **active tab** (state-arch §8) — **not**
  a log: subscribe `use_radio(Chan::Diagnostics(active))` for the validation half; derive the
  query-error half from the tab's freya-query state. They **self-clear** by construction — each
  validation pass replaces the vec wholesale (fixed SQL → next pass writes `[]`), and the query
  error lives in the run's cache entry (auto-clears on re-run). No dismissal state to build.
- Row = **icon · message · line** (no code chip — dropped in the Dioxus app, DEV_TASKS U10).
- **No Clear button** on Problems (the scaffold hides it — deliberate, do not "fix").
- Empty state: "No problems — queries are clean".

## Acceptance
- [ ] Problems reflects the active tab's diagnostics live and updates as the SQL changes / re-runs.
- [ ] No Clear button; empty state shows the clean message.

## Freya / references
- state-arch §8 (Problems = validation ∪ query_error). Core `sql::validate` + query error. Design:
  `DrawerProblems.dc.html`. DEV_TASKS U10 (row shape + the deliberate no-Clear divergence).
