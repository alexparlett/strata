---
name: run-app
description: Launch the Strata app (the Freya/Skia native frontend) via cargo run. Use when asked to run, spin up, or visually check the app.
---

# Run Strata (the Freya app)

The repo's default-member is `strata-freya`, so from the repo root:

```bash
cargo run
```

opens the Strata project window — a **native Skia window on the Mac's display**.
There is no headless mode: you can't drive or screenshot it from the terminal;
verify a launch by the process staying alive and its tracing output, and let the
user look at the window.

## Details

- **Project folder**: argv[1] is the project to open, defaulting to the committed
  `sample/`: `cargo run -- path/to/project`. A folder without a `.strata/` gets
  one scaffolded.
- **Run it in the background** (it blocks until the window closes) and read the
  stdout/stderr tracing output for errors — registration failures and engine
  errors log there.
- First build compiles Skia + DataFusion and takes a long time; that's normal.
  Use `cargo run --release` only when performance matters — the dev profile is
  fine for checking behaviour.
- Quit by closing the window (or kill the background process).
