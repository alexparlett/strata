# P4-13 · Open / create a project (`.strata/` load)

**Phase:** 4 · **Status:** 🟡 `[core ✓ IO]` **internals + open routing done; new-project UI pending** · **DEV_TASKS:** project lifecycle · **Depends on:** P4-01 · **Feeds:** Phase 2/3 (the window needs a real project)

> **🟡 Landed (internals, no UI):** `strata_core::project` (`.strata/project.json` defs IO:
> load/save/scaffold + `resolve_source`/`relativize`), `Engine::create_view`/`drop_view` (consuming
> the `plan_deps` reservoir), the per-window **`ProjectState`** Radio store (`ProjChan`), and
> `use_init_project` in the window root: opens argv\[1\] (default the committed `sample/`),
> scaffolds a fresh `.strata/` when absent, and registers tables → views (fixed-point retry for
> view-on-view deps) as a background task, landing per-row `Loading → Ready/Failed`. Covered by
> `strata-core/tests/project_load.rs` against `sample/`.
>
> **✅ Landed (the open path + This/New prompt, 2026-07-25).** `OpenPref` is read at last.
> Everything below is built; what remains of this task is the **new-project** UI (a *create*
> flow — the picker only opens existing folders, scaffolding one silently if it has no
> `.strata/`) and window title/switcher polish.
>
> - **`platform::open` — one routing, four surfaces.** `OpenCtx` (the window's current project
>   root + its pending This/New question) is provided at the project window root, and ⌘O,
>   File ▸ Open…, File ▸ Open Recent and the header switcher's rows all go through it.
>   `decide()` is a pure rule over plain values (unit-tested): the project this window already
>   shows is a no-op, a project another window already has is **focused** (two windows would
>   both autosave over one `session.json` — this outranks the pref), otherwise `OpenPref`
>   decides. Acting is split from deciding (`OpenTarget`) because the menubar handler runs on
>   the renderer with no `Platform`.
> - **This Window is a keyed remount, not a reopen path.** `ProjectApp` is now two layers: the
>   **window** (theme, app-globals, close bridge, menubar, open path) and `ProjectRoot` — the
>   **open project** (engine, Project/Session/History stores, autosave, catalog, every view),
>   whose `render_key` is the project folder. So "open in this window" is a plain `State` write:
>   the old subtree drops (flushing its session, dropping its engine, leaving the open-set) and
>   the new one mounts through the very same hooks that run at launch. There is no second
>   reopen-in-place path to keep in step with the mount path, which is what item 5 below was
>   worried about. Covered by `project::tests::changing_the_root_remounts_the_project_subtree`.
> - **`use_autosave` now saves on `use_drop` too** — the debounce is a task, and a task dies
>   with its scope, so a close *or* a re-root inside the debounce window would have lost the
>   last few hundred ms.
> - **Menubar Open Recent** is the one File item that carries data rather than synthesizing a
>   chord, so the focused window parks its `OpenCtx` in `AppCtx::open` (`use_file_menu`) for it
>   to reach. The launcher parks `None` and keeps its old behaviour.
> - **Still unwired:** `OpenPref` has no *settings* control — P4-06 (Settings ▸ System) owns
>   that. Until it lands the only way to change it is the prompt's "Remember, don't ask again",
>   which is enough to reach all three states.

> **Sequencing note:** this is the *load* half of project lifecycle. The launcher (P4-02) is one
> entry point, but the project window can open a project directly — so the load path is a prerequisite
> that will likely be pulled **earlier** than the rest of Phase 4 (nothing in the workbench/catalog is
> real without a loaded project). `main.rs` currently launches `ProjectApp::window()` with **no
> project loaded**.

## Goal
Open a `.strata/` project and bring the window fully to life: register its catalog + restore its
session.

## Current state
Core provides the `.strata/` IO + `project.json` / `session.json` formats (top README;
state-arch §5). `project.json` = shareable catalog **defs** (committed); `session.json` = local
session state (gitignored).

The header switcher, ⌘O, File ▸ Open… and File ▸ Open Recent all open real projects through the
one `platform::open` path, honouring `OpenPref` (see the landed note above). What is **not** built
is *creating* a project deliberately: today a picked folder without a `.strata/` is scaffolded
silently, which is the right fallback but not a New Project affordance.

## Build (state-arch §5)
1. On open (launcher P4-02 / Open Recent / folder pick), read **`.strata/project.json`** — catalog
   **defs** (external tables · views · saved queries) — and register them on the engine (the same
   register / create-view commands).
