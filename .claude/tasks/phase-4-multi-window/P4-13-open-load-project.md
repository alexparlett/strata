# P4-13 · Open / create a project (`.strata/` load)

**Phase:** 4 · **Status:** 🟡 `[core ✓ IO]` **internals done, UI pending** · **DEV_TASKS:** project lifecycle · **Depends on:** P4-01 · **Feeds:** Phase 2/3 (the window needs a real project)

> **🟡 Landed (internals, no UI):** `strata_core::project` (`.strata/project.json` defs IO:
> load/save/scaffold + `resolve_source`/`relativize`), `Engine::create_view`/`drop_view` (consuming
> the `plan_deps` reservoir), the per-window **`ProjectState`** Radio store (`ProjChan`), and
> `use_init_project` in the window root: opens argv\[1\] (default the committed `sample/`),
> scaffolds a fresh `.strata/` when absent, and registers tables → views (fixed-point retry for
> view-on-view deps) as a background task, landing per-row `Loading → Ready/Failed`. Covered by
> `strata-core/tests/project_load.rs` against `sample/`. **Remaining:** the open/new-project UI
> (launcher P4-02 / Open Recent / folder pick), window title/switcher, session restore (P4-14),
> and the re-open-in-place path guard below.

> **Sequencing note:** this is the *load* half of project lifecycle. The launcher (P4-02) is one
> entry point, but the project window can open a project directly — so the load path is a prerequisite
> that will likely be pulled **earlier** than the rest of Phase 4 (nothing in the workbench/catalog is
> real without a loaded project). `main.rs` currently launches `ProjectApp::window()` with **no
> project loaded**.

## Goal
Open a `.strata/` project and bring the window fully to life: register its catalog + restore its
session.

## Current state
Not built. `session.rs` says persistence is "a later slice." Core provides the `.strata/` IO +
`project.json` / `session.json` formats (top README; state-arch §5). `project.json` = shareable
catalog **defs** (committed); `session.json` = local session state (gitignored).

**The UI seam is already built and waiting** (P3-15): the header's project switcher
(`views/header/project_menu.rs`) lists **Open…**, the open set and the recents off real state, and
each row logs a `tracing::debug!` stub instead of opening. Wire those presses to *this* task's open
path (with P4-01's This-Window / New-Window prompt) — one mechanism, not a header-local fold.

## Build (state-arch §5)
1. On open (launcher P4-02 / Open Recent / folder pick), read **`.strata/project.json`** — catalog
   **defs** (external tables · views · saved queries) — and register them on the engine (the same
   register / create-view commands).
2. Read **`.strata/session.json`** (`SessionSnapshot`) → rebuild each `QueryTab`
   (`CodeEditorData::new(Rope::from(text), lang)`), the order / active / closed stack, history, layout,
   inspector selection, and window geometry.
3. **New project:** scaffold a `.strata/` dir (`project.json` + `session.json`) for a chosen folder.
4. Set `project_path` on the Project store; window title / switcher reflect it.
5. ⚠️ **Guard the re-open-in-place bug** (Known bugs): re-opening the already-open project must not
   mangle relative source paths / partition columns.
6. **Unrecoverable open error → close the window, don't fall back.** A project can't exist without a
   root, so `ProjectState` is always built full (no `Default`, no rootless in-memory project). A
   folder that won't canonicalize — or a defs file that won't load / scaffold — currently **`panic!`s**
   in `open_project` (interim). The real handling (close this window; open the launcher if it was the
   last) is **P4-01 build item 5** — swap the panic for that close path when it exists.

## Acceptance
- [ ] Opening a `.strata/` project registers its tables/views and restores tabs + history + layout.
- [ ] New-project scaffolds a `.strata/` dir; re-opening the same project doesn't corrupt paths.

## Freya / references
- state-arch §5 (SessionSnapshot load, `.strata/` split). Core `.strata/` IO + register/create-view.
  DEV_TASKS Known bugs. Memory `project-persistence` (defs vs session split).
