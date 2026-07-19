# Freya port — Phase 1 (skeleton + engine round-trip)

Building Freya blind (no Skia here), so this lands in small build-checkable increments.
Plan: `docs/FREYA_PORT_PLAN.md` §6 (phase 1). API basis: the `freya` skill + the spike
gotchas in memory `freya-migration`.

## 1a — skeleton (this step)

`crates/strata-freya`: a Tokio context held for the program (Freya's runtime isn't Tokio)
+ one placeholder window. **freya + tokio only, no strata-core yet** — so this first build
validates just the Skia/Freya toolchain in the workspace, isolated from the datafusion tree
(1b adds that).

**Build:** `cargo run -p strata-freya` (first Skia compile is slow). Bare `cargo run` at the
root is unaffected — `strata-freya` is a member but excluded from `default-members`.

⚠️ **Version risk to confirm on first build:** `freya = "0.4"`. If 0.4 isn't on crates.io
(the own-reactive-core line may still be git-only), switch to:
```toml
freya = { git = "https://github.com/marc2332/freya" }   # + rev = "..." to pin
```
and tell me the spec that resolved, so I pin it for 1b.

Other things that may need a first-build tweak (all small — the skeleton mirrors the skill's
exact `launch(LaunchConfig::new().with_window(WindowConfig::new(app)))` example):
- `use freya::prelude::*` should cover `rect`/`launch`/`LaunchConfig`/`WindowConfig`.
- window title/size intentionally omitted here; added in 1b once the toolchain is proven.

## 1b — engine bridge + round-trip (next)

Add `strata-core`/`strata-model` deps; spawn `strata_core::engine::Engine`; put the cmd
handle in root context; drain the `Event` stream in a Freya `spawn` loop into a Radio
station; a SQL input + Run button → `Command::Query` (start with `SELECT` literals, no
tables) → render `QueryOutput` rows. Proves the shared core runs real queries under Freya.
