---
name: run-app
description: Launch the Strata app (the Freya/Skia native frontend) via cargo run. Use when asked to run, spin up, or visually check the app.
---

# Run Strata

> **One window across every session.** A `PreToolUse` hook (present on machines with the local
> hook set) refuses a launch while any Strata is alive — in *any*
> worktree — and names the directory that owns the running one. It is a refusal, not a kill: that
> window may be what someone is looking at right now. If it is stale, ask the user to close it or
> to run the `kill` the message quotes. Don't try to route around it; the guard exists because two
> instances quietly clobber each other's app config (`AppConfig` is read once at startup, and the
> last writer wins for recents, settings and the open-project set).

The repo's default-member is `strata-freya`, so from the repo root:

```bash
cargo run
```

opens Strata — a **native Skia window on the Mac's display**. There is no headless
mode: you can't drive or screenshot it from the terminal; verify a launch by the
process staying alive and its tracing output, and let the user look at the window.

## Details

- **Which window opens**: the projects that had a window at the last *quit* (⌘Q),
  one window each; otherwise the **launcher** (the welcome window). Closing every
  window by hand — rather than quitting — is what makes the next launch show the
  launcher.
- **Project folder**: argv[1] opens exactly that project and skips the above:
  `cargo run -- sample` for the committed sample project. A folder without a
  `.strata/` gets one scaffolded.
- **Run it in the background** (it blocks until the window closes) and read the
  stdout/stderr tracing output for errors — registration failures and engine
  errors log there.
- First build compiles Skia + DataFusion and takes a long time; that's normal.
  Use `cargo run --release` only when performance matters — the dev profile is
  fine for checking behaviour.
- Quit by closing the window (or kill the background process).
