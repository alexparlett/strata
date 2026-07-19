# Freya port — Phase 1 (skeleton + engine round-trip)

Building Freya blind (no Skia here), so this lands in small build-checkable increments.
Plan: `docs/FREYA_PORT_PLAN.md` §6 (phase 1). API basis: the `freya` skill + the spike
gotchas in memory `freya-migration`.

## Runtime decision (why no `rt.enter()` in `main`)

The engine already owns a Tokio runtime **on its own dedicated thread** (`Engine::spawn` →
`std::thread` → `rt.block_on(engine_loop)`); all DataFusion async runs there. The UI side only
ever does `cmd_tx.send()` (non-blocking, sync — no runtime) and `evt_rx.recv().await`. Because
`tokio::sync` channels are **executor-agnostic** (unlike `tokio::time`/`net`, they don't need
the Tokio reactor), Freya's own `spawn()` can drain the event stream directly. So `main` just
`launch(...)`s — no second runtime.

A held `rt.enter()` is genuinely required *only* if we call `tokio::time` or a Tokio-ecosystem
crate (reqwest/sqlx) **directly on the UI thread**, which this architecture never does. (This
refines the earlier, over-cautious "hold `rt.enter()` for the whole program" note.)

⚠️ *If* 1b's first run ever panics on `evt_rx.recv().await` with a "no reactor / must be called
from a runtime" message (not expected — `tokio::sync` is runtime-independent), the minimal fix
is one background `tokio::runtime` + `rt.enter()` in `main`. Don't add it pre-emptively.

## 1a — skeleton (this step)

`crates/strata-freya`: one placeholder window, laid out in the valin-style structure from
plan §3 from the start (so 1b/Phase 2 grow into it, no later reshuffle):

```
src/
├── main.rs                 launch the project window (mod apps)
└── apps/                   one folder per OS window (Phase 1 = project only)
    ├── mod.rs
    └── project/
        ├── mod.rs          wiring / re-export (grows: mod state; mod views; mod commands)
        └── project.rs      the window root shell (placeholder body for now)
```

Top-level `state/` (global singletons), `engine/` (bridge), `components/` (DS widgets),
`theme.rs`, `platform/`, and the other `apps/*` windows are created by the phase that needs
them, not stubbed now.

**freya only — no tokio, no strata-core** — so this first build validates just the Skia/Freya
toolchain in the workspace, isolated from the datafusion tree (1b adds that).

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

## 1b — engine bridge + round-trip

Deps added: `strata-core`, `strata-model`, `tokio = { features = ["sync"] }` (sync **only** —
just to name `UnboundedReceiver<Event>`; no runtime).

**Slice 1 (this build) — prove the bridge, minimal UI.** `src/engine/mod.rs` spawns the core
engine and returns a cloneable `EngineCtx` (sending half) + the event `evt_rx`; needed a tiny
core addition, `Engine::sender()` (clone of `cmd_tx`, so the whole non-`Clone` handle needn't
go into context). `ProjectApp::render` spawns the engine in a `use_hook`, drains `evt_rx` in a
Freya `spawn` into `use_state`, and a **Run** button fires a hardcoded `Command::Query`
(`SELECT 1 AS n, 'hello' AS greeting` — no tables), rendered as a plain text table on
`Event::QueryResult`. This validates: engine spawns under Freya, the `tokio::sync` drain works
on Freya's executor, and the round-trip lands.

Likely first-build wrinkles (blind Freya API — all small): the `.children(iter)` /
`.width(Size::px(..))` / `.map`/`.maybe` builder signatures, and whether `use_state.set()` from
inside `spawn` needs any extra bound. Report anything and I'll adjust.

**Slice 2 (next) — station + input.** Promote `result`/`error`/`running` into a per-window
Radio station (`apps/project/state.rs`, `use_init_radio_station`), add a Freya `Input`
two-way-bound SQL box (`State<String>::into_writable()`), and extract the body into
`apps/project/views/workbench.rs`. Then Phase 1 is complete.
