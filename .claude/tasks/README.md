# Strata — Freya rewrite task breakdown

The working backlog for finishing the **Dioxus → Freya 0.4** rewrite. This is the *Freya* plan:
what's built, what's a shell, what's missing, and what's next — decomposed into **per-feature
tasks inside each migration phase**, because the phases themselves are very large.

Read this index first, then open only the phase/workstream file you're working in.

## How this is organised

- **One folder per phase / workstream.** Each has a `README.md` describing the phase and indexing its
  tasks, then **one file per task** — self-contained, with enough context (current state, what to
  build, acceptance, Freya components, source files/specs) that Claude Code can pick it up and
  implement it without loading everything else.
- **Phases** follow `docs/FREYA_PORT_PLAN.md` §6. Tasks are **feature-level** — small enough to pick
  up, finish, and verify on a Mac build in one sitting.
- **Two workstreams** (**Connections**, **Chart view**) sit *outside* the linear phases — they're
  large features that cut across surfaces and don't belong to one phase. They have their own files.
- Every task keeps its **`DEV_TASKS.md` ID** (U3, W1, D4, Rz2…) so it traces back to the full spec
  and honesty notes there. `docs/DEV_TASKS.md` is the **Dioxus** app's backlog — it's nearly all
  ✅ *there*, and it's the **parity target**: the Freya port has to reach it surface by surface.
- The **design source of truth** is the `.dc.html` canvases in `.claude/design-handoff/` (read the
  source, don't screenshot). `docs/DESIGN_SPEC.md` §14 + `docs/FEATURES.md` back them.

## The big framing (read before sizing anything)

- **The workbench is part-built, part-stub — and the round trip is now wired.** The datagrid core
  and tab strip are real, and since P2-03 the grid renders the **real result set** (the fixture is
  gone): page 1 rides the Run's output, later pages are cached snapshot reads, and a minimal pager
  sits in the status bar. Since P2-01/P2-02, editor → run → engine → results is live: the results
  state machine (empty / running / grid / explain / error) is driven by freya-query off the tab's
  SQL. Since P2-06 the **running** body is real (spinner · live elapsed · Cancel/Esc). Still to
  build: the **explain-plan** body content (its state is reached, the body is a placeholder), the
  **status bar** pager/info, and the **Table/Chart switcher, find, record view, copy** surfaces. So Phase 2 remains **build *and* wire** — per surface, on a live spine.
- **The core logic survives.** The DataFusion engine + `Command`/`Event` protocol, the SQL language
  service (`sql`), `serialize`, `plan`, `profile`, view-deps/validity, config, and `.strata`
  persistence all live in **`strata-core`/`strata-model`** and are done. So most remaining Freya work
  is **UI + wiring**, not rebuilding logic. A task tagged `[core ✓]` means "the hard logic exists;
  build the Freya surface and wire it."
- **Freya has been a slow, learn-as-we-go build** (the datagrid alone — hover, selection, resize,
  autofit — took many iterations against Freya's reactivity/event model). Size tasks accordingly; a
  "simple" surface often carries a Freya-idiom discovery cost. Prefer **reusing + theming Freya
  built-ins** (plan §5) over hand-rolling.

## Status legend

- ✅ **done** — built and wired in Freya (or awaiting a green Mac build).
- 🟢 **UI only** — the view exists but is a shell: on fixture data, decorative, or not dispatched.
- 🟡 **partial** — some of it works; specifics in the task.
- ⬜ **todo** — not started in Freya.
- `[core ✓]` — the underlying logic already exists in `strata-core`; only Freya UI/wiring remains.

## Where we are

| Phase | Scope | State |
|---|---|---|
| 0 · Core extraction | `strata-model` / `strata-core` split; both frontends on the shared core | ✅ done |
| 1 · Skeleton + engine round-trip | window shell, per-window state scaffold, engine bridge | ✅ shell up, round-trip wired (P2-01/02: direct-call facade + freya-query) |
| **2 · Workbench** | editor · results grid · tabs · run/explain · toolbar · status bar | 🟡 **datagrid + tabs + running body built; run/explain wired to real results states; plan body + chart-switcher/find/record/copy still to build** → `phase-2-workbench/` |
| 3 · Catalog + inspector + drawer | sidebar/catalog · column inspector + profiling · bottom drawer | ⬜ greenfield → `phase-3-catalog-inspector-drawer/` |
| 4 · Multi-window | launcher · settings · export · config modal · native close | ⬜ greenfield → `phase-4-multi-window/` |
| 5 · Design polish | spacing/radius tokens, hover/focus, animation, theme dial-in per surface | ⬜ ongoing → `phase-5-design-polish/` |
| 6 · Platform + parity | keymap/hotkeys · command palette · native menu · then delete Dioxus | ⬜ → `phase-6-platform-parity/` |

## Cross-cutting workstreams (not in a single phase)

- **Connections + remote object stores** (`workstream-connections/`, DEV_TASKS **W7**) — the
  activity-rail button, the sidebar connections pane, and the config-table LOCATION toggle +
  S3/GCS/HTTP object stores. Touches Phase 2/3/4 surfaces. Spec: `docs/CONNECTIONS_SPEC.md`.
- **Chart view** (`workstream-chart-view/`, DEV_TASKS **Rz2**) — the results Chart surface: chart
  types, encoder strip, client-side aggregate, guardrails. A whole feature surface, not drift.
  Spec: `docs/CHART_SPEC.md`.

## Known bugs (carried from DEV_TASKS; re-verify under Freya)

- **Re-opening the already-open project via Open Recent corrupts its saved paths** (relative source
  paths + partition columns mangled on next save). Was in the Dioxus `open_in_current` path — confirm
  whether the Freya session/persistence port reintroduces it.
- **Editing a view's SQL and pressing ⌘S saves a new saved-query instead of updating the view.** The
  editor needs to remember a tab's *origin* (a view) and route Save to `CREATE OR REPLACE VIEW`.

## Rough order

1. **Phase 2 plumbing first** — wire the query round-trip so the workbench is live, then light up each
   results feature (sort, record, copy, plan, find, clear, status aggregate) against real data.
2. **Phase 3** — catalog/inspector/drawer (the app is barely usable without the catalog).
3. **Phase 4** — launcher + settings + export windows (multi-window on shared state).
4. **Connections** + **Chart** workstreams (largest net-new features).
5. **Phase 5 polish** + **Phase 6 platform/parity**, then delete the Dioxus app.

## Sourcing

Derived from `docs/FREYA_PORT_PLAN.md` (phases, survives-vs-rewrite, Freya gotchas),
`docs/FREYA_STATE_ARCHITECTURE.md` (per-window state), `docs/DEV_TASKS.md` (the parity target +
per-surface drift + honesty calls + known bugs), the `.dc.html` design canvases, and the current
`strata-freya` tree.
