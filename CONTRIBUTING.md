# Contributing to Strata

Issues and PRs are welcome. The conventions, architecture summary and checks live in
[AGENTS.md](AGENTS.md) — written for humans and AI agents alike — and the system end to end is
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). This file is just the practical on-ramp.

## Getting a dev build

You need a Rust toolchain from [rustup](https://rustup.rs). The first build compiles Skia and
DataFusion from source and takes a while; after that it's a normal `cargo` loop.

```bash
git clone https://github.com/alexparlett/strata
cd strata
cargo run
```

The repo's `sample/` folder is a ready-made project with parquet, CSV, JSON, a Hive-partitioned
directory, views and saved queries — open it in the app to have real data to poke at.

## Before you open a PR

Run the checks in [AGENTS.md](AGENTS.md#checks): `cargo fmt --all`, clippy at `-D warnings`, and
`cargo test --workspace --locked`. Two integration tests drive real servers through
testcontainers, so tests need a container runtime (Docker, colima or Testcontainers Cloud);
without one they fail rather than skip, on purpose.

Then, for the PR itself:

- Target `main`.
- **Open with a short human-written paragraph**: why the work was done and who it helps. Put
  any generated technical description below it.
- New behaviour comes with a test; a bug fix comes with the test that would have caught it.
- If your change alters behaviour a document in `docs/` describes, change the document in the
  same PR.
- Keep the diff to the thing you're doing.

Looking for something to pick up? The
[issues](https://github.com/alexparlett/strata/issues) are the place to start, or open one to
talk an idea through first.

## On AI

AI assistance is welcome here — much of Strata is built with it — but the bar is the same for
every author: whoever opens the PR must understand the change and its consequences for the
codebase, whether they typed it or an agent did. Point your agent at [AGENTS.md](AGENTS.md).
PRs that read entirely machine-generated and don't engage with the codebase's conventions will
be closed. The longer statement of this policy is in the [README](README.md#use-of-ai).