2. Read **`.strata/session.json`** (`SessionSnapshot`) → rebuild each `QueryTab`
   (`CodeEditorData::new(Rope::from(text), lang)`), the order / active / closed stack, history, layout,
   inspector selection, and window geometry.
3. **New project:** scaffold a `.strata/` dir (`project.json` + `session.json`) for a chosen folder.
4. Set `project_path` on the Project store; window title / switcher reflect it.
5. ✅ **The re-open-in-place bug is designed out** (Known bugs): re-opening the already-open project
   is a no-op (`decide` returns `Nothing`), and opening a *different* project in place remounts
   rather than mutating a live store — nothing re-reads or re-relativizes an open project's defs,
   so there is no path on which relative sources / partition columns can be mangled.
6. **Unrecoverable open error → close the window, don't fall back.** A project can't exist without a
   root, so `ProjectState` is always built full (no `Default`, no rootless in-memory project). A
   folder that won't canonicalize — or a defs file that won't load / scaffold — currently **`panic!`s**
   in `open_project` (interim). The real handling (close this window; open the launcher if it was the
   last) is **P4-01 build item 5** — swap the panic for that close path when it exists.

## Acceptance
- [x] Opening a `.strata/` project registers its tables/views and restores tabs + history + layout.
- [ ] New-project scaffolds a `.strata/` dir; re-opening the same project doesn't corrupt paths.
      (The re-open half holds — see build item 5. The **New Project** affordance is what's left.)
- [x] An open from a window that already has a project honours `OpenPref`: This Window re-roots in
      place, New Window opens one, Ask raises the prompt, and "Remember" persists the answer.

### Re-rooting asks before it destroys work
Opening in place unmounts the project subtree, and dropping its engine aborts **every in-flight
query** — the same loss ⇧⌘W, the red button and ⌘Q all stop and ask about. So `OpenCtx::reroot`
goes through that very dialog: `CloseTarget::Reroot(PathBuf)` carries the folder, the T2 confirm
grows a third copy variant ("Confirm open" / "Stop & open"), and answering it calls
`reroot_confirmed`. Cancelling leaves the window on its project with the queries still running.

The gate is `OpenCtx`'s because all four surfaces reach the re-root through it; the guard and the
confirm slot ride in `State` slots so `OpenCtx` stays `Copy`, the same trick `TabCloser` uses for
its engine. `CloseTarget` is `Clone` rather than `Copy` now that one variant carries a path — a
handful of `*confirm.read()` sites became `.clone()`.

This also settles a dialog-ordering wrinkle: `OpenPrompt` is mounted above `CloseConfirm` in
document order (deliberately — the canvas puts it at z 210 against the confirm's 96), so if both
were up, Enter would take "This Window" instead of "Stop & exit". Answering the open question is
now what *raises* the confirm, so the re-root path can't put both on screen at once.

### Coverage, and the two gaps in it
`decide`'s rules (`platform::open::tests`), the keyed remount
(`project::tests::changing_the_root_remounts_the_project_subtree`) and the rendered dialog
(`open_prompt::interaction` — headless `TestingRunner`: the card's presence, This Window
re-rooting, Enter taking the primary, and Cancel/Esc/backdrop all opening nothing) are covered.
Alex click-tested the rest in the real app on 2026-07-25.

Two things no test drives, and why (the module doc on `open_prompt::interaction` has the long
version):

- **The New Window press.** It reaches `Platform::launch_window`, which awaits a renderer ack the
  headless harness never sends and `expect`s on it — the spawned task panics on the first poll.
  Loosening that `expect` would widen a production signature to suit a test, so the button's
  routing rests on `platform::open::tests` proving the same `OpenTarget::NewWindow` decision.
- **"Remember" persisting the pref.** `write_config` funnels to the *real* user config file, so a
  test that ticked the box would overwrite the developer's own settings and recents. Covering it
  wants a config path a test can redirect — not worth a production seam today. (Alex deliberately
  left this untried in the app too: with no Settings ▸ System control yet, a remembered This/New
  would be stuck on until P4-06 lands or the JSON is hand-edited.)

## Freya / references
- state-arch §5 (SessionSnapshot load, `.strata/` split). Core `.strata/` IO + register/create-view.
  DEV_TASKS Known bugs. Memory `project-persistence` (defs vs session split).
