# Phase 2 — Workbench

The core UX: editor · results grid · tabs · run/explain · toolbar · status bar.

## State of play (honest)

It is **not** "UI done, plumbing missing." Three kinds of work remain:

- **Genuinely built (on a static `fixture()`):** the datagrid core (typed header, type-coloured
  cells, selection + 2px ring, per-column resize + double-click autofit, hover) and the tab strip
  (open/close/rename/context-menu/drag/overflow). The editor toolbar renders its full button row and
  a `CodeEditor` is mounted per tab.
- **Stub / not built:** results state is hardcoded to `Grid` (`Results::new()` is a spike);
  `running.rs` and `explain_plan.rs` are one-line placeholders; the status bar shows only a dot +
  label + selection; there's no Table/Chart switcher, find popover, record view, cell/gutter double-click views, or copy menu; and the
  editor has **no completions/diagnostics** binding (SQL highlighting *is* wired).
- **No plumbing:** `EngineCtx` spawns the engine and can `send`, but nothing wires editor → run →
  engine → results (no `use_query` yet).

The logic behind every feature already lives in `strata-core` (`[core ✓]`). Do the **plumbing** tasks
first — they unblock the rest. **P2-01 folds in the result snapshot system, which needs a design
decision** before pagination/sort/filter/export rest on it.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| P2-01 | **Query round-trip + result snapshot system (design + build)** | ✅ | — | — |
| P2-02 | Results driven by `use_query` (no runs store) | ⬜ | — | P2-01 |
| P2-03 | `QueryPage` → grid model (kill fixture) | ⬜ | — | P2-01 |
| P2-04 | SQL autocomplete (completions + follow-ups) | ⬜ | E2 | — |
| P2-05 | Explain-plan view | ⬜ | Rz-plan/U8 | P2-02/03 |
| P2-06 | Running state (spinner + elapsed + cancel) | ⬜ | — | P2-02 |
| P2-07 | Table/Chart switcher | ⬜ | U6a | P2-02 |
| P2-08 | Status bar — pager + info + aggregate | 🟢 | U6/Rz3 | P2-03 |
| P2-09 | Find in results | ⬜ | U6c | P2-03 |
| P2-10 | Gutter double-click → row detail (record view) | ⬜ | Rz5 | P2-03 |
| P2-11 | Copy affordances (TSV/CSV/JSON/MD) | ⬜ | Rz4 | P2-03 |
| P2-12 | Cell double-click → nested-data view | ⬜ | U5 | P2-03 |
| P2-13 | Column sort | 🟢 | Rz6 | P2-01/03 |
| P2-14 | Clear results | 🟢 | Rz8 | P2-02 |
| P2-15 | Run / Explain / Analyze buttons wiring | 🟢 | E4 | P2-01 |
| P2-16 | Editor toolbar actions (Format/Preview/Save-as-view) | 🟢 | — | P2-01 |
| P2-18 | SQL validation + inline squiggles | ⬜ | E1 | — |
| P2-19 | Undo/redo per tab | ⬜ | E3 | — |
| P2-20 | Keyboard shortcuts + OS-close intercept | ⬜ | W4/T2 | — |
| P2-21 | Tabs & split — remaining nits | 🟡 | — | — |

**Already done (no task file):** datagrid core, cell/row/col selection (`selection.rs` + `SelCtl`),
resizable columns + autofit, tab strip (`tab_bar/*`).

## Legend

✅ done · 🟢 UI only (shell/inert) · 🟡 partial · ⬜ todo · `[core ✓]` logic exists in `strata-core`.
